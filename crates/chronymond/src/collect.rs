//! One poll of chronyd: read every report, compare it with the last
//! poll's, and write what changed, with the events the changes mean.

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;

use chronymon::event::EventKind;
use chronymon::units::Seconds;
use chronymon_control::client::Client;
use chronymon_control::error::DecodeError;
use chronymon_control::error::Error;
use chronymon_control::error::Status;
use chronymon_control::report::Address;
use chronymon_control::report::NtpData;
use chronymon_control::report::ReferenceId;
use chronymon_control::report::SelectData;
use chronymon_control::report::SourceData;
use chronymon_control::report::SourceMode;
use chronymon_control::report::SourceStats;
use chronymon_control::report::Tracking;
use chronymon_log::writer::Batch;
use chronymon_log::writer::ChronydState;
use jiff::Timestamp;

use crate::host::Process;

/// The reports a poll reads, so tests can stand in for chronyd.
pub(crate) trait Chronyd {
    fn tracking(&mut self) -> Result<Tracking, Error>;
    fn source_count(&mut self) -> Result<u32, Error>;
    fn source_data(&mut self, index: u32) -> Result<SourceData, Error>;
    fn source_stats(&mut self, index: u32) -> Result<SourceStats, Error>;
    fn select_data(&mut self, index: u32) -> Result<SelectData, Error>;
    fn ntp_data(&mut self, address: IpAddr) -> Result<NtpData, Error>;
    fn source_name(&mut self, address: IpAddr) -> Result<String, Error>;
}

impl Chronyd for Client {
    fn tracking(&mut self) -> Result<Tracking, Error> {
        Client::tracking(self)
    }
    fn source_count(&mut self) -> Result<u32, Error> {
        Client::source_count(self)
    }
    fn source_data(&mut self, index: u32) -> Result<SourceData, Error> {
        Client::source_data(self, index)
    }
    fn source_stats(&mut self, index: u32) -> Result<SourceStats, Error> {
        Client::source_stats(self, index)
    }
    fn select_data(&mut self, index: u32) -> Result<SelectData, Error> {
        Client::select_data(self, index)
    }
    fn ntp_data(&mut self, address: IpAddr) -> Result<NtpData, Error> {
        Client::ntp_data(self, address)
    }
    fn source_name(&mut self, address: IpAddr) -> Result<String, Error> {
        Client::source_name(self, address)
    }
}

/// One source as one poll saw it.
#[derive(Debug, Clone)]
pub(crate) struct Source {
    data: SourceData,
    stats: SourceStats,
    selection: SelectData,
    /// For an NTP source only.
    ntp: Option<NtpData>,
}

/// Everything one poll read.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    tracking: Tracking,
    /// `None` when the sources changed while being read, so this poll
    /// cannot say which are which; the next will.
    sources: Option<Vec<Source>>,
    /// Configured names of NTP sources not named before.
    names: HashMap<IpAddr, String>,
}

/// Every source but those whose names are not resolved yet: their
/// placeholders are replaced by the addresses the names resolve to,
/// which are then sources of their own, and chronyc leaves them out
/// too (`client.c`, chrony 4.3 to 4.9).
fn read_sources(chronyd: &mut impl Chronyd) -> Result<Vec<Source>, Error> {
    (0..chronyd.source_count()?)
        .filter_map(|index| {
            let data = match chronyd.source_data(index) {
                Ok(data) => data,
                Err(e) => return Some(Err(e)),
            };
            if let Address::Unresolved(_) = data.address {
                return None;
            }
            Some(read_source(chronyd, index, data))
        })
        .collect()
}

/// The rest of one source's reports, after `sources`.
fn read_source(chronyd: &mut impl Chronyd, index: u32, data: SourceData) -> Result<Source, Error> {
    let ntp = match (data.mode, data.address) {
        (SourceMode::Client | SourceMode::Peer, Address::Ip(address)) => {
            Some(chronyd.ntp_data(address)?)
        }
        _ => None,
    };
    Ok(Source {
        data,
        stats: chronyd.source_stats(index)?,
        selection: chronyd.select_data(index)?,
        ntp,
    })
}

/// What an error reading chronyd means for the status row.
pub(crate) fn state_of(error: &Error) -> ChronydState {
    match error {
        Error::Decode(DecodeError::UnsupportedReply { .. } | DecodeError::UnknownValue { .. }) => {
            ChronydState::Unsupported
        }
        _ => ChronydState::Down,
    }
}

/// What the last poll recorded of one source, to tell what is new.
#[derive(Debug, Clone)]
struct Seen {
    id: i64,
    data: SourceData,
    stats: SourceStats,
    selection: SelectData,
    ntp_valid: Option<u32>,
}

/// The poll-to-poll state: what was last written, to write only what
/// changed.
#[derive(Debug)]
pub(crate) struct Collector {
    step_threshold: Seconds,
    /// The last poll's state of chronyd; `None` before the first poll.
    state: Option<ChronydState>,
    /// chronyd's PID as last seen running.
    pid: Option<u32>,
    tracking: Option<Tracking>,
    /// By address.
    sources: HashMap<Address, Seen>,
    /// Whether the sources have been read once: those found the first
    /// time were there before, not added.
    sources_read: bool,
    /// Configured names, asked once per source.
    names: HashMap<IpAddr, String>,
}

impl Collector {
    pub(crate) fn new(step_threshold: Seconds) -> Self {
        Self {
            step_threshold,
            state: None,
            pid: None,
            tracking: None,
            sources: HashMap::new(),
            sources_read: false,
            names: HashMap::new(),
        }
    }

    /// Read every report, and the names of sources not named before.
    pub(crate) fn read(&self, chronyd: &mut impl Chronyd) -> Result<Snapshot, Error> {
        let tracking = chronyd.tracking()?;
        let sources = match read_sources(chronyd) {
            Ok(sources) => Some(sources),
            // A source added or removed between the count and the index.
            Err(Error::Status(Status::NoSuchSource)) => None,
            Err(e) => return Err(e),
        };
        let mut names = HashMap::new();
        for source in sources.iter().flatten() {
            if let Address::Ip(ip) = source.data.address
                && !self.names.contains_key(&ip)
                // A name chronyd cannot give is not worth failing the
                // poll for; it is asked again next time.
                && let Ok(name) = chronyd.source_name(ip)
            {
                names.insert(ip, name);
            }
        }
        Ok(Snapshot {
            tracking,
            sources,
            names,
        })
    }

    /// Record one poll at `at`: `read` is what reading chronyd gave,
    /// `process` what its PID file says.
    pub(crate) fn record(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
        read: Result<Snapshot, Error>,
        process: Process,
    ) -> chronymon_log::error::Result<()> {
        let pid = match process {
            Process::Running(pid) => Some(pid),
            _ => None,
        };
        match read {
            Ok(snapshot) => {
                if self.state.is_some_and(|state| state != ChronydState::Up) {
                    batch.event(at, EventKind::ChronydUp, None, None)?;
                }
                if let (Some(before), Some(now)) = (self.pid, pid)
                    && before != now
                {
                    let detail = format!("pid {before} to {now}");
                    batch.event(at, EventKind::ChronydRestart, None, Some(&detail))?;
                }
                self.state = Some(ChronydState::Up);
                self.pid = pid.or(self.pid);
                batch.status(at, ChronydState::Up, None, pid)?;
                self.record_tracking(batch, at, &snapshot.tracking)?;
                self.names.extend(snapshot.names);
                if let Some(sources) = snapshot.sources {
                    self.record_sources(batch, at, &sources)?;
                }
            }
            Err(error) => {
                let state = state_of(&error);
                let detail = error.to_string();
                if self.state != Some(state) {
                    let kind = match (state, process) {
                        (ChronydState::Unsupported, _) => EventKind::Unsupported,
                        (_, Process::Dead(_)) => EventKind::ChronydCrash,
                        _ => EventKind::ChronydDown,
                    };
                    batch.event(at, kind, None, Some(&detail))?;
                }
                self.state = Some(state);
                batch.status(at, state, Some(&detail), pid)?;
            }
        }
        Ok(())
    }

    fn record_tracking(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
        tracking: &Tracking,
    ) -> chronymon_log::error::Result<()> {
        let before = self.tracking.replace(*tracking);
        if before.is_some_and(|before| before.reference_time == tracking.reference_time) {
            return Ok(());
        }
        batch.tracking(at, tracking)?;
        let Some(before) = before else {
            return Ok(());
        };
        if tracking.last_offset.get().abs() > self.step_threshold.get() {
            let detail = format!("{} s", tracking.last_offset.get());
            batch.event(at, EventKind::Step, None, Some(&detail))?;
        }
        if before.reference_id != tracking.reference_id {
            let detail = format!("{} to {}", before.address, tracking.address);
            batch.event(at, EventKind::ReferenceChange, None, Some(&detail))?;
        }
        if before.leap != tracking.leap {
            let detail = format!("{:?} to {:?}", before.leap, tracking.leap);
            batch.event(at, EventKind::LeapChange, None, Some(&detail))?;
        }
        Ok(())
    }

    fn record_sources(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
        sources: &[Source],
    ) -> chronymon_log::error::Result<()> {
        let first_poll = !self.sources_read;
        self.sources_read = true;
        let present: HashSet<Address> = sources.iter().map(|s| s.data.address).collect();
        for source in sources {
            let address = source.data.address;
            let name = self.name(address);
            let id = batch.source(
                at,
                address,
                source.stats.reference_id,
                source.data.mode,
                name.as_deref(),
            )?;
            let seen = self.sources.get(&address);
            if seen.is_none() && !first_poll {
                batch.event(at, EventKind::SourceAdded, Some(id), name.as_deref())?;
            }
            if seen.is_none_or(|seen| report_changed(&seen.data, &source.data)) {
                batch.source_report(at, id, &source.data)?;
            }
            if seen.is_none_or(|seen| new_sample(&seen.data, &source.data)) {
                batch.source_sample(at, id, &source.data)?;
            }
            if seen.is_none_or(|seen| seen.stats != source.stats) {
                batch.source_stats(at, id, &source.stats)?;
            }
            if seen.is_none_or(|seen| selection_changed(&seen.selection, &source.selection)) {
                batch.selection(at, id, &source.selection)?;
            }
            let ntp_valid = source.ntp.map(|ntp| ntp.total_valid);
            if let Some(ntp) = &source.ntp
                && ntp.total_valid > 0
                && seen.is_none_or(|seen| seen.ntp_valid != ntp_valid)
            {
                batch.ntp_sample(at, id, ntp)?;
            }
            self.sources.insert(
                address,
                Seen {
                    id,
                    data: source.data,
                    stats: source.stats,
                    selection: source.selection,
                    ntp_valid,
                },
            );
        }
        let gone: Vec<Address> = self
            .sources
            .keys()
            .filter(|address| !present.contains(address))
            .copied()
            .collect();
        for address in gone {
            if let Some(seen) = self.sources.remove(&address) {
                let detail = address.to_string();
                batch.event(at, EventKind::SourceRemoved, Some(seen.id), Some(&detail))?;
            }
        }
        Ok(())
    }

    /// A source's configured name; a refclock's reference ID as text.
    fn name(&self, address: Address) -> Option<String> {
        match address {
            Address::Ip(ip) => self.names.get(&ip).cloned(),
            Address::ReferenceId(id) => ReferenceId(id).text(),
            Address::Unresolved(_) | Address::Unspecified => None,
        }
    }
}

/// Whether selection treats a source differently: anything but the
/// age of its last sample, which grows every second.
fn selection_changed(before: &SelectData, now: &SelectData) -> bool {
    SelectData {
        last_sample_ago: now.last_sample_ago,
        ..*before
    } != *now
}

/// Whether `sources` says anything new of a source: anything but the
/// age of its last sample growing.
fn report_changed(before: &SourceData, now: &SourceData) -> bool {
    let since_sample = match (before.since_sample, now.since_sample) {
        (Some(_), Some(age)) => Some(age),
        (then, _) => then,
    };
    new_sample(before, now)
        || SourceData {
            since_sample,
            ..*before
        } != *now
}

/// Whether `now` reports a sample `before` did not: the measurement
/// changed, or the sample's age went down.
fn new_sample(before: &SourceData, now: &SourceData) -> bool {
    match (before.since_sample, now.since_sample) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(then), Some(age)) => {
            age < then
                || before.original_latest_measurement != now.original_latest_measurement
                || before.latest_measurement_error != now.latest_measurement_error
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use chronymon::units::Ppm;
    use chronymon_control::report::Leap;
    use chronymon_control::report::SelectOptions;
    use chronymon_control::report::SourceState;
    use chronymon_log::timestamp::to_stored;
    use chronymon_log::writer::Log;
    use rusqlite::Connection;

    use super::*;

    /// A database in a fresh temporary file, and a second connection to
    /// read what was written.
    pub(crate) struct Database {
        directory: PathBuf,
        pub(crate) log: Log,
        pub(crate) reader: Connection,
    }

    impl Database {
        pub(crate) fn new(name: &str) -> Self {
            let directory =
                std::env::temp_dir().join(format!("chronymond-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a directory");
            let path = directory.join("test.sqlite");
            let log = Log::open(&path).expect("a database");
            let reader = Connection::open(&path).expect("a reader");
            Self {
                directory,
                log,
                reader,
            }
        }

        /// A directory for the test's own files beside the database.
        pub(crate) fn directory(&self) -> &std::path::Path {
            &self.directory
        }

        pub(crate) fn count(&self, table: &str) -> i64 {
            self.reader
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("a count")
        }

        pub(crate) fn events(&self) -> Vec<String> {
            let mut statement = self
                .reader
                .prepare("SELECT kind FROM event ORDER BY rowid")
                .expect("a query");
            statement
                .query_map([], |row| row.get(0))
                .expect("rows")
                .map(|kind| kind.expect("a kind"))
                .collect()
        }
    }

    impl Drop for Database {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::new(1_800_000_000 + second, 0).expect("in range")
    }

    const SERVER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));

    fn tracking(reference_second: i64, last_offset: f64) -> Tracking {
        Tracking {
            reference_id: ReferenceId(0xc000_0201),
            address: Address::Ip(SERVER),
            stratum: 3,
            leap: Leap::Normal,
            reference_time: at(reference_second),
            current_correction: Seconds::new(0.0),
            last_offset: Seconds::new(last_offset),
            rms_offset: Seconds::new(1e-6),
            frequency: Ppm::new(-12.5),
            residual_frequency: Ppm::new(0.001),
            skew: Ppm::new(0.01),
            root_delay: Seconds::new(0.01),
            root_dispersion: Seconds::new(0.001),
            last_update_interval: Seconds::new(64.0),
        }
    }

    fn source(age: u64, total_valid: u32) -> Source {
        let data = SourceData {
            address: Address::Ip(SERVER),
            poll: 6,
            stratum: 2,
            state: SourceState::Selected,
            mode: SourceMode::Client,
            reachability: 0o377,
            since_sample: Some(Duration::from_secs(age)),
            original_latest_measurement: Seconds::new(f64::from(total_valid) * 1e-6),
            latest_measurement: Seconds::new(1e-6),
            latest_measurement_error: Seconds::new(1e-5),
        };
        let stats = SourceStats {
            reference_id: ReferenceId(0xc000_0201),
            address: data.address,
            samples: total_valid,
            runs: 3,
            span: Duration::from_secs(600),
            standard_deviation: Seconds::new(1e-6),
            residual_frequency: Ppm::new(0.0),
            skew: Ppm::new(0.01),
            estimated_offset: Seconds::new(0.0),
            estimated_offset_error: Seconds::new(1e-6),
        };
        let selection = SelectData {
            reference_id: stats.reference_id,
            address: data.address,
            state: '*',
            authenticated: false,
            leap: Leap::Normal,
            configured_options: SelectOptions::default(),
            effective_options: SelectOptions::default(),
            last_sample_ago: Duration::from_secs(age),
            score: 1.0,
            low_limit: Seconds::new(-1e-5),
            high_limit: Seconds::new(1e-5),
        };
        let ntp = NtpData {
            remote_address: data.address,
            local_address: Address::Unspecified,
            remote_port: 123,
            leap: 0,
            version: 4,
            mode: 4,
            stratum: 2,
            poll: 6,
            precision: -20,
            root_delay: Seconds::new(0.01),
            root_dispersion: Seconds::new(0.001),
            reference_id: ReferenceId(1),
            reference_time: at(0),
            offset: Seconds::new(1e-6),
            peer_delay: Seconds::new(1e-3),
            peer_dispersion: Seconds::new(1e-6),
            response_time: Seconds::new(1e-5),
            jitter_asymmetry: 0.0,
            tests: 0x3ff,
            interleaved: false,
            authenticated: false,
            transmit_timestamp_source: 'K',
            receive_timestamp_source: 'K',
            total_transmitted: total_valid,
            total_received: total_valid,
            total_valid,
            total_good: total_valid,
            timestamps: None,
        };
        Source {
            data,
            stats,
            selection,
            ntp: Some(ntp),
        }
    }

    fn snapshot(tracking: Tracking, sources: Vec<Source>) -> Result<Snapshot, Error> {
        Ok(Snapshot {
            tracking,
            sources: Some(sources),
            names: HashMap::from([(SERVER, "ntp.example".to_owned())]),
        })
    }

    fn down() -> Result<Snapshot, Error> {
        Err(Error::Timeout {
            attempts: 3,
            timeout: Duration::from_secs(1),
        })
    }

    #[test]
    fn writes_only_what_changed() {
        let mut db = Database::new("changes");
        let mut collector = Collector::new(Seconds::new(1.0));
        let running = Process::Running(100);
        for (second, read) in [
            (0, snapshot(tracking(0, 1e-6), vec![source(5, 10)])),
            // Nothing new a second later but the sample's age.
            (1, snapshot(tracking(0, 1e-6), vec![source(6, 10)])),
            // A new clock update and a new sample.
            (2, snapshot(tracking(2, 2e-6), vec![source(0, 11)])),
        ] {
            db.log
                .write(|batch| collector.record(batch, at(second), read, running))
                .expect("recorded");
        }
        assert_eq!(db.count("tracking"), 2);
        assert_eq!(db.count("source"), 1);
        assert_eq!(db.count("source_sample"), 2);
        assert_eq!(db.count("source_stats"), 2);
        assert_eq!(db.count("ntp_sample"), 2);
        assert_eq!(db.count("selection"), 1);
        assert!(db.events().is_empty(), "{:?}", db.events());
        let name: String = db
            .reader
            .query_row("SELECT name FROM source", [], |row| row.get(0))
            .expect("a name");
        assert_eq!(name, "ntp.example");
    }

    #[test]
    fn derives_events() {
        let mut db = Database::new("events");
        let mut collector = Collector::new(Seconds::new(1.0));
        let polls = [
            (snapshot(tracking(0, 0.0), vec![]), Process::Running(100)),
            // A source appears.
            (
                snapshot(tracking(0, 0.0), vec![source(1, 1)]),
                Process::Running(100),
            ),
            // The clock steps.
            (
                snapshot(tracking(2, 1.5), vec![source(1, 1)]),
                Process::Running(100),
            ),
            // chronyd goes, twice over, leaving its PID file.
            (down(), Process::Dead(100)),
            (down(), Process::Dead(100)),
            // and comes back as another process, without the source.
            (snapshot(tracking(5, 0.0), vec![]), Process::Running(200)),
        ];
        for (second, (read, process)) in (0..).zip(polls) {
            db.log
                .write(|batch| collector.record(batch, at(second), read, process))
                .expect("recorded");
        }
        assert_eq!(
            db.events(),
            [
                "source_added",
                "step",
                "chronyd_crash",
                "chronyd_up",
                "chronyd_restart",
                "source_removed"
            ]
        );
        let state: String = db
            .reader
            .query_row("SELECT state FROM status", [], |row| row.get(0))
            .expect("a status");
        assert_eq!(state, "up");
    }

    /// chronyd answering with `sources`, by index.
    struct Fake {
        sources: Vec<Source>,
    }

    impl Fake {
        fn source(&self, index: u32) -> Result<&Source, Error> {
            let index = usize::try_from(index).expect("an index");
            self.sources
                .get(index)
                .ok_or(Error::Status(Status::NoSuchSource))
        }
    }

    impl Chronyd for Fake {
        fn tracking(&mut self) -> Result<Tracking, Error> {
            Ok(tracking(0, 0.0))
        }
        fn source_count(&mut self) -> Result<u32, Error> {
            Ok(u32::try_from(self.sources.len()).expect("a count"))
        }
        fn source_data(&mut self, index: u32) -> Result<SourceData, Error> {
            Ok(self.source(index)?.data)
        }
        fn source_stats(&mut self, index: u32) -> Result<SourceStats, Error> {
            let source = self.source(index)?;
            assert!(!matches!(source.data.address, Address::Unresolved(_)));
            Ok(source.stats)
        }
        fn select_data(&mut self, index: u32) -> Result<SelectData, Error> {
            let source = self.source(index)?;
            assert!(!matches!(source.data.address, Address::Unresolved(_)));
            Ok(source.selection)
        }
        fn ntp_data(&mut self, address: IpAddr) -> Result<NtpData, Error> {
            self.sources
                .iter()
                .find(|s| s.data.address == Address::Ip(address))
                .and_then(|s| s.ntp)
                .ok_or(Error::Status(Status::NoSuchSource))
        }
        fn source_name(&mut self, address: IpAddr) -> Result<String, Error> {
            assert_eq!(address, SERVER);
            Ok("ntp.example".to_owned())
        }
    }

    #[test]
    fn leaves_out_unresolved_sources() {
        let mut placeholder = source(0, 0);
        placeholder.data.address = Address::Unresolved(1);
        placeholder.data.since_sample = None;
        placeholder.ntp = None;
        let mut fake = Fake {
            sources: vec![placeholder, source(5, 10)],
        };
        let snapshot = Collector::new(Seconds::new(1.0))
            .read(&mut fake)
            .expect("a snapshot");
        let sources = snapshot.sources.expect("sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].data.address, Address::Ip(SERVER));
        assert_eq!(snapshot.names.len(), 1);
    }

    #[test]
    fn reports_reach_lost_and_samples_gone() {
        let mut db = Database::new("report");
        let mut collector = Collector::new(Seconds::new(1.0));
        let unreached = |age, reachability| {
            let mut s = source(age, 10);
            s.data.reachability = reachability;
            s
        };
        let mut gone = unreached(0, 0);
        gone.data.since_sample = None;
        gone.data.state = SourceState::Nonselectable;
        gone.data.stratum = 0;
        for (second, source) in [
            (0, source(5, 10)),
            // Older, and nothing else: no report.
            (1, source(6, 10)),
            // A poll unanswered.
            (2, unreached(7, 0o376)),
            // chronyd restarted, with no sample yet.
            (3, gone),
        ] {
            let mut fake = Fake {
                sources: vec![source],
            };
            let read = collector.read(&mut fake);
            db.log
                .write(|batch| collector.record(batch, at(second), read, Process::Running(100)))
                .expect("recorded");
        }
        assert_eq!(db.count("source_sample"), 1);
        assert_eq!(db.count("source_report"), 1);
        let report: (i64, String, i64, Option<i64>, Option<f64>) = db
            .reader
            .query_row(
                "SELECT at, state, reachability, sample_time, latest_measurement
                 FROM source_report",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("a report");
        let stored = to_stored(at(3)).expect("in range");
        assert_eq!(report, (stored, "nonselectable".to_owned(), 0, None, None));
    }

    #[test]
    fn a_report_waits_for_a_change() {
        let mut db = Database::new("report-age");
        let mut collector = Collector::new(Seconds::new(1.0));
        for (second, age) in [(0, 5), (1, 6)] {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(tracking(0, 0.0), vec![source(age, 10)]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
        }
        let written: i64 = db
            .reader
            .query_row("SELECT at FROM source_report", [], |row| row.get(0))
            .expect("a report");
        assert_eq!(written, to_stored(at(0)).expect("in range"));
    }
}
