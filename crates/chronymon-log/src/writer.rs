//! Opening the database to write it, migrating it, and the rows the
//! daemon writes.

use std::path::Path;
use std::time::Duration;

use chronymon::event::EventKind;
use chronymon_control::report::Address;
use chronymon_control::report::Leap;
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
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::chrony_log::Line;
use crate::error::Error;
use crate::error::Result;
use crate::schema::INODE_KEY;
use crate::schema::LOG_FILE_ASIDE;
use crate::schema::LOG_FILE_BACK;
use crate::schema::LOG_TABLES;
use crate::schema::LOG_TABLES_ASIDE;
use crate::schema::LOG_TABLES_BACK;
use crate::schema::META;
use crate::schema::REFCLOCK_LOCKS;
use crate::schema::RefclockLock;
use crate::schema::TABLES;
use crate::schema::TRACKING_ASIDE;
use crate::schema::TRACKING_BACK;
use crate::schema::TRACKING_REPORT;
use crate::schema::VERSION;
use crate::schema::WHOLE_LINES;
use crate::text;
use crate::timestamp::to_stored;

/// How long a statement waits for another writer -- an operator's
/// `sqlite3`, say -- before failing.  WAL readers never block it.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// `tracking`'s columns, as `tracking` and `tracking_report` both have
/// them, in [`tracking_values`]' order.
const TRACKING_COLUMNS: &str = "at, reference_time, reference_id, reference_address, stratum,
    leap, current_correction, last_offset, rms_offset, frequency, residual_frequency, skew,
    root_delay, root_dispersion, last_update_interval";

/// The parameters `?1` to `?15` of [`TRACKING_COLUMNS`].
const TRACKING_PARAMETERS: &str =
    "?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15";

/// The last `tracking` row written.
const LAST_TRACKING: &str = "SELECT reference_time, reference_id, last_offset, rms_offset
    FROM tracking ORDER BY rowid DESC LIMIT 1";

/// A `tracking` report read at `at`, as [`TRACKING_COLUMNS`].
fn tracking_values(at: Timestamp, t: &Tracking) -> Result<[Value; 15]> {
    Ok([
        Value::Integer(to_stored(at)?),
        Value::Integer(to_stored(t.reference_time)?),
        Value::Integer(i64::from(t.reference_id.0)),
        Value::Text(address(t.address)),
        Value::Integer(i64::from(t.stratum)),
        Value::Text(text::leap(t.leap).to_owned()),
        Value::Real(t.current_correction.get()),
        Value::Real(t.last_offset.get()),
        Value::Real(t.rms_offset.get()),
        Value::Real(t.frequency.get()),
        Value::Real(t.residual_frequency.get()),
        Value::Real(t.skew.get()),
        Value::Real(t.root_delay.get()),
        Value::Real(t.root_dispersion.get()),
        Value::Real(t.last_update_interval.get()),
    ])
}

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

/// A file as its file system knows it, the same across renames: its
/// inode.  Not its device, whose number can change across boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    /// Its inode.
    pub inode: u64,
}

/// How far a log file was read, and how to tell it is still the file
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    /// Its length then.
    pub length: u64,
    /// The bytes read, up to the last whole line.
    pub read: u64,
    /// The bytes just before `read`: gone or changed, the file is
    /// another.
    pub fingerprint: Vec<u8>,
    /// Whether its header was refused, so none of it is read.
    pub rejected: bool,
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
        // Schema 2 and 3's log tables, keyed by part of a line, set aside
        // for [`TABLES`] to make them anew, then copied in.
        let rekey = found.is_some_and(|found| (LOG_TABLES..WHOLE_LINES).contains(&found));
        if rekey {
            transaction.execute_batch(LOG_TABLES_ASIDE)?;
        }
        // An older `tracking`, unique by reference time, likewise.
        let rebuild = found.is_some_and(|found| found < TRACKING_REPORT);
        if rebuild {
            transaction.execute_batch(TRACKING_ASIDE)?;
        }
        // Schema 4 and 5's `log_file`, by device and inode, likewise.
        // Older ones' are dropped by `rekey`, or not there.
        let inodes = found.is_some_and(|found| (WHOLE_LINES..INODE_KEY).contains(&found));
        if inodes {
            transaction.execute_batch(LOG_FILE_ASIDE)?;
        }
        transaction.execute_batch(TABLES)?;
        if inodes {
            transaction.execute_batch(LOG_FILE_BACK)?;
        }
        if rekey {
            transaction.execute_batch(LOG_TABLES_BACK)?;
        }
        if rebuild {
            transaction.execute_batch(TRACKING_BACK)?;
        }
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

/// An inode or a file length, as SQLite's signed integers hold them.
fn stored_u64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::Range(value))
}

fn loaded_u64(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Negative(value))
}

/// When a source's last sample arrived, as of `at`; `None` before its
/// first.
fn sample_time(at: Timestamp, s: &SourceData) -> Result<Option<Timestamp>> {
    s.since_sample
        .map(|age| at.checked_sub(age).map_err(|_| Error::Timestamp))
        .transpose()
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

    /// Record chronyd's refclocks locked to others, as its configuration
    /// now says, in place of those recorded before.
    pub fn refclock_locks(&self, locks: &[RefclockLock]) -> Result<()> {
        let stored: Vec<String> = locks.iter().map(|lock| lock.stored()).collect();
        self.set_meta(REFCLOCK_LOCKS, &stored.join("\n"))
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

    /// A clock update, or the daemon's first read.  A restart's first
    /// read of what the last row recorded is ignored: of its reference
    /// time, or of its update, the same last offset, RMS offset and
    /// reference, which only an update changes (`updated`, chronymond's
    /// `collect.rs`), its reference time moved by slews since
    /// (`handle_slew`, chrony 4.8 `reference.c:175`).  A read of
    /// chronyd unsynchronized is no update, nor is one of it on its
    /// local reference, which is never ignored: each run's marks when
    /// chronyd was on it.
    pub fn tracking(&self, at: Timestamp, t: &Tracking) -> Result<()> {
        let local = t.reference_id == ReferenceId::LOCAL;
        let update = t.reference_time != Timestamp::UNIX_EPOCH && !local;
        let values = tracking_values(at, t)?
            .into_iter()
            .chain([update, local].map(|b| Value::Integer(i64::from(b))));
        self.transaction.execute(
            &format!(
                "INSERT INTO tracking ({TRACKING_COLUMNS})
                 SELECT {TRACKING_PARAMETERS}
                 WHERE ?17 OR NOT EXISTS (SELECT 1 FROM ({LAST_TRACKING}) AS last
                     WHERE last.reference_time = ?2
                         OR (?16 AND last.reference_id = ?3 AND last.last_offset = ?8
                             AND last.rms_offset = ?9))"
            ),
            params_from_iter(values),
        )?;
        Ok(())
    }

    /// What `tracking` now says, in place of what it said before.
    pub fn tracking_report(&self, at: Timestamp, t: &Tracking) -> Result<()> {
        self.transaction.execute(
            &format!(
                "INSERT OR REPLACE INTO tracking_report (id, {TRACKING_COLUMNS})
                 VALUES (1, {TRACKING_PARAMETERS})"
            ),
            params_from_iter(tracking_values(at, t)?),
        )?;
        Ok(())
    }

    /// What `sources` now says of a source, in place of what it said
    /// before: its sample's columns NULL while it has none.
    pub fn source_report(&self, at: Timestamp, source_id: i64, s: &SourceData) -> Result<()> {
        let sample = s.since_sample.is_some();
        let sample_time = sample_time(at, s)?.map(to_stored).transpose()?;
        self.transaction.execute(
            "INSERT INTO source_report (source_id, at, poll, stratum, state, reachability,
                 sample_time, latest_measurement, original_latest_measurement,
                 latest_measurement_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (source_id) DO UPDATE SET at = excluded.at, poll = excluded.poll,
                 stratum = excluded.stratum, state = excluded.state,
                 reachability = excluded.reachability, sample_time = excluded.sample_time,
                 latest_measurement = excluded.latest_measurement,
                 original_latest_measurement = excluded.original_latest_measurement,
                 latest_measurement_error = excluded.latest_measurement_error",
            params![
                source_id,
                to_stored(at)?,
                s.poll,
                s.stratum,
                text::state(s.state),
                s.reachability,
                sample_time,
                sample.then(|| s.latest_measurement.get()),
                sample.then(|| s.original_latest_measurement.get()),
                sample.then(|| s.latest_measurement_error.get()),
            ],
        )?;
        Ok(())
    }

    /// A source's new sample, from `sources`.  A source with no sample
    /// yet has none to record.
    pub fn source_sample(&self, at: Timestamp, source_id: i64, s: &SourceData) -> Result<()> {
        let Some(sample_time) = sample_time(at, s)? else {
            return Ok(());
        };
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

    /// A line of one of chronyd's logs.  Whether it was new: a line
    /// already read, from a file read again, is not.
    pub fn log_line(&self, line: &Line) -> Result<bool> {
        let inserted = match line {
            Line::Tracking(t) => self.transaction.execute(
                "INSERT OR IGNORE INTO log_tracking (at, reference, stratum, frequency, skew,
                     offset, leap, combined_sources, offset_sd, remaining_correction,
                     root_delay, root_dispersion, max_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    to_stored(t.at)?,
                    t.reference,
                    t.stratum,
                    t.frequency,
                    t.skew,
                    t.offset,
                    t.leap.to_string(),
                    t.combined_sources,
                    t.offset_sd,
                    t.remaining_correction,
                    t.root_delay,
                    t.root_dispersion,
                    t.max_error,
                ],
            )?,
            Line::Measurement(m) => self.transaction.execute(
                "INSERT OR IGNORE INTO log_measurement (at, source, leap, stratum, tests,
                     local_poll, remote_poll, score, offset, peer_delay, peer_dispersion,
                     root_delay, root_dispersion, reference_id, mode, interleaved,
                     transmit_timestamp_source, receive_timestamp_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     ?16, ?17, ?18)",
                params![
                    to_stored(m.at)?,
                    m.source,
                    m.leap.to_string(),
                    m.stratum,
                    m.tests,
                    m.local_poll,
                    m.remote_poll,
                    m.score,
                    m.offset,
                    m.peer_delay,
                    m.peer_dispersion,
                    m.root_delay,
                    m.root_dispersion,
                    m.reference_id,
                    m.mode,
                    m.interleaved,
                    m.transmit_timestamp_source.to_string(),
                    m.receive_timestamp_source.to_string(),
                ],
            )?,
            Line::Statistics(s) => self.transaction.execute(
                "INSERT OR IGNORE INTO log_statistics (at, source, standard_deviation,
                     estimated_offset, offset_sd, residual_frequency, skew, stress, samples,
                     best_start, runs, asymmetry)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    to_stored(s.at)?,
                    s.source,
                    s.standard_deviation,
                    s.estimated_offset,
                    s.offset_sd,
                    s.residual_frequency,
                    s.skew,
                    s.stress,
                    s.samples,
                    s.best_start,
                    s.runs,
                    s.asymmetry,
                ],
            )?,
            Line::Refclock(c) => self.transaction.execute(
                "INSERT OR IGNORE INTO refclock_sample (at, reference, driver_polls, leap,
                     pulse, raw_offset, cooked_offset, dispersion)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    to_stored(c.at)?,
                    c.reference,
                    c.driver_polls,
                    c.leap.to_string(),
                    c.pulse,
                    c.raw_offset,
                    c.cooked_offset,
                    c.dispersion,
                ],
            )?,
            Line::Tempcomp(t) => self.transaction.execute(
                "INSERT OR IGNORE INTO tempcomp_sample (at, temperature, compensation)
                 VALUES (?1, ?2, ?3)",
                params![to_stored(t.at)?, t.temperature, t.compensation],
            )?,
        };
        Ok(inserted > 0)
    }

    /// How far the log file `id` was read.
    pub fn log_file(&self, id: FileId) -> Result<Option<LogFile>> {
        let found: Option<(i64, i64, Vec<u8>, bool)> = self
            .transaction
            .query_row(
                "SELECT length, read, fingerprint, rejected FROM log_file WHERE inode = ?1",
                params![stored_u64(id.inode)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        found
            .map(|(length, read, fingerprint, rejected)| {
                Ok(LogFile {
                    length: loaded_u64(length)?,
                    read: loaded_u64(read)?,
                    fingerprint,
                    rejected,
                })
            })
            .transpose()
    }

    /// Record how far the log file `id`, now at `path`, was read.
    pub fn set_log_file(&self, id: FileId, path: &str, file: &LogFile) -> Result<()> {
        self.transaction.execute(
            "INSERT INTO log_file (inode, path, length, read, fingerprint, rejected)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (inode) DO UPDATE SET path = excluded.path,
                 length = excluded.length, read = excluded.read,
                 fingerprint = excluded.fingerprint, rejected = excluded.rejected",
            params![
                stored_u64(id.inode)?,
                path,
                stored_u64(file.length)?,
                stored_u64(file.read)?,
                file.fingerprint,
                file.rejected,
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

    /// chronyd's reference changing from `before`'s to `now`'s.
    pub fn reference_change(&self, at: Timestamp, before: &Tracking, now: &Tracking) -> Result<()> {
        let detail = text::reference_change(before, now);
        self.event(at, EventKind::ReferenceChange, None, Some(&detail))
    }

    /// chronyd's leap status changing from `before` to `now`.
    pub fn leap_change(&self, at: Timestamp, before: Leap, now: Leap) -> Result<()> {
        let detail = text::leap_change(before, now);
        self.event(at, EventKind::LeapChange, None, Some(&detail))
    }
}

#[cfg(test)]
mod tests {
    use chronymon::units::Ppm;
    use chronymon::units::Seconds;
    use chronymon_control::report::SourceState;

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
    fn adds_the_log_tables_to_schema_1() {
        let directory =
            std::env::temp_dir().join(format!("chronymon-log-v1-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let _ = std::fs::remove_file(&path);
        {
            let log = Log::open(&path).expect("created");
            // Back to schema 1: its tables only.
            log.connection
                .execute_batch(
                    "DROP TABLE log_tracking; DROP TABLE log_measurement;
                     DROP TABLE log_statistics; DROP TABLE refclock_sample;
                     DROP TABLE tempcomp_sample; DROP TABLE log_file;
                     DROP TABLE source_report;
                     UPDATE meta SET value = '1' WHERE key = 'schema_version';",
                )
                .expect("downgraded");
        }
        let mut log = Log::open(&path).expect("migrated");
        assert_eq!(schema_version(&log), VERSION);
        let at = Timestamp::new(1_800_000_000, 0).expect("in range");
        let line = Line::Tempcomp(crate::chrony_log::TempcompLine {
            at,
            temperature: 41.5,
            compensation: 0.1,
        });
        let (first, again) = log
            .write(|batch| Ok((batch.log_line(&line)?, batch.log_line(&line)?)))
            .expect("written");
        assert!(first && !again);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn adds_the_source_report_to_schema_2() {
        let directory =
            std::env::temp_dir().join(format!("chronymon-log-v2-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let _ = std::fs::remove_file(&path);
        {
            let log = Log::open(&path).expect("created");
            log.connection
                .execute_batch(
                    "DROP TABLE source_report;
                     UPDATE meta SET value = '2' WHERE key = 'schema_version';",
                )
                .expect("downgraded");
        }
        let mut log = Log::open(&path).expect("migrated");
        assert_eq!(schema_version(&log), VERSION);
        let at = Timestamp::new(1_800_000_000, 0).expect("in range");
        let data = SourceData {
            address: Address::ReferenceId(u32::from_be_bytes(*b"PPS\0")),
            poll: 4,
            stratum: 0,
            state: SourceState::Nonselectable,
            mode: SourceMode::Refclock,
            reachability: 0,
            since_sample: None,
            original_latest_measurement: Seconds::new(0.0),
            latest_measurement: Seconds::new(0.0),
            latest_measurement_error: Seconds::new(0.0),
        };
        log.write(|batch| {
            let id = batch.source(at, data.address, ReferenceId(1), data.mode, None)?;
            batch.source_report(at, id, &data)?;
            batch.source_report(at, id, &data)
        })
        .expect("written");
        let (rows, sample_time, measurement): (i64, Option<i64>, Option<f64>) = log
            .connection
            .query_row(
                "SELECT COUNT(*), sample_time, latest_measurement FROM source_report",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("a report");
        assert_eq!((rows, sample_time, measurement), (1, None, None));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The names of the database's indexes, but SQLite's own.
    fn indexes(log: &Log) -> Vec<String> {
        let mut statement = log
            .connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'index'
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .expect("a statement");
        statement
            .query_map([], |row| row.get(0))
            .expect("indexes")
            .collect::<rusqlite::Result<_>>()
            .expect("indexes")
    }

    #[test]
    fn rebuilds_schema_4s_tracking() {
        let directory =
            std::env::temp_dir().join(format!("chronymon-log-v4-tracking-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let _ = std::fs::remove_file(&path);
        let created = indexes(&Log::open(&path).expect("created"));
        {
            let log = Log::open(&path).expect("reopened");
            // Back to schema 4: `tracking` unique by reference time, no
            // report, and its indexes.
            log.connection
                .execute_batch(
                    "DROP TABLE tracking_report;
                     DROP TABLE tracking;
                     CREATE TABLE tracking (
                         at                   INTEGER NOT NULL,
                         reference_time       INTEGER NOT NULL UNIQUE,
                         reference_id         INTEGER NOT NULL,
                         reference_address    TEXT    NOT NULL,
                         stratum              INTEGER NOT NULL,
                         leap                 TEXT    NOT NULL,
                         current_correction   REAL    NOT NULL,
                         last_offset          REAL    NOT NULL,
                         rms_offset           REAL    NOT NULL,
                         frequency            REAL    NOT NULL,
                         residual_frequency   REAL    NOT NULL,
                         skew                 REAL    NOT NULL,
                         root_delay           REAL    NOT NULL,
                         root_dispersion      REAL    NOT NULL,
                         last_update_interval REAL    NOT NULL
                     ) STRICT;
                     CREATE INDEX tracking_at ON tracking(at);
                     DROP INDEX source_sample_source_time;
                     DROP INDEX source_sample_time;
                     DROP INDEX event_source;
                     DROP INDEX refclock_sample_reference;
                     CREATE INDEX source_sample_source ON source_sample(source_id, at);
                     CREATE INDEX source_sample_at ON source_sample(at);
                     INSERT INTO tracking VALUES
                         (3, 30, 1, '192.0.2.1', 2, 'normal', 0, 0, 0, 0, 0, 0, 0, 0, 0),
                         (1, 10, 1, '192.0.2.1', 2, 'normal', 0, 0, 0, 0, 0, 0, 0, 0, 0),
                         (2, 0, 0, '', 0, 'unsynchronized', 0, 0, 0, 0, 0, 0, 0, 0, 0);
                     UPDATE meta SET value = '4' WHERE key = 'schema_version';",
                )
                .expect("downgraded");
        }
        let mut log = Log::open(&path).expect("migrated");
        assert_eq!(schema_version(&log), VERSION);
        assert_eq!(indexes(&log), created);
        // Its rows, in the order written.
        let ats: Vec<i64> = {
            let mut statement = log
                .connection
                .prepare("SELECT at FROM tracking ORDER BY rowid")
                .expect("a statement");
            statement
                .query_map([], |row| row.get(0))
                .expect("rows")
                .collect::<rusqlite::Result<_>>()
                .expect("rows")
        };
        assert_eq!(ats, [3, 1, 2]);
        // Unsynchronized a second time, after a clock update, is kept.
        let at = Timestamp::new(1_800_000_000, 0).expect("in range");
        let tracking = Tracking {
            reference_id: ReferenceId(1),
            address: Address::Ip([192, 0, 2, 1].into()),
            stratum: 2,
            leap: Leap::Normal,
            reference_time: at,
            current_correction: Seconds::new(0.0),
            last_offset: Seconds::new(0.0),
            rms_offset: Seconds::new(0.0),
            frequency: Ppm::new(0.0),
            residual_frequency: Ppm::new(0.0),
            skew: Ppm::new(0.0),
            root_delay: Seconds::new(0.0),
            root_dispersion: Seconds::new(0.0),
            last_update_interval: Seconds::new(0.0),
        };
        let unsynchronized = Tracking {
            reference_time: Timestamp::UNIX_EPOCH,
            leap: Leap::Unsynchronized,
            ..tracking
        };
        log.write(|batch| {
            batch.tracking(at, &tracking)?;
            batch.tracking(at, &unsynchronized)?;
            batch.tracking(at, &unsynchronized)
        })
        .expect("written");
        let zeros: i64 = log
            .connection
            .query_row(
                "SELECT COUNT(*) FROM tracking WHERE reference_time = 0",
                [],
                |row| row.get(0),
            )
            .expect("a count");
        assert_eq!(zeros, 2);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn keys_schema_5s_log_files_by_inode() {
        let directory =
            std::env::temp_dir().join(format!("chronymon-log-v5-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let _ = std::fs::remove_file(&path);
        let created = indexes(&Log::open(&path).expect("created"));
        {
            let log = Log::open(&path).expect("reopened");
            // Back to schema 5's `log_file`, by device and inode: inode 7
            // under a device a boot numbered anew, and 9.
            log.connection
                .execute_batch(
                    "DROP TABLE log_file;
                     CREATE TABLE log_file (device INTEGER NOT NULL, inode INTEGER NOT NULL,
                         path TEXT NOT NULL, length INTEGER NOT NULL, read INTEGER NOT NULL,
                         fingerprint BLOB NOT NULL, rejected INTEGER NOT NULL,
                         PRIMARY KEY (device, inode)) STRICT;
                     INSERT INTO log_file VALUES
                         (2049, 7, 'tracking.log.1', 100, 100, x'0a', 0),
                         (2050, 7, 'tracking.log.1', 200, 180, x'0b', 0),
                         (2050, 9, 'tracking.log', 50, 50, x'0c', 1);
                     UPDATE meta SET value = '5' WHERE key = 'schema_version';",
                )
                .expect("downgraded");
        }
        let mut log = Log::open(&path).expect("migrated");
        assert_eq!(schema_version(&log), VERSION);
        assert_eq!(indexes(&log), created);
        let found = log
            .write(|batch| {
                Ok([
                    batch.log_file(FileId { inode: 7 })?,
                    batch.log_file(FileId { inode: 9 })?,
                ])
            })
            .expect("read");
        // Of inode 7, the row written last.
        assert_eq!(
            found,
            [
                Some(LogFile {
                    length: 200,
                    read: 180,
                    fingerprint: vec![0x0b],
                    rejected: false,
                }),
                Some(LogFile {
                    length: 50,
                    read: 50,
                    fingerprint: vec![0x0c],
                    rejected: true,
                }),
            ]
        );
        let old: i64 = log
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'old_log_file'",
                [],
                |row| row.get(0),
            )
            .expect("a count");
        assert_eq!(old, 0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn rekeys_the_log_tables_of_schema_3() {
        let directory =
            std::env::temp_dir().join(format!("chronymon-log-v3-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let _ = std::fs::remove_file(&path);
        {
            let log = Log::open(&path).expect("created");
            // Back to schema 3's log tables, keyed by part of a line, and
            // its checkpoints by inode alone.
            log.connection
                .execute_batch(
                    "DROP TABLE log_tracking; DROP TABLE log_measurement;
                     DROP TABLE log_statistics; DROP TABLE refclock_sample;
                     DROP TABLE tempcomp_sample; DROP TABLE log_file;
                     CREATE TABLE log_tracking (at INTEGER NOT NULL, reference TEXT NOT NULL,
                         stratum INTEGER NOT NULL, frequency REAL NOT NULL, skew REAL NOT NULL,
                         offset REAL NOT NULL, leap TEXT NOT NULL,
                         combined_sources INTEGER NOT NULL, offset_sd REAL NOT NULL,
                         remaining_correction REAL NOT NULL, root_delay REAL NOT NULL,
                         root_dispersion REAL NOT NULL, max_error REAL NOT NULL,
                         UNIQUE (at, reference, offset, frequency)) STRICT;
                     CREATE TABLE log_measurement (at INTEGER NOT NULL, source TEXT NOT NULL,
                         leap TEXT NOT NULL, stratum INTEGER NOT NULL, tests INTEGER NOT NULL,
                         local_poll INTEGER NOT NULL, remote_poll INTEGER NOT NULL,
                         score REAL NOT NULL, offset REAL NOT NULL, peer_delay REAL NOT NULL,
                         peer_dispersion REAL NOT NULL, root_delay REAL NOT NULL,
                         root_dispersion REAL NOT NULL, reference_id INTEGER NOT NULL,
                         mode INTEGER NOT NULL, interleaved INTEGER NOT NULL,
                         transmit_timestamp_source TEXT NOT NULL,
                         receive_timestamp_source TEXT NOT NULL,
                         UNIQUE (at, source, offset, peer_delay)) STRICT;
                     CREATE INDEX log_measurement_source ON log_measurement(source, at);
                     CREATE TABLE log_statistics (at INTEGER NOT NULL, source TEXT NOT NULL,
                         standard_deviation REAL NOT NULL, estimated_offset REAL NOT NULL,
                         offset_sd REAL NOT NULL, residual_frequency REAL NOT NULL,
                         skew REAL NOT NULL, stress REAL NOT NULL, samples INTEGER NOT NULL,
                         best_start INTEGER NOT NULL, runs INTEGER NOT NULL,
                         asymmetry REAL NOT NULL,
                         UNIQUE (at, source, samples, estimated_offset)) STRICT;
                     CREATE INDEX log_statistics_source ON log_statistics(source, at);
                     CREATE TABLE refclock_sample (at INTEGER NOT NULL, reference TEXT NOT NULL,
                         driver_polls INTEGER, leap TEXT NOT NULL, pulse INTEGER,
                         raw_offset REAL, cooked_offset REAL NOT NULL,
                         dispersion REAL NOT NULL,
                         UNIQUE (at, reference, cooked_offset)) STRICT;
                     CREATE TABLE tempcomp_sample (at INTEGER NOT NULL,
                         temperature REAL NOT NULL, compensation REAL NOT NULL,
                         UNIQUE (at, temperature, compensation)) STRICT;
                     CREATE TABLE log_file (inode INTEGER PRIMARY KEY, path TEXT NOT NULL,
                         length INTEGER NOT NULL, read INTEGER NOT NULL) STRICT;
                     INSERT INTO refclock_sample VALUES (1, 'PPS', 0, 'N', 1, 1e-6, 1e-6, 1e-9);
                     INSERT INTO log_file VALUES (7, 'refclocks.log', 100, 100);
                     UPDATE meta SET value = '3' WHERE key = 'schema_version';",
                )
                .expect("downgraded");
        }
        let mut log = Log::open(&path).expect("migrated");
        assert_eq!(schema_version(&log), VERSION);
        let count = |log: &Log, sql: &str| -> i64 {
            log.connection
                .query_row(sql, [], |row| row.get(0))
                .expect("a count")
        };
        assert_eq!(count(&log, "SELECT COUNT(*) FROM refclock_sample"), 1);
        // Every file to be read again, under the new keys.
        assert_eq!(count(&log, "SELECT COUNT(*) FROM log_file"), 0);
        assert_eq!(
            count(
                &log,
                "SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'old_%'"
            ),
            0
        );
        // The filtered sample schema 3 took for the raw one.
        let filtered = Line::Refclock(crate::chrony_log::RefclockLine {
            at: Timestamp::new(0, 1).expect("in range"),
            reference: "PPS".to_owned(),
            driver_polls: None,
            leap: 'N',
            pulse: None,
            raw_offset: None,
            cooked_offset: 1e-6,
            dispersion: 1e-9,
        });
        assert!(
            log.write(|batch| batch.log_line(&filtered))
                .expect("written")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn keys_a_refclock_line_whole() {
        let mut log = Log::in_memory().expect("a database");
        let raw = crate::chrony_log::RefclockLine {
            at: Timestamp::new(1_800_000_000, 123_000).expect("in range"),
            reference: "PPS".to_owned(),
            driver_polls: Some(0),
            leap: 'N',
            pulse: Some(true),
            raw_offset: Some(1e-6),
            cooked_offset: 1e-6,
            dispersion: 1e-9,
        };
        let filtered = Line::Refclock(crate::chrony_log::RefclockLine {
            driver_polls: None,
            pulse: None,
            raw_offset: None,
            ..raw.clone()
        });
        let raw = Line::Refclock(raw);
        let written = log
            .write(|batch| {
                Ok([
                    batch.log_line(&raw)?,
                    batch.log_line(&filtered)?,
                    batch.log_line(&raw)?,
                    batch.log_line(&filtered)?,
                ])
            })
            .expect("written");
        assert_eq!(written, [true, true, false, false]);
    }

    #[test]
    fn records_where_a_log_file_was_read() {
        let mut log = Log::in_memory().expect("a database");
        let id = FileId { inode: 7 };
        let file = LogFile {
            length: 120,
            read: 100,
            fingerprint: b"line\n".to_vec(),
            rejected: true,
        };
        let found = log
            .write(|batch| {
                batch.set_log_file(id, "tracking.log", &file)?;
                batch.log_file(id)
            })
            .expect("written");
        assert_eq!(found, Some(file));
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
