//! PPS edges as the kernel sees them, before chronyd: whether each
//! pulse lands.  chronyd logs a pulse only once it accepts it, after its
//! lock and offset checks (4.8 `refclock.c`, `RCL_AddCookedPulse`), so a
//! gap in its samples is a pulse missing or refused; the kernel tells
//! the two apart.
//!
//! Two kinds of device:
//!
//! - `/dev/ppsN`, read through `/sys/class/pps/ppsN/assert`: the last
//!   edge's time and sequence (`pps_show_assert`, `drivers/pps/sysfs.c`).
//! - `/dev/ptpN[:channel]`, a clock timestamping the pulse on a pin:
//!   its external timestamp events, `struct ptp_extts_event`
//!   (`include/uapi/linux/ptp_clock.h`).  From Linux 6.7 each open file
//!   has its own queue of them, every channel unmasked, so a second
//!   reader takes none from chronyd and needs no ioctl.  Before 6.7 one
//!   queue is shared, so the device is refused.

use std::fmt;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use chronymon::event::EventKind;
use chronymon_log::writer::Batch;
use jiff::Timestamp;

/// The pulse's period: chronyd's `rate` default, 1 Hz (`refclock`
/// documentation).
const PERIOD: f64 = 1.0;

/// How old the last edge may be at a poll before the next is missed:
/// half a period's slack for the poll's jitter.
const LATE: f64 = 1.5 * PERIOD;

/// A clock's events carry no system time, only the clock's own, so one
/// is late when this many polls in a row read none.
const SILENT_POLLS: u32 = 2;

/// How long after a loss another starts flapping, and how long pulses
/// must run unbroken for flapping to end.
const SETTLING: f64 = 60.0;

/// Where the kernel shows a PPS device's last edge.
const PPS_CLASS: &str = "/sys/class/pps";

/// The first kernel giving each reader of a clock its own events.
const PER_READER_EVENTS: (u32, u32) = (6, 7);

/// The kernel's release.
const OSRELEASE: &str = "/proc/sys/kernel/osrelease";

/// The size of one `struct ptp_extts_event`: a `ptp_clock_time` (`i64`
/// seconds, `u32` nanoseconds, `u32` reserved), then `u32` index and
/// flags and two reserved.
const EXTTS_EVENT: usize = 32;
const EXTTS_NANOSECONDS: usize = 8;
const EXTTS_INDEX: usize = 16;

/// A nanosecond in seconds.
const NANOSECOND: f64 = 1e-9;

/// How many events one read takes at most.
const EXTTS_EVENTS_PER_READ: usize = 16;

/// How long a clock's reader waits before opening it again.
const REOPEN: Duration = Duration::from_secs(1);

/// A device `--pps` names.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Device {
    /// `/dev/ppsN`, by its kernel name `ppsN`.
    Pps(String),
    /// `/dev/ptpN` and the channel of its events that are the pulse.
    Clock { path: PathBuf, channel: u32 },
}

impl FromStr for Device {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (path, channel) = match text.rsplit_once(':') {
            Some((path, channel)) => (
                path,
                Some(
                    channel
                        .parse()
                        .map_err(|_| format!("{text}: channel must be a number"))?,
                ),
            ),
            None => (text, None),
        };
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with("pps") && channel.is_none() {
            Ok(Self::Pps(name))
        } else if name.starts_with("ptp") {
            Ok(Self::Clock {
                path: PathBuf::from(path),
                channel: channel.unwrap_or(0),
            })
        } else {
            Err(format!("{text}: not /dev/ppsN or /dev/ptpN[:channel]"))
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pps(name) => write!(f, "{name}"),
            Self::Clock { path, channel } => {
                let name = path.file_name().unwrap_or(path.as_os_str());
                write!(f, "{}:{channel}", name.to_string_lossy())
            }
        }
    }
}

/// What one poll of a device saw.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reading {
    /// The device could not be read, and why.
    Gone(String),
    /// It could: edges new since the last poll, how many were missed
    /// among and before them, and whether the next is late.
    Edges { new: u64, missed: u64, late: bool },
}

/// What a device's pulse is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pulse {
    /// Not yet read.
    Unknown,
    Gone,
    Arriving,
    /// Missing since a poll, this many edges so far.
    Missing {
        missed: u64,
    },
}

/// A run of losses close together, journaled as one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flapping {
    missed: u64,
    losses: u64,
}

/// One device's pulse, poll to poll: what it is doing and what has
/// been journaled of it.
#[derive(Debug)]
pub(crate) struct Tracker {
    name: String,
    pulse: Pulse,
    /// When pulses last came back after a loss, in seconds.
    back_at: Option<f64>,
    flapping: Option<Flapping>,
}

/// An event to journal: its kind and detail.
type Journaled = (EventKind, String);

impl Tracker {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            pulse: Pulse::Unknown,
            back_at: None,
            flapping: None,
        }
    }

    /// Take in one poll's `reading` at `now`, in seconds, and return
    /// what to journal.
    pub(crate) fn observe(&mut self, now: f64, reading: Reading) -> Vec<Journaled> {
        let mut journal = Vec::new();
        let (new, missed, late) = match reading {
            Reading::Edges { new, missed, late } => (new, missed, late),
            Reading::Gone(why) => {
                if self.pulse != Pulse::Gone {
                    journal.push((EventKind::PpsDown, format!("{}: {why}", self.name)));
                }
                self.pulse = Pulse::Gone;
                self.flapping = None;
                self.back_at = None;
                return journal;
            }
        };
        if self.pulse == Pulse::Gone {
            journal.push((EventKind::PpsUp, self.name.clone()));
            self.pulse = Pulse::Unknown;
        }
        match self.pulse {
            // The first reading is where counting starts.
            Pulse::Unknown | Pulse::Gone => {
                self.pulse = if new == 0 && late {
                    journal.push((EventKind::PpsLost, self.name.clone()));
                    Pulse::Missing { missed: 0 }
                } else {
                    Pulse::Arriving
                };
                return journal;
            }
            Pulse::Arriving if missed > 0 || (new == 0 && late) => self.lost(now, &mut journal),
            Pulse::Arriving | Pulse::Missing { .. } => {}
        }
        if let Pulse::Missing { missed: before } = self.pulse {
            let missed = before + missed;
            if new > 0 {
                self.back(now, missed, &mut journal);
                self.pulse = Pulse::Arriving;
            } else {
                self.pulse = Pulse::Missing { missed };
            }
        }
        if self.pulse == Pulse::Arriving
            && let (Some(flapping), Some(back_at)) = (self.flapping, self.back_at)
            && now - back_at >= SETTLING
        {
            journal.push((
                EventKind::PpsSteady,
                format!(
                    "{}: {} edges missed in {} losses",
                    self.name, flapping.missed, flapping.losses
                ),
            ));
            self.flapping = None;
        }
        journal
    }

    /// A loss starting: lost, or flapping if soon after the last.
    fn lost(&mut self, now: f64, journal: &mut Vec<Journaled>) {
        self.pulse = Pulse::Missing { missed: 0 };
        let soon = self.back_at.is_some_and(|back_at| now - back_at < SETTLING);
        match &mut self.flapping {
            Some(flapping) => flapping.losses += 1,
            None if soon => {
                journal.push((EventKind::PpsFlapping, self.name.clone()));
                self.flapping = Some(Flapping {
                    missed: 0,
                    losses: 1,
                });
            }
            None => journal.push((EventKind::PpsLost, self.name.clone())),
        }
    }

    /// Pulses back after `missed` edges.
    fn back(&mut self, now: f64, missed: u64, journal: &mut Vec<Journaled>) {
        self.back_at = Some(now);
        match &mut self.flapping {
            Some(flapping) => flapping.missed += missed,
            None => journal.push((
                EventKind::PpsBack,
                format!("{}: {missed} edges missed", self.name),
            )),
        }
    }
}

/// Edges missed between two edges `interval` seconds apart.
fn missed_between(interval: f64) -> u64 {
    let periods = (interval / PERIOD).round();
    if periods > 1.0 {
        // Whole periods, as many as a pulse can miss.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "positive and whole"
        )]
        let periods = periods as u64;
        periods - 1
    } else {
        0
    }
}

/// `/sys/class/pps/ppsN`'s last edge: its time, in seconds, and its
/// sequence; none before the first.
fn assert_edge(name: &str) -> io::Result<Option<(f64, u64)>> {
    let text = fs::read_to_string(Path::new(PPS_CLASS).join(name).join("assert"))?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, format!("assert {text:?}"));
    let (time, sequence) = text.trim().split_once('#').ok_or_else(invalid)?;
    let time: f64 = time.parse().map_err(|_| invalid())?;
    let sequence: u64 = sequence.parse().map_err(|_| invalid())?;
    Ok((sequence > 0).then_some((time, sequence)))
}

/// A `/dev/ppsN`'s last edge as last read.
#[derive(Debug, Default)]
struct PpsReader {
    last: Option<(f64, u64)>,
}

impl PpsReader {
    fn read(&mut self, name: &str, now: f64) -> Reading {
        match assert_edge(name) {
            Ok(edge) => self.update(edge, now),
            Err(e) => {
                self.last = None;
                Reading::Gone(e.to_string())
            }
        }
    }

    /// What the last `edge`, its time and sequence, read at `now`, says.
    fn update(&mut self, edge: Option<(f64, u64)>, now: f64) -> Reading {
        let Some((time, sequence)) = edge else {
            return Reading::Edges {
                new: 0,
                missed: 0,
                late: true,
            };
        };
        let (new, missed) = match self.last {
            Some((last_time, last_sequence)) if sequence > last_sequence => {
                let new = sequence - last_sequence;
                let expected = missed_between(time - last_time) + 1;
                (new, expected.saturating_sub(new))
            }
            Some(_) => (0, 0),
            // The first read: the edge was there before.
            None => (0, 0),
        };
        self.last = Some((time, sequence));
        Reading::Edges {
            new,
            missed,
            late: now - time > LATE,
        }
    }
}

/// What a clock's reader thread sends: the times of its pulse's edges,
/// in the clock's own seconds, or why it could not read.
type ClockEvents = Result<Vec<f64>, String>;

/// A `/dev/ptpN`'s events as read on a thread of its own.
#[derive(Debug)]
struct ClockReader {
    events: Receiver<ClockEvents>,
    last: Option<f64>,
    silent: u32,
    /// Why the device could not be read, until it can.
    gone: Option<String>,
}

impl ClockReader {
    fn start(path: PathBuf, channel: u32) -> io::Result<Self> {
        let (send, events) = mpsc::channel();
        thread::Builder::new()
            .name(format!("chronymond-{}", path.display()))
            .spawn(move || read_clock(&path, channel, &send))?;
        Ok(Self {
            events,
            last: None,
            silent: 0,
            gone: None,
        })
    }

    fn read(&mut self) -> Reading {
        let mut times = Vec::new();
        for events in self.events.try_iter() {
            match events {
                Ok(more) => {
                    self.gone = None;
                    times.extend(more);
                }
                Err(why) => {
                    self.gone = Some(why);
                    self.last = None;
                    times.clear();
                }
            }
        }
        if let Some(why) = &self.gone {
            return Reading::Gone(why.clone());
        }
        let mut missed = 0;
        for &time in &times {
            if let Some(last) = self.last {
                missed += missed_between(time - last);
            }
            self.last = Some(time);
        }
        let new = times.len() as u64;
        self.silent = if new == 0 { self.silent + 1 } else { 0 };
        Reading::Edges {
            new,
            missed,
            late: self.silent >= SILENT_POLLS,
        }
    }
}

/// Read `path`'s events of `channel` for ever, sending them on, and why
/// not when it cannot, opening it again after a while.
fn read_clock(path: &Path, channel: u32, send: &Sender<ClockEvents>) {
    loop {
        let error = match File::open(path) {
            Ok(file) => read_events(file, channel, send),
            Err(e) => e,
        };
        if send.send(Err(error.to_string())).is_err() {
            return;
        }
        thread::sleep(REOPEN);
    }
}

/// Read `file`'s events of `channel` until a read fails.
fn read_events(mut file: File, channel: u32, send: &Sender<ClockEvents>) -> io::Error {
    let mut buffer = [0; EXTTS_EVENT * EXTTS_EVENTS_PER_READ];
    loop {
        let length = match file.read(&mut buffer) {
            Ok(0) => return io::Error::from(io::ErrorKind::UnexpectedEof),
            Ok(length) => length,
            Err(e) => return e,
        };
        let times = buffer[..length]
            .as_chunks::<EXTTS_EVENT>()
            .0
            .iter()
            .filter_map(|event| edge_time(event, channel))
            .collect();
        if send.send(Ok(times)).is_err() {
            return io::Error::from(io::ErrorKind::BrokenPipe);
        }
    }
}

/// One `struct ptp_extts_event`'s time, in the clock's seconds, if of
/// `channel`.
fn edge_time(event: &[u8], channel: u32) -> Option<f64> {
    let word = |at: usize| {
        event
            .get(at..at + 4)?
            .try_into()
            .ok()
            .map(u32::from_ne_bytes)
    };
    let seconds = i64::from_ne_bytes(event.get(..EXTTS_NANOSECONDS)?.try_into().ok()?);
    if word(EXTTS_INDEX)? != channel {
        return None;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "seconds since the epoch fit a double's 53 bits"
    )]
    let seconds = seconds as f64;
    Some(seconds + f64::from(word(EXTTS_NANOSECONDS)?) * NANOSECOND)
}

/// Whether this kernel gives each reader of a clock its own events.
fn per_reader_events() -> bool {
    let Ok(release) = fs::read_to_string(OSRELEASE) else {
        return false;
    };
    let mut numbers = release
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse::<u32>().ok());
    match (numbers.next(), numbers.next()) {
        (Some(major), Some(minor)) => (major, minor) >= PER_READER_EVENTS,
        _ => false,
    }
}

/// One device, its reader and its tracker.
#[derive(Debug)]
struct Watched {
    device: Device,
    reader: Reader,
    tracker: Tracker,
}

/// How a device is read.
#[derive(Debug)]
enum Reader {
    Pps(PpsReader),
    Clock(ClockReader),
}

/// Every device `--pps` names, polled with chronyd.
#[derive(Debug, Default)]
pub(crate) struct Pps {
    watched: Vec<Watched>,
}

impl Pps {
    /// Watch `devices`; a clock this kernel would let a reader take
    /// chronyd's events from is refused, said so, and not read.
    pub(crate) fn new(devices: &[Device]) -> (Self, Vec<String>) {
        let mut refused = Vec::new();
        let mut watched = Vec::new();
        for device in devices {
            let reader = match device {
                Device::Pps(_) => Reader::Pps(PpsReader::default()),
                Device::Clock { .. } if !per_reader_events() => {
                    refused.push(format!(
                        "{device}: Linux before {}.{} would take chronyd's events; not read",
                        PER_READER_EVENTS.0, PER_READER_EVENTS.1
                    ));
                    continue;
                }
                Device::Clock { path, channel } => {
                    match ClockReader::start(path.clone(), *channel) {
                        Ok(reader) => Reader::Clock(reader),
                        Err(e) => {
                            refused.push(format!("{device}: {e}"));
                            continue;
                        }
                    }
                }
            };
            watched.push(Watched {
                tracker: Tracker::new(device.to_string()),
                device: device.clone(),
                reader,
            });
        }
        (Self { watched }, refused)
    }

    /// Read every device at `at` and journal what changed.
    pub(crate) fn poll(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
    ) -> chronymon_log::error::Result<()> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "to the microsecond, enough for edges a second apart"
        )]
        let now = at.as_nanosecond() as f64 * NANOSECOND;
        for watched in &mut self.watched {
            let reading = match (&mut watched.reader, &watched.device) {
                (Reader::Pps(reader), Device::Pps(name)) => reader.read(name, now),
                (Reader::Clock(reader), _) => reader.read(),
                (Reader::Pps(_), Device::Clock { .. }) => continue,
            };
            for (kind, detail) in watched.tracker.observe(now, reading) {
                eprintln!("chronymond: {}: {detail}", kind.name());
                batch.event(at, kind, None, Some(&detail))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A poll seeing `new` edges, `missed` missed among them.
    const fn edges(new: u64, missed: u64) -> Reading {
        Reading::Edges {
            new,
            missed,
            late: false,
        }
    }

    /// A poll seeing none, the next late.
    const SILENT: Reading = Reading::Edges {
        new: 0,
        missed: 0,
        late: true,
    };

    /// The kinds `tracker` journals for `readings`, one a second from
    /// `start`.
    fn kinds(tracker: &mut Tracker, start: f64, readings: &[Reading]) -> Vec<&'static str> {
        readings
            .iter()
            .enumerate()
            .flat_map(|(n, reading)| tracker.observe(start + n as f64, reading.clone()))
            .map(|(kind, _)| kind.name())
            .collect()
    }

    fn gone() -> Reading {
        Reading::Gone("Permission denied (os error 13)".to_owned())
    }

    fn tracker() -> Tracker {
        let mut tracker = Tracker::new("pps2".to_owned());
        assert!(kinds(&mut tracker, 0.0, &[edges(0, 0), edges(1, 0)]).is_empty());
        tracker
    }

    #[test]
    fn a_single_missed_edge_is_lost_and_back() {
        let mut tracker = tracker();
        let journal = tracker.observe(10.0, edges(1, 1));
        assert_eq!(
            journal,
            [
                (EventKind::PpsLost, "pps2".to_owned()),
                (EventKind::PpsBack, "pps2: 1 edges missed".to_owned())
            ]
        );
    }

    #[test]
    fn an_outage_is_one_loss_however_long() {
        let mut tracker = tracker();
        assert_eq!(
            kinds(&mut tracker, 10.0, &[SILENT, SILENT, SILENT, SILENT]),
            ["pps_lost"]
        );
        let journal = tracker.observe(14.0, edges(1, 4));
        assert_eq!(
            journal,
            [(EventKind::PpsBack, "pps2: 4 edges missed".to_owned())]
        );
    }

    #[test]
    fn flapping_is_capped_to_its_start_and_end() {
        let mut tracker = tracker();
        assert_eq!(
            kinds(&mut tracker, 10.0, &[edges(1, 1)]),
            ["pps_lost", "pps_back"]
        );
        // Another loss within the settling window: flapping, once.
        assert_eq!(
            kinds(
                &mut tracker,
                20.0,
                &[edges(1, 2), SILENT, edges(1, 3), edges(1, 1)]
            ),
            ["pps_flapping"]
        );
        // Unbroken for the window since pulses last came back.
        let steady: Vec<Reading> = (0..60).map(|_| edges(1, 0)).collect();
        assert!(kinds(&mut tracker, 24.0, &steady[..58]).is_empty());
        let journal = tracker.observe(83.0, edges(1, 0));
        assert_eq!(
            journal,
            [(
                EventKind::PpsSteady,
                "pps2: 6 edges missed in 3 losses".to_owned()
            )]
        );
        // And a loss after is a loss again.
        assert_eq!(
            kinds(&mut tracker, 200.0, &[edges(1, 1)]),
            ["pps_lost", "pps_back"]
        );
    }

    #[test]
    fn a_device_gone_is_down_once_then_up() {
        let mut tracker = tracker();
        assert_eq!(
            tracker.observe(9.0, gone()),
            [(
                EventKind::PpsDown,
                "pps2: Permission denied (os error 13)".to_owned()
            )]
        );
        assert_eq!(
            kinds(
                &mut tracker,
                10.0,
                &[gone(), gone(), edges(0, 0), edges(1, 0)]
            ),
            ["pps_up"]
        );
    }

    #[test]
    fn not_pulsing_when_first_read_is_lost() {
        let mut tracker = Tracker::new("pps0".to_owned());
        assert_eq!(
            kinds(&mut tracker, 0.0, &[SILENT, SILENT, edges(1, 0)]),
            ["pps_lost", "pps_back"]
        );
    }

    #[test]
    fn counts_a_pps_devices_missed_edges_by_its_sequence() {
        let mut reader = PpsReader::default();
        let mut at =
            |time: f64, sequence: u64, now: f64| reader.update(Some((time, sequence)), now);
        assert_eq!(at(100.0, 7, 100.2), edges(0, 0));
        // Two edges between polls, none missed.
        assert_eq!(at(102.0, 9, 102.3), edges(2, 0));
        // Four seconds on, two edges: two missed.
        assert_eq!(at(106.0, 11, 106.1), edges(2, 2));
        // None since, and late.
        assert_eq!(at(106.0, 11, 107.7), SILENT);
        assert_eq!(
            reader.update(None, 0.0),
            SILENT,
            "no edge yet is a late one"
        );
    }

    #[test]
    fn counts_edges_missed_between_two() {
        assert_eq!(missed_between(1.0), 0);
        assert_eq!(missed_between(1.4), 0);
        assert_eq!(missed_between(2.0), 1);
        assert_eq!(missed_between(3.1), 2);
        assert_eq!(missed_between(0.0), 0);
    }

    #[test]
    fn reads_an_extts_event() {
        let mut event = [0; EXTTS_EVENT];
        event[..8].copy_from_slice(&1_793_429_889_i64.to_ne_bytes());
        event[8..12].copy_from_slice(&500_000_000_u32.to_ne_bytes());
        event[16..20].copy_from_slice(&1_u32.to_ne_bytes());
        assert_eq!(edge_time(&event, 0), None);
        assert_eq!(edge_time(&event, 1), Some(1_793_429_889.5));
    }

    #[test]
    fn parses_devices() {
        assert_eq!("/dev/pps2".parse(), Ok(Device::Pps("pps2".to_owned())));
        assert_eq!(
            "/dev/ptp2".parse(),
            Ok(Device::Clock {
                path: PathBuf::from("/dev/ptp2"),
                channel: 0
            })
        );
        assert_eq!(
            "/dev/ptp2:1".parse::<Device>().map(|d| d.to_string()),
            Ok("ptp2:1".to_owned())
        );
        assert!("/dev/pps2:1".parse::<Device>().is_err());
        assert!("/dev/ttyS0".parse::<Device>().is_err());
        assert!("/dev/ptp2:x".parse::<Device>().is_err());
    }
}
