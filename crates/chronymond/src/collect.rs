//! One poll of chronyd: read every report, compare it with the last
//! poll's, and write what changed, with the events the changes mean.

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;

use chronymon::event::EventKind;
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

use crate::config::Config;
use crate::config::ConfigSource;
use crate::config::DEFAULT_LARGE_OFFSET;
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
///
/// `None` when the sources changed while being read.  Each report is
/// asked for by index, and chronyd renumbers the sources after one it
/// removes (`SRC_DestroyInstance`, `sources.c`, 4.3 to 4.9), so one
/// source's reports could be filed under another's address: each
/// report's address is checked against `sources`'.  A source moved
/// past leaves the last index empty: `NoSuchSource`.
fn read_sources(chronyd: &mut impl Chronyd) -> Result<Option<Vec<Source>>, Error> {
    let mut sources = Vec::new();
    for index in 0..chronyd.source_count()? {
        let source = match chronyd.source_data(index) {
            Ok(data) if matches!(data.address, Address::Unresolved(_)) => continue,
            Ok(data) => read_source(chronyd, index, data),
            Err(e) => Err(e),
        };
        match source {
            Ok(Some(source)) => sources.push(source),
            Ok(None) | Err(Error::Status(Status::NoSuchSource)) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(Some(sources))
}

/// The rest of one source's reports, after `sources`; `None` when one
/// is of another source.
fn read_source(
    chronyd: &mut impl Chronyd,
    index: u32,
    data: SourceData,
) -> Result<Option<Source>, Error> {
    let ntp = match (data.mode, data.address) {
        (SourceMode::Client | SourceMode::Peer, Address::Ip(address)) => {
            Some(chronyd.ntp_data(address)?)
        }
        _ => None,
    };
    let stats = chronyd.source_stats(index)?;
    let selection = chronyd.select_data(index)?;
    if stats.address != data.address
        || selection.address != data.address
        || ntp.is_some_and(|ntp| ntp.remote_address != data.address)
    {
        return Ok(None);
    }
    Ok(Some(Source {
        data,
        stats,
        selection,
        ntp,
    }))
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
    /// Where chronyd's configuration is read from, and what it said when
    /// last read successfully: empty before.
    chrony_config: ConfigSource,
    config: Config,
    /// chronyd's clock updates since it started, as far as seen; `None`
    /// when it was running before they could be counted.
    updates: Option<u32>,
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
    /// The offset of a step `makestep`'s rule said this poll, until
    /// taken.
    step: Option<f64>,
}

impl Collector {
    /// A collector going by chronyd's configuration from
    /// `chrony_config`.  One that cannot be read is said so, and read
    /// again when chronyd is first seen.
    pub(crate) fn new(chrony_config: ConfigSource) -> Self {
        let config = match &chrony_config {
            ConfigSource::File(path) => Config::read(path).unwrap_or_else(|e| {
                eprintln!("chronymond: chronyd's configuration: {e}");
                Config::default()
            }),
            ConfigSource::None => Config::default(),
        };
        Self {
            config,
            chrony_config,
            updates: None,
            state: None,
            pid: None,
            tracking: None,
            sources: HashMap::new(),
            sources_read: false,
            names: HashMap::new(),
            step: None,
        }
    }

    /// chronyd's configuration, as last read.
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Read chronyd's configuration again, as chronyd does on starting,
    /// and record its refclocks' locks.  One that cannot be read leaves
    /// what was read last, and the locks recorded, as they were: a
    /// configuration taken for empty would unpair all history.
    fn reread_config(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
    ) -> chronymon_log::error::Result<()> {
        let ConfigSource::File(path) = &self.chrony_config else {
            return Ok(());
        };
        match Config::read(path) {
            Ok(config) => {
                if config != self.config {
                    eprintln!("chronymond: chronyd's configuration now {config}");
                }
                self.config = config;
                batch.refclock_locks(&self.config.locks())?;
                batch.tempcomp(at, self.config.tempcomp.as_ref())?;
                if batch.chrony_config_failed()? {
                    eprintln!("chronymond: chronyd's configuration read again");
                    batch.event(at, EventKind::ChronyConfigRead, None, None)?;
                }
                batch.chrony_config_error(None)
            }
            Err(e) => {
                let detail = e.to_string();
                eprintln!(
                    "chronymond: chronyd's configuration: {detail}: keeping {}",
                    self.config
                );
                batch.event(at, EventKind::ChronyConfigError, None, Some(&detail))?;
                batch.chrony_config_error(Some(&detail))
            }
        }
    }

    /// Read every report, and the names of sources not named before.
    pub(crate) fn read(&self, chronyd: &mut impl Chronyd) -> Result<Snapshot, Error> {
        let tracking = chronyd.tracking()?;
        let sources = read_sources(chronyd)?;
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
                // chronyd first seen, or perhaps started again: back, or
                // another process.  Its configuration may have changed.
                let mut started = self.state.is_none();
                if self.state.is_some_and(|state| state != ChronydState::Up) {
                    batch.event(at, EventKind::ChronydUp, None, None)?;
                    // It may have restarted, and updated, unseen.
                    self.updates = None;
                    started = true;
                }
                if let (Some(before), Some(now)) = (self.pid, pid)
                    && before != now
                {
                    let detail = format!("pid {before} to {now}");
                    batch.event(at, EventKind::ChronydRestart, None, Some(&detail))?;
                    self.updates = None;
                    started = true;
                }
                if started {
                    self.reread_config(batch, at)?;
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
        if before != Some(*tracking) {
            batch.tracking_report(at, tracking)?;
        }
        let update = !never_updated(tracking) && before.is_some_and(|b| updated(&b, tracking));
        if never_updated(tracking) {
            self.updates = Some(0);
        } else if update {
            self.updates = self.updates.map(|n| n.saturating_add(1));
            self.record_correction(batch, at, tracking)?;
        }
        // History of updates, and of the first read: chronyd's last
        // update, which the writer leaves out when the last row's, or
        // that it is unsynchronized or on its local reference.
        let Some(before) = before else {
            return batch.tracking(at, tracking);
        };
        if update {
            batch.tracking(at, tracking)?;
        }
        if before.reference_id != tracking.reference_id {
            batch.reference_change(at, &before, tracking)?;
        }
        if before.leap != tracking.leap {
            batch.leap_change(at, before.leap, tracking.leap)?;
        }
        Ok(())
    }

    /// A step or a large offset, for an update's correction past the
    /// threshold: a step only where `makestep` allows one.
    fn record_correction(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
        tracking: &Tracking,
    ) -> chronymon_log::error::Result<()> {
        let threshold = self
            .config
            .make_step
            .map_or(DEFAULT_LARGE_OFFSET, |make_step| make_step.threshold);
        if tracking.last_offset.get().abs() <= threshold.get() {
            return Ok(());
        }
        let kind = if self
            .config
            .make_step
            .is_some_and(|make_step| make_step.steps_on(self.updates))
        {
            EventKind::Step
        } else {
            EventKind::LargeOffset
        };
        if kind == EventKind::Step {
            self.step = Some(tracking.last_offset.get());
        }
        let detail = format!("{} s", tracking.last_offset.get());
        batch.event(at, kind, None, Some(&detail))
    }

    /// The offset of a step `makestep`'s rule said since last taken.
    pub(crate) const fn take_step(&mut self) -> Option<f64> {
        self.step.take()
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
            if seen.is_none_or(|seen| regression_changed(&seen.stats, &source.stats)) {
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

/// Whether chronyd has not updated the clock since it started: its last
/// offset and the RMS of its offsets are both still 0, which every
/// update changes (`REF_SetReference`, `reference.c`, 4.3 to 4.9).
fn never_updated(tracking: &Tracking) -> bool {
    tracking.last_offset.get() == 0.0 && tracking.rms_offset.get() == 0.0
}

/// Whether chronyd updated the clock between two reports.  Not the
/// reference time, which also changes when chronyd loses or regains
/// sync, or serves its local reference, and which it moves on every
/// slew or frequency change, tempcomp's included (`handle_slew`, chrony
/// 4.8 `reference.c:175`).
fn updated(before: &Tracking, now: &Tracking) -> bool {
    before.last_offset != now.last_offset || before.rms_offset != now.rms_offset
}

/// Whether selection treats a source differently: anything but the
/// age of its last sample, which grows every second.
fn selection_changed(before: &SelectData, now: &SelectData) -> bool {
    SelectData {
        last_sample_ago: now.last_sample_ago,
        ..*before
    } != *now
}

/// Whether chronyd's regression of a source changed: anything but the
/// estimated offset and its error, which chronyd extrapolates to the
/// time of the report (`SST_DoSourcestatsReport`, chrony 4.8
/// `sourcestats.c:1019`), so that they change every poll.
fn regression_changed(before: &SourceStats, now: &SourceStats) -> bool {
    SourceStats {
        estimated_offset: now.estimated_offset,
        estimated_offset_error: now.estimated_offset_error,
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
    use chronymon::units::Seconds;
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

    /// Debian's and Ubuntu's `makestep`.
    const MAKE_STEP: &str = "makestep 1 3\n";

    /// A collector going by `config`, chronyd's configuration, written
    /// beside `db`'s database.
    fn collector(db: &Database, config: &str) -> Collector {
        let path = db.directory().join("chrony.conf");
        std::fs::write(&path, config).expect("written");
        Collector::new(ConfigSource::File(path))
    }

    /// A collector with no configuration to read.
    fn unconfigured() -> Collector {
        Collector::new(ConfigSource::None)
    }

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

    /// chronyd's tracking before its first clock update.
    fn fresh() -> Tracking {
        Tracking {
            reference_time: Timestamp::UNIX_EPOCH,
            last_offset: Seconds::new(0.0),
            rms_offset: Seconds::new(0.0),
            ..tracking(0, 0.0)
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
        let mut collector = collector(&db, MAKE_STEP);
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
        let mut collector = collector(&db, MAKE_STEP);
        let polls = [
            (snapshot(fresh(), vec![]), Process::Running(100)),
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
            (snapshot(tracking(5, 1e-6), vec![]), Process::Running(200)),
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

    /// The events of `collector` recording each poll's tracking, from
    /// chronyd at the PID beside it, one poll a second.
    fn corrections(
        name: &str,
        config: &str,
        polls: impl IntoIterator<Item = (Tracking, u32)>,
    ) -> Vec<String> {
        let mut db = Database::new(name);
        let mut collector = collector(&db, config);
        for (second, (tracking, pid)) in (0..).zip(polls) {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(tracking, vec![]),
                        Process::Running(pid),
                    )
                })
                .expect("recorded");
        }
        db.events()
    }

    #[test]
    fn hands_the_rules_steps_to_the_clocks_check() {
        let mut db = Database::new("take-step");
        let mut collector = collector(&db, MAKE_STEP);
        let mut poll = |db: &mut Database, second, tracking| {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(tracking, vec![]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
            collector.take_step()
        };
        assert_eq!(poll(&mut db, 0, fresh()), None);
        assert_eq!(poll(&mut db, 1, tracking(1, 1.5)), Some(1.5));
        // Taken: not again until another.
        assert_eq!(poll(&mut db, 2, tracking(1, 1.5)), None);
        // Past the limit of 3 updates, slewed: no step to hand over.
        for second in 3..6 {
            poll(&mut db, second, tracking(second, 2.0 + second as f64));
        }
        assert_eq!(poll(&mut db, 6, tracking(6, 9.0)), None);
    }

    #[test]
    fn steps_only_where_makestep_would() {
        let polls = [
            (fresh(), 100),
            (tracking(1, 1.5), 100),
            // Read again, not updated.
            (tracking(1, 1.5), 100),
            // Sync lost: the reference time changes, the offset not.
            (
                Tracking {
                    reference_time: Timestamp::UNIX_EPOCH,
                    ..tracking(1, 1.5)
                },
                100,
            ),
            (tracking(3, 1e-3), 100),
            (tracking(4, 2e-3), 100),
            // The fourth update: slewed.
            (tracking(5, -2.0), 100),
            // A restart, and its first update.
            (fresh(), 200),
            (tracking(7, 3.0), 200),
        ];
        assert_eq!(
            corrections("makestep", MAKE_STEP, polls),
            ["step", "large_offset", "chronyd_restart", "step"]
        );
        // No makestep: chronyd never steps.
        assert_eq!(
            corrections("no-makestep", "", polls),
            [
                "large_offset",
                "large_offset",
                "chronyd_restart",
                "large_offset"
            ]
        );
        // Running before the daemon: its updates were not counted.
        assert_eq!(
            corrections("unknown", MAKE_STEP, polls[1..7].to_vec()),
            ["large_offset"]
        );
        assert_eq!(
            corrections("always", "makestep 1 -1\n", polls[1..7].to_vec()),
            ["step"]
        );
    }

    #[test]
    fn rereads_the_configuration_when_chronyd_restarts() {
        let mut db = Database::new("config");
        let mut collector = collector(&db, MAKE_STEP);
        let locks = |db: &Database| -> Option<String> {
            db.reader
                .query_row(
                    "SELECT value FROM meta WHERE key = 'refclock_locks'",
                    [],
                    |row| row.get(0),
                )
                .ok()
        };
        let mut poll = |db: &mut Database, second, read, pid| {
            db.log
                .write(|batch| collector.record(batch, at(second), read, Process::Running(pid)))
                .expect("recorded");
        };
        poll(&mut db, 0, snapshot(fresh(), vec![]), 100);
        assert_eq!(locks(&db).as_deref(), Some(""));
        std::fs::write(
            db.directory().join("chrony.conf"),
            "refclock SHM 0 refid NMEA noselect\nrefclock PPS /dev/pps0 refid PPS lock NMEA\n",
        )
        .expect("written");
        // Not read again while chronyd runs on, as chronyd does not.
        poll(&mut db, 1, snapshot(fresh(), vec![]), 100);
        assert_eq!(locks(&db).as_deref(), Some(""));
        poll(&mut db, 2, snapshot(fresh(), vec![]), 200);
        assert_eq!(locks(&db).as_deref(), Some("50505300 4E4D4541"));
        // Nor while it is down; again when it is back.
        std::fs::write(db.directory().join("chrony.conf"), MAKE_STEP).expect("written");
        poll(&mut db, 3, down(), 200);
        assert_eq!(locks(&db).as_deref(), Some("50505300 4E4D4541"));
        poll(&mut db, 4, snapshot(fresh(), vec![]), 200);
        assert_eq!(locks(&db).as_deref(), Some(""));
    }

    #[test]
    fn keeps_the_last_configuration_it_could_read() {
        let mut db = Database::new("config-unreadable");
        let paired = "refclock SHM 0 refid NMEA\nrefclock PPS /dev/pps0 refid PPS lock NMEA\n";
        let mut collector = collector(&db, paired);
        let meta = |db: &Database, key: &str| -> Option<String> {
            db.reader
                .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .ok()
        };
        let count = |db: &Database, kind: &str| -> i64 {
            db.reader
                .query_row(
                    "SELECT COUNT(*) FROM event WHERE kind = ?1",
                    [kind],
                    |row| row.get(0),
                )
                .expect("counted")
        };
        let errors = |db: &Database| count(db, "chrony_config_error");
        let mut poll = |db: &mut Database, second, pid| {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(fresh(), vec![]),
                        Process::Running(pid),
                    )
                })
                .expect("recorded");
        };
        let locks = "50505300 4E4D4541";
        poll(&mut db, 0, 100);
        assert_eq!(meta(&db, "refclock_locks").as_deref(), Some(locks));
        assert_eq!(meta(&db, "chrony_config_error").as_deref(), Some(""));
        // Gone when chronyd restarts: not taken for empty, which would
        // unpair all history.
        let path = db.directory().join("chrony.conf");
        std::fs::remove_file(&path).expect("removed");
        poll(&mut db, 1, 200);
        assert_eq!(meta(&db, "refclock_locks").as_deref(), Some(locks));
        assert!(meta(&db, "chrony_config_error").is_some_and(|e| e.contains("chrony.conf")));
        assert_eq!(errors(&db), 1);
        // Once per read: not again while chronyd runs on.
        poll(&mut db, 2, 200);
        assert_eq!(errors(&db), 1);
        // Back, and read when chronyd next starts.
        std::fs::write(&path, MAKE_STEP).expect("written");
        poll(&mut db, 3, 300);
        assert_eq!(meta(&db, "refclock_locks").as_deref(), Some(""));
        assert_eq!(meta(&db, "chrony_config_error").as_deref(), Some(""));
        assert_eq!(errors(&db), 1);
        assert_eq!(count(&db, "chrony_config_read"), 1);
    }

    /// chronyd answering with `sources`, by index.
    struct Fake {
        sources: Vec<Source>,
        /// Asked for `sourcestats` of the index first, chronyd removes
        /// the source second, as a source going between two requests.
        remove: Option<(u32, usize)>,
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
            if let Some((at, removed)) = self.remove
                && at == index
            {
                self.sources.remove(removed);
                self.remove = None;
            }
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
            Ok(format!("{address}.example"))
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
            remove: None,
        };
        let snapshot = unconfigured().read(&mut fake).expect("a snapshot");
        let sources = snapshot.sources.expect("sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].data.address, Address::Ip(SERVER));
        assert_eq!(snapshot.names.len(), 1);
    }

    /// A source like [`source`]'s at another address.
    fn source_at(address: IpAddr) -> Source {
        let mut source = source(5, 10);
        source.data.address = Address::Ip(address);
        source.stats.address = source.data.address;
        source.selection.address = source.data.address;
        if let Some(ntp) = &mut source.ntp {
            ntp.remote_address = source.data.address;
        }
        source
    }

    #[test]
    fn a_source_removed_while_read_discards_the_sources() {
        let addresses: Vec<IpAddr> = (1..=3)
            .map(|host| IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, host)))
            .collect();
        let collector = unconfigured();
        // The source being read goes: the next one's reports take its
        // index.  Then one not yet read goes: the one after it is moved
        // past.
        for remove in [(0, 0), (0, 1)] {
            let mut fake = Fake {
                sources: addresses.iter().copied().map(source_at).collect(),
                remove: Some(remove),
            };
            let snapshot = collector.read(&mut fake).expect("a snapshot");
            assert!(snapshot.sources.is_none(), "{remove:?}");
            // The next poll reads what is left.
            let snapshot = collector.read(&mut fake).expect("a snapshot");
            assert_eq!(snapshot.sources.expect("sources").len(), 2);
        }
    }

    #[test]
    fn reports_reach_lost_and_samples_gone() {
        let mut db = Database::new("report");
        let mut collector = collector(&db, MAKE_STEP);
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
                remove: None,
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
        let mut collector = collector(&db, MAKE_STEP);
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

    #[test]
    fn extrapolation_is_not_a_new_regression() {
        let mut db = Database::new("extrapolated");
        let mut collector = collector(&db, MAKE_STEP);
        for second in 0..4 {
            // One regression, its offset and error extrapolated a second
            // further each poll, as chronyd reports them; a new sample,
            // and a new regression, at the last.
            let mut s = source(5 + second, 10);
            let elapsed = second as f64;
            s.stats.residual_frequency = Ppm::new(0.1);
            s.stats.estimated_offset = Seconds::new(elapsed * 0.1e-6);
            s.stats.estimated_offset_error = Seconds::new(1e-6 + elapsed * 0.01e-6);
            if second == 3 {
                s = source(0, 11);
            }
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(i64::try_from(second).expect("a second")),
                        snapshot(tracking(0, 0.0), vec![s]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
        }
        assert_eq!(db.count("source_stats"), 2);
    }

    #[test]
    fn tracking_is_reported_as_chronyd_last_said() {
        let mut db = Database::new("tracking-report");
        let mut collector = collector(&db, MAKE_STEP);
        // chronyd's while unsynchronized (`REF_GetReferenceParams`,
        // chrony 4.8 `reference.c:1216`).
        let report = |db: &Database| -> (i64, String) {
            db.reader
                .query_row(
                    "SELECT reference_time, leap FROM tracking_report",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("a report")
        };
        let polls = [
            (0, tracking(0, 1e-6)),
            (1, unsynchronized(1e-6)),
            (2, tracking(2, 2e-6)),
            (3, unsynchronized(2e-6)),
            // Synchronized, then stepped back.
            (4, tracking(100, 3e-6)),
            (5, tracking(50, 4e-6)),
            (6, tracking(50, 4e-6)),
        ];
        for (second, read) in polls {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(read, vec![]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
            let expected = (
                to_stored(read.reference_time).expect("in range"),
                format!("{:?}", read.leap).to_lowercase(),
            );
            assert_eq!(report(&db), expected, "at {second}");
        }
        // The first read and each update; not losing sync.
        assert_eq!(db.count("tracking"), 4);
        assert_eq!(db.count("tracking_report"), 1);
    }

    /// chronyd's tracking while unsynchronized, its last update's offset
    /// `last_offset` (`REF_GetReferenceParams`, chrony 4.8
    /// `reference.c:1216`; `REF_SetUnsynchronised` keeps the offset).
    fn unsynchronized(last_offset: f64) -> Tracking {
        Tracking {
            reference_time: Timestamp::UNIX_EPOCH,
            reference_id: ReferenceId(0),
            leap: Leap::Unsynchronized,
            stratum: 0,
            ..tracking(0, last_offset)
        }
    }

    #[test]
    fn a_restart_does_not_repeat_the_last_update() {
        let mut db = Database::new("restart-repeat");
        let stored = |second| to_stored(at(second)).expect("in range");
        let local = Tracking {
            reference_id: ReferenceId::LOCAL,
            address: Address::Unspecified,
            stratum: 10,
            ..tracking(9, 2e-6)
        };
        // Each run of the daemon: a collector of its own, and its polls.
        let runs = [
            vec![(0, tracking(0, 1e-6)), (1, tracking(1, 2e-6))],
            // Its first read the last update, its reference time moved by
            // slews since (`handle_slew`, chrony 4.8 `reference.c:175`);
            // then an update.
            vec![(3, tracking(2, 2e-6)), (4, tracking(4, 3e-6))],
            // The last update again, unsynchronized, then on the local
            // reference: each kept, marking when chronyd was.
            vec![(5, unsynchronized(3e-6))],
            vec![(10, local)],
            vec![(11, local)],
        ];
        for polls in runs {
            let mut collector = collector(&db, MAKE_STEP);
            for (second, read) in polls {
                db.log
                    .write(|batch| {
                        collector.record(
                            batch,
                            at(second),
                            snapshot(read, vec![]),
                            Process::Running(100),
                        )
                    })
                    .expect("recorded");
            }
        }
        let mut statement = db
            .reader
            .prepare("SELECT at FROM tracking ORDER BY rowid")
            .expect("a query");
        let rows: Vec<i64> = statement
            .query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("a row"))
            .collect();
        assert_eq!(rows, [0, 1, 4, 5, 10, 11].map(stored));
    }

    #[test]
    fn a_refclock_and_none_are_named() {
        let mut db = Database::new("refclock-reference");
        let mut collector = collector(&db, MAKE_STEP);
        // A refclock has no address, only its reference ID; none has
        // neither.
        let refclock = Tracking {
            reference_id: ReferenceId(u32::from_be_bytes(*b"GPS\0")),
            address: Address::Unspecified,
            stratum: 1,
            ..tracking(1, 1e-6)
        };
        let none = Tracking {
            address: Address::Unspecified,
            ..unsynchronized(1e-6)
        };
        for (second, read) in [
            (0, tracking(0, 1e-6)),
            (1, refclock),
            (2, none),
            (3, tracking(3, 2e-6)),
        ] {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(read, vec![]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
        }
        let mut statement = db
            .reader
            .prepare("SELECT detail FROM event WHERE kind = 'reference_change' ORDER BY rowid")
            .expect("a query");
        let details: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("a row"))
            .collect();
        assert_eq!(
            details,
            ["192.0.2.1 to GPS", "GPS to none", "none to 192.0.2.1"]
        );
    }

    #[test]
    fn the_local_reference_is_named() {
        let mut db = Database::new("local-reference");
        let mut collector = collector(&db, MAKE_STEP);
        // chronyd's tracking on its local reference
        // (`REF_GetReferenceParams`, chrony 4.8 `reference.c:1184`).
        let local = Tracking {
            reference_id: ReferenceId::LOCAL,
            address: Address::Unspecified,
            stratum: 10,
            ..tracking(1, 1e-6)
        };
        for (second, read) in [(0, tracking(0, 1e-6)), (1, local), (2, tracking(2, 2e-6))] {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(read, vec![]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
        }
        let mut statement = db
            .reader
            .prepare("SELECT detail FROM event WHERE kind = 'reference_change' ORDER BY rowid")
            .expect("a query");
        let details: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("a row"))
            .collect();
        assert_eq!(details, ["192.0.2.1 to local", "local to 192.0.2.1"]);
    }

    #[test]
    fn a_history_row_is_an_update() {
        let mut db = Database::new("tracking-updates");
        let mut collector = collector(&db, MAKE_STEP);
        let polls = [
            (0, tracking(0, 1e-6)),
            // A slew or a frequency change: the reference time moved, and
            // nothing else.
            (1, tracking(1, 1e-6)),
            (2, unsynchronized(1e-6)),
            (3, unsynchronized(1e-6)),
            // Back, an update.
            (4, tracking(4, 2e-6)),
            (5, tracking(5, 2e-6)),
            (6, tracking(6, 3e-6)),
        ];
        for (second, read) in polls {
            db.log
                .write(|batch| {
                    collector.record(
                        batch,
                        at(second),
                        snapshot(read, vec![]),
                        Process::Running(100),
                    )
                })
                .expect("recorded");
        }
        let mut statement = db
            .reader
            .prepare("SELECT at, reference_time FROM tracking ORDER BY rowid")
            .expect("a query");
        let rows: Vec<(i64, i64)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("rows")
            .map(|row| row.expect("a row"))
            .collect();
        let stored = |second| to_stored(at(second)).expect("in range");
        assert_eq!(
            rows,
            [0, 4, 6].map(|second| (stored(second), stored(second)))
        );
        // Losing and regaining sync are events, though no row is.
        assert_eq!(
            db.events(),
            [
                "reference_change",
                "leap_change",
                "reference_change",
                "leap_change"
            ]
        );
    }
}
