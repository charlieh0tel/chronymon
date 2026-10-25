//! Opening the database to write it, migrating it, and the rows the
//! daemon writes.

use std::path::Path;
use std::time::Duration;

use chronymon::event::EventKind;
use chronymon_control::report::Address;
use chronymon_control::report::NtpData;
use chronymon_control::report::ReferenceId;
use chronymon_control::report::SelectData;
use chronymon_control::report::SourceData;
use chronymon_control::report::SourceMode;
use chronymon_control::report::SourceStats;
use chronymon_control::report::Tracking;
use jiff::Timestamp;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::Transaction;
use rusqlite::params;

use crate::error::Error;
use crate::error::Result;
use crate::schema::META;
use crate::schema::TABLES;
use crate::schema::VERSION;
use crate::text;
use crate::timestamp::to_stored;

/// How long a statement waits for another writer -- an operator's
/// `sqlite3`, say -- before failing.  WAL readers never block it.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// What the daemon last saw of chronyd, for the `status` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChronydState {
    /// Answering.
    Up,
    /// Not answering.
    Down,
    /// Answering with replies this version cannot decode.
    Unsupported,
}

impl ChronydState {
    /// The name stored in the status row and shown.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Unsupported => "unsupported",
        }
    }
}

/// The database, open for writing.
#[derive(Debug)]
pub struct Log {
    connection: Connection,
}

impl Log {
    /// Open or create the database at `path`, migrating it to
    /// [`VERSION`].
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).map_err(|source| Error::Open {
            path: path.to_owned(),
            source,
        })?;
        Self::configure(connection)
    }

    /// An in-memory database, for tests.
    pub fn in_memory() -> Result<Self> {
        Self::configure(Connection::open_in_memory()?)
    }

    fn configure(connection: Connection) -> Result<Self> {
        // WAL lets readers read while the daemon writes; NORMAL rather
        // than FULL, since a row lost to a power cut is one second.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        let mut log = Self { connection };
        log.migrate()?;
        Ok(log)
    }

    /// Bring the database to [`VERSION`]: refuse a newer one, create
    /// what an older or empty one lacks.
    fn migrate(&mut self) -> Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute_batch(META)?;
        let found: Option<i64> = transaction
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(found) = found
            && found > VERSION
        {
            return Err(Error::NewerSchema {
                found,
                supported: VERSION,
            });
        }
        transaction.execute_batch(TABLES)?;
        transaction.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![VERSION.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Write one poll's rows together, so a reader never sees half a
    /// poll.
    pub fn write<T>(&mut self, f: impl FnOnce(&Batch<'_>) -> Result<T>) -> Result<T> {
        let transaction = self.connection.transaction()?;
        let result = f(&Batch {
            transaction: &transaction,
        })?;
        transaction.commit()?;
        Ok(result)
    }
}

/// The rows of one poll, written in one transaction.
#[derive(Debug)]
pub struct Batch<'a> {
    transaction: &'a Transaction<'a>,
}

/// How a source's address is stored: the IP address, or a refclock's
/// reference ID in hex.
fn address(address: Address) -> String {
    address.to_string()
}

impl Batch<'_> {
    /// A fact about the database or the daemon's last run, by key.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .transaction
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Record a fact under `key`.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Record the daemon's view of chronyd as of `at`.
    pub fn status(
        &self,
        at: Timestamp,
        state: ChronydState,
        detail: Option<&str>,
        pid: Option<u32>,
    ) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO status (id, polled_at, state, detail, chronyd_pid)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET polled_at = excluded.polled_at,
                 state = excluded.state, detail = excluded.detail,
                 chronyd_pid = excluded.chronyd_pid",
            params![to_stored(at)?, state.name(), detail, pid],
        )?;
        Ok(())
    }

    /// The ID of the source at `source`, recorded if new, its name and
    /// last sighting brought up to date.
    pub fn source(
        &self,
        at: Timestamp,
        source: Address,
        reference_id: ReferenceId,
        mode: SourceMode,
        name: Option<&str>,
    ) -> Result<i64> {
        let at = to_stored(at)?;
        Ok(self.transaction.query_row(
            "INSERT INTO source (address, reference_id, mode, name, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (address) DO UPDATE SET reference_id = excluded.reference_id,
                 mode = excluded.mode, name = COALESCE(excluded.name, source.name),
                 last_seen = excluded.last_seen
             RETURNING id",
            params![address(source), reference_id.0, text::mode(mode), name, at],
            |row| row.get(0),
        )?)
    }

    /// A clock update.  A repeat of one already recorded, by reference
    /// time, is ignored.
    pub fn tracking(&self, at: Timestamp, t: &Tracking) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO tracking (at, reference_time, reference_id, reference_address,
                 stratum, leap, current_correction, last_offset, rms_offset, frequency,
                 residual_frequency, skew, root_delay, root_dispersion, last_update_interval)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT (reference_time) DO NOTHING",
            params![
                to_stored(at)?,
                to_stored(t.reference_time)?,
                t.reference_id.0,
                address(t.address),
                t.stratum,
                text::leap(t.leap),
                t.current_correction.get(),
                t.last_offset.get(),
                t.rms_offset.get(),
                t.frequency.get(),
                t.residual_frequency.get(),
                t.skew.get(),
                t.root_delay.get(),
                t.root_dispersion.get(),
                t.last_update_interval.get(),
            ],
        )?;
        Ok(())
    }

    /// A source's new sample, from `sources`.  A source with no sample
    /// yet has none to record.
    pub fn source_sample(&self, at: Timestamp, source_id: i64, s: &SourceData) -> Result<()> {
        let Some(age) = s.since_sample else {
            return Ok(());
        };
        let sample_time = at.checked_sub(age).map_err(|_| Error::Timestamp)?;
        self.transaction.execute(
            "INSERT INTO source_sample (at, source_id, sample_time, poll, stratum, state,
                 reachability, latest_measurement, original_latest_measurement,
                 latest_measurement_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                to_stored(at)?,
                source_id,
                to_stored(sample_time)?,
                s.poll,
                s.stratum,
                text::state(s.state),
                s.reachability,
                s.latest_measurement.get(),
                s.original_latest_measurement.get(),
                s.latest_measurement_error.get(),
            ],
        )?;
        Ok(())
    }

    /// A change in a source's regression, from `sourcestats`.
    pub fn source_stats(&self, at: Timestamp, source_id: i64, s: &SourceStats) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO source_stats (at, source_id, samples, runs, span, standard_deviation,
                 residual_frequency, skew, estimated_offset, estimated_offset_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                to_stored(at)?,
                source_id,
                s.samples,
                s.runs,
                i64::try_from(s.span.as_secs()).map_err(|_| Error::Timestamp)?,
                s.standard_deviation.get(),
                s.residual_frequency.get(),
                s.skew.get(),
                s.estimated_offset.get(),
                s.estimated_offset_error.get(),
            ],
        )?;
        Ok(())
    }

    /// An NTP source's new valid packet, from `ntpdata`.
    pub fn ntp_sample(&self, at: Timestamp, source_id: i64, n: &NtpData) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO ntp_sample (at, source_id, offset, peer_delay, peer_dispersion,
                 response_time, jitter_asymmetry, root_delay, root_dispersion, stratum, poll,
                 precision, leap, reference_id, reference_time, tests, interleaved,
                 authenticated, total_transmitted, total_received, total_valid, total_good)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                to_stored(at)?,
                source_id,
                n.offset.get(),
                n.peer_delay.get(),
                n.peer_dispersion.get(),
                n.response_time.get(),
                n.jitter_asymmetry,
                n.root_delay.get(),
                n.root_dispersion.get(),
                n.stratum,
                n.poll,
                n.precision,
                n.leap,
                n.reference_id.0,
                to_stored(n.reference_time)?,
                n.tests,
                n.interleaved,
                n.authenticated,
                n.total_transmitted,
                n.total_received,
                n.total_valid,
                n.total_good,
            ],
        )?;
        Ok(())
    }

    /// A change in how selection treats a source, from `selectdata`.
    pub fn selection(&self, at: Timestamp, source_id: i64, s: &SelectData) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO selection (at, source_id, state, authenticated, leap,
                 configured_options, effective_options, score, low_limit, high_limit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                to_stored(at)?,
                source_id,
                s.state.to_string(),
                s.authenticated,
                text::leap(s.leap),
                text::options(s.configured_options),
                text::options(s.effective_options),
                s.score,
                s.low_limit.get(),
                s.high_limit.get(),
            ],
        )?;
        Ok(())
    }

    /// Something that happened.
    pub fn event(
        &self,
        at: Timestamp,
        kind: EventKind,
        source_id: Option<i64>,
        detail: Option<&str>,
    ) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO event (at, kind, source_id, detail) VALUES (?1, ?2, ?3, ?4)",
            params![to_stored(at)?, kind.name(), source_id, detail],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_version(log: &Log) -> i64 {
        log.connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("a version")
    }

    #[test]
    fn creates_and_reopens() {
        let directory = std::env::temp_dir().join(format!("chronymon-log-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let _ = std::fs::remove_file(&path);
        assert_eq!(schema_version(&Log::open(&path).expect("created")), VERSION);
        assert_eq!(
            schema_version(&Log::open(&path).expect("reopened")),
            VERSION
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn refuses_a_newer_schema() {
        let mut log = Log::in_memory().expect("a database");
        log.connection
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                params![(VERSION + 1).to_string()],
            )
            .expect("updated");
        assert!(matches!(log.migrate(), Err(Error::NewerSchema { .. })));
    }

    #[test]
    fn a_source_is_recorded_once() {
        let mut log = Log::in_memory().expect("a database");
        let at = Timestamp::new(1_800_000_000, 0).expect("in range");
        let address = Address::Ip([192, 0, 2, 1].into());
        let (first, second) = log
            .write(|batch| {
                let first = batch.source(at, address, ReferenceId(1), SourceMode::Client, None)?;
                let second = batch.source(
                    at,
                    address,
                    ReferenceId(1),
                    SourceMode::Client,
                    Some("ntp.example"),
                )?;
                Ok((first, second))
            })
            .expect("written");
        assert_eq!(first, second);
        let name: String = log
            .connection
            .query_row("SELECT name FROM source", [], |row| row.get(0))
            .expect("a name");
        assert_eq!(name, "ntp.example");
    }

    #[test]
    fn events_are_written() {
        let mut log = Log::in_memory().expect("a database");
        let at = Timestamp::new(1_800_000_000, 0).expect("in range");
        log.write(|batch| batch.event(at, EventKind::Step, None, Some("1.5 s")))
            .expect("written");
        let kind: String = log
            .connection
            .query_row("SELECT kind FROM event", [], |row| row.get(0))
            .expect("an event");
        assert_eq!(EventKind::from_name(&kind), Some(EventKind::Step));
    }
}
