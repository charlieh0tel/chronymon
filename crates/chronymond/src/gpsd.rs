//! What gpsd says of its receivers: whether each has a fix, and its
//! satellites and dilutions of precision, from gpsd's generic reports,
//! TPV and SKY, the same whatever the receiver (`gpsd_json(5)`, 3.25).
//! Every other class, a driver's raw measurements among them, is read
//! past.
//!
//! gpsd run for chronyd runs with `-n`, polling its devices whether or
//! not a client is connected, so connecting starts nothing.  Its
//! connection is its health: gpsd down, or up again.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Write as _;
use std::net::TcpStream;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use chronymon::event::EventKind;
use chronymon_log::schema::Trouble;
use chronymon_log::writer::Batch;
use chronymon_log::writer::Gnss;
use jiff::Timestamp;
use serde_json::Value;

use crate::time::seconds;

/// gpsd's port (gpsd(8)).
const DEFAULT_PORT: u16 = 2947;

/// Every report, as JSON, none of the device's own data.
const WATCH: &[u8] = b"?WATCH={\"enable\":true,\"json\":true};\n";

/// How long before connecting again after gpsd went away.
const RECONNECT: Duration = Duration::from_secs(1);

/// How long a device may send no TPV, in seconds, before it is taken to
/// have no fix: gpsd sends one a cycle, a second by default.
const SILENT: f64 = 10.0;

/// How many TPVs in a row must agree before a change of fix is
/// journaled, so a fix wavering once is not two events.
const FIX_CONFIRM: u32 = 2;

/// TPV's mode of a 3D fix.
const MODE_3D: i64 = 3;

/// TPV's status of a surveyed-in position, a timing receiver's, which
/// is a fix whatever its mode says.
const STATUS_TIME: i64 = 7;

/// Dilutions of precision are compared, and kept, to this, so their
/// last digit wandering is not a row a second.
const DOP_STEP: f64 = 0.1;

/// How often, at most, in seconds, a row is written for satellites and
/// dilutions of precision alone: a satellite at the edge of being used
/// flips the count, and the dilutions with it, every few seconds.  A
/// change of fix mode or status is written at once.
const SKY_EVERY: f64 = 60.0;

/// What gpsd's own trouble, unreachable, is of: what the web view takes
/// the owner of a `gpsd_` event's span to be.
const GPSD: &str = "gpsd";

/// gpsd's address, `host[:port]`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Address(String);

impl FromStr for Address {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() {
            return Err("gpsd's address is empty".to_owned());
        }
        // A bare host, or a bracketed IPv6 address without a port.
        let has_port = match text.rsplit_once(':') {
            Some((host, port)) => {
                port.parse::<u16>().is_ok() && (!host.contains(':') || host.ends_with(']'))
            }
            None => false,
        };
        Ok(Self(if has_port {
            text.to_owned()
        } else {
            format!("{text}:{DEFAULT_PORT}")
        }))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What the reader thread passes on.
#[derive(Debug, Clone, PartialEq)]
enum Message {
    /// Connected and watching.
    Up,
    /// Gone, and why.
    Down(String),
    /// A TPV: a device's fix.
    Tpv {
        device: String,
        mode: i64,
        status: Option<i64>,
    },
    /// A SKY that lists its satellites: a device's satellites used and
    /// seen and its dilutions of precision.
    Sky { device: String, sky: Sky },
}

/// A SKY's figures, as kept.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Sky {
    used: Option<i64>,
    seen: Option<i64>,
    gdop: Option<f64>,
    pdop: Option<f64>,
    hdop: Option<f64>,
    vdop: Option<f64>,
    tdop: Option<f64>,
}

/// The message one line of gpsd's says, if any of interest.
fn message(line: &str) -> Option<Message> {
    let report: Value = serde_json::from_str(line).ok()?;
    let device = report.get("device")?.as_str()?.to_owned();
    let integer = |name: &str| report.get(name).and_then(Value::as_i64);
    let dop = |name: &str| {
        report
            .get(name)
            .and_then(Value::as_f64)
            .map(|dop| (dop / DOP_STEP).round() * DOP_STEP)
    };
    match report.get("class")?.as_str()? {
        "TPV" => Some(Message::Tpv {
            device,
            mode: integer("mode")?,
            status: integer("status"),
        }),
        // A SKY of dilutions alone can disagree with one that lists
        // satellites, so only the latter is taken.
        "SKY" if report.get("satellites").is_some() => {
            let satellites = report.get("satellites").and_then(Value::as_array);
            let counted = |used: bool| {
                satellites.map(|list| {
                    let count = list
                        .iter()
                        .filter(|s| !used || s.get("used").and_then(Value::as_bool) == Some(true))
                        .count();
                    i64::try_from(count).unwrap_or(i64::MAX)
                })
            };
            Some(Message::Sky {
                device,
                sky: Sky {
                    used: integer("uSat").or_else(|| counted(true)),
                    seen: integer("nSat").or_else(|| counted(false)),
                    gdop: dop("gdop"),
                    pdop: dop("pdop"),
                    hdop: dop("hdop"),
                    vdop: dop("vdop"),
                    tdop: dop("tdop"),
                },
            })
        }
        _ => None,
    }
}

/// Watch gpsd at `address` for ever, passing on what it says, and
/// whether it is there.
fn read_gpsd(address: &str, send: &Sender<Message>) {
    loop {
        let why = match watch(address, send) {
            Ok(()) => "gpsd closed the connection".to_owned(),
            Err(e) => e.to_string(),
        };
        if send.send(Message::Down(why)).is_err() {
            return;
        }
        thread::sleep(RECONNECT);
    }
}

/// One connection to gpsd, until it ends.
fn watch(address: &str, send: &Sender<Message>) -> io::Result<()> {
    let mut stream = TcpStream::connect(address)?;
    stream.write_all(WATCH)?;
    let broken = || io::Error::from(io::ErrorKind::BrokenPipe);
    send.send(Message::Up).map_err(|_| broken())?;
    for line in BufReader::new(stream).lines() {
        if let Some(message) = message(&line?) {
            send.send(message).map_err(|_| broken())?;
        }
    }
    Ok(())
}

/// One device as gpsd reports it, poll to poll.
#[derive(Debug, Default)]
struct Device {
    /// Its last TPV's mode and status, none before one.
    tpv: Option<(i64, Option<i64>)>,
    sky: Sky,
    /// When its last TPV came, in seconds.
    tpv_at: f64,
    /// The row last written, and when, in seconds.
    written: Option<Gnss>,
    written_at: f64,
    /// Whether it has a fix, as journaled; none before it is known.
    fix: Option<bool>,
    /// A change of fix seen this many TPVs in a row, not yet journaled.
    pending: Option<(bool, u32)>,
}

impl Device {
    /// Its row, once a TPV has said its mode.
    fn row(&self) -> Option<Gnss> {
        let (mode, status) = self.tpv?;
        Some(Gnss {
            mode,
            status,
            satellites_used: self.sky.used,
            satellites_seen: self.sky.seen,
            gdop: self.sky.gdop,
            pdop: self.sky.pdop,
            hdop: self.sky.hdop,
            vdop: self.sky.vdop,
            tdop: self.sky.tdop,
        })
    }

    /// Take in whether it has a fix, and say what to journal: a change
    /// once `FIX_CONFIRM` readings agree, or, the first time, no fix.
    fn observe_fix(&mut self, fix: bool) -> Option<EventKind> {
        let Some(journaled) = self.fix else {
            self.fix = Some(fix);
            return (!fix).then_some(EventKind::FixLost);
        };
        if fix == journaled {
            self.pending = None;
            return None;
        }
        let count = match self.pending {
            Some((pending, count)) if pending == fix => count + 1,
            _ => 1,
        };
        if count < FIX_CONFIRM {
            self.pending = Some((fix, count));
            return None;
        }
        self.pending = None;
        self.fix = Some(fix);
        Some(if fix {
            EventKind::FixRegained
        } else {
            EventKind::FixLost
        })
    }
}

/// Whether a TPV's mode and status are a fix: a 3D one, or a timing
/// receiver's surveyed-in position.
const fn has_fix(mode: i64, status: Option<i64>) -> bool {
    mode >= MODE_3D || matches!(status, Some(STATUS_TIME))
}

/// gpsd, polled with chronyd.
#[derive(Debug)]
pub(crate) struct Gpsd {
    messages: Receiver<Message>,
    /// Whether gpsd is connected, as journaled; none before it is known.
    up: Option<bool>,
    devices: HashMap<String, Device>,
}

impl Gpsd {
    /// Watch gpsd at `address`.
    pub(crate) fn start(address: &Address) -> io::Result<Self> {
        let (send, messages) = mpsc::channel();
        let address = address.0.clone();
        thread::Builder::new()
            .name("chronymond-gpsd".to_owned())
            .spawn(move || read_gpsd(&address, &send))?;
        Ok(Self {
            messages,
            up: None,
            devices: HashMap::new(),
        })
    }

    /// Take in what gpsd said since the last poll, at `at`, and record
    /// what changed.
    pub(crate) fn poll(
        &mut self,
        batch: &Batch<'_>,
        at: Timestamp,
    ) -> chronymon_log::error::Result<()> {
        let now = seconds(at);
        let mut journal = Vec::new();
        for message in self.messages.try_iter().collect::<Vec<_>>() {
            journal.extend(self.take(now, message));
        }
        if self.up == Some(true) {
            for (name, device) in &mut self.devices {
                if device.tpv.is_some()
                    && now - device.tpv_at > SILENT
                    && let Some(kind) = device.observe_fix(false)
                {
                    journal.push((kind, format!("{name}: no TPV for {SILENT} s")));
                }
            }
        }
        for (kind, detail) in journal {
            eprintln!("chronymond: {}: {detail}", kind.name());
            batch.event(at, kind, None, Some(&detail))?;
        }
        for (name, device) in &mut self.devices {
            let Some(row) = device.row() else {
                continue;
            };
            let fix_changed = device
                .written
                .as_ref()
                .is_none_or(|written| (written.mode, written.status) != (row.mode, row.status));
            let sky_due = now - device.written_at >= SKY_EVERY;
            if device.written.as_ref() != Some(&row) && (fix_changed || sky_due) {
                batch.gnss(at, name, &row)?;
                device.written = Some(row);
                device.written_at = now;
            }
        }
        Ok(())
    }

    /// What is wrong now, as the spans the journal has open: gpsd
    /// unreachable, and each receiver without a fix.
    pub(crate) fn troubles(&self) -> Vec<Trouble> {
        let down = (self.up == Some(false)).then(|| Trouble {
            kind: EventKind::GpsdDown,
            owner: GPSD.to_owned(),
        });
        let mut troubles: Vec<Trouble> = down
            .into_iter()
            .chain(
                self.devices
                    .iter()
                    .filter(|(_, device)| device.fix == Some(false))
                    .map(|(name, _)| Trouble {
                        kind: EventKind::FixLost,
                        owner: name.clone(),
                    }),
            )
            .collect();
        troubles.sort_by(|a, b| a.owner.cmp(&b.owner));
        troubles
    }

    /// Take in one message at `now`, and say what to journal.
    fn take(&mut self, now: f64, message: Message) -> Vec<(EventKind, String)> {
        let mut journal = Vec::new();
        match message {
            Message::Up => {
                if self.up == Some(false) {
                    journal.push((EventKind::GpsdUp, String::new()));
                }
                self.up = Some(true);
                for device in self.devices.values_mut() {
                    device.tpv_at = now;
                }
            }
            Message::Down(why) => {
                if self.up != Some(false) {
                    journal.push((EventKind::GpsdDown, why));
                }
                self.up = Some(false);
            }
            Message::Tpv {
                device: name,
                mode,
                status,
            } => {
                let device = self.devices.entry(name.clone()).or_default();
                device.tpv = Some((mode, status));
                device.tpv_at = now;
                if let Some(kind) = device.observe_fix(has_fix(mode, status)) {
                    let status = status.map_or_else(|| "-".to_owned(), |s| s.to_string());
                    journal.push((kind, format!("{name}: mode {mode}, status {status}")));
                }
            }
            Message::Sky { device, sky } => self.devices.entry(device).or_default().sky = sky,
        }
        journal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::tests::Database;

    const ACM: &str = "/dev/ttyACM0";

    /// apu-0's gpsd 3.25 and u-blox NEO-M8T, cut down.
    const TPV: &str = r#"{"class":"TPV","device":"/dev/ttyACM0","status":7,"mode":3,"time":"2026-10-31T07:59:22.000Z","ept":0.005}"#;
    const SKY: &str = r#"{"class":"SKY","device":"/dev/ttyACM0","gdop":2.06,"hdop":0.87,"pdop":1.81,"tdop":0.97,"vdop":1.59,"nSat":14,"uSat":9,"satellites":[{"PRN":5,"used":true},{"PRN":12,"used":false}]}"#;
    const DOPS_ALONE: &str = r#"{"class":"SKY","device":"/dev/ttyACM0","gdop":0.33,"hdop":0.87,"pdop":1.81,"tdop":0.33,"vdop":1.59}"#;
    const RAW: &str = r#"{"class":"RAW","device":"/dev/ttyACM0","time":1793433579,"nsec":993000000,"rawdata":[]}"#;

    fn tpv(mode: i64, status: Option<i64>) -> Message {
        Message::Tpv {
            device: ACM.to_owned(),
            mode,
            status,
        }
    }

    #[test]
    fn takes_the_generic_reports_only() {
        assert_eq!(message(TPV), Some(tpv(3, Some(7))));
        assert_eq!(
            message(SKY),
            Some(Message::Sky {
                device: ACM.to_owned(),
                sky: Sky {
                    used: Some(9),
                    seen: Some(14),
                    gdop: Some(2.1),
                    pdop: Some(1.8),
                    hdop: Some(0.9),
                    vdop: Some(1.6),
                    tdop: Some(1.0),
                },
            })
        );
        // A SKY of dilutions alone, which disagrees, and a driver's own.
        assert_eq!(message(DOPS_ALONE), None);
        assert_eq!(message(RAW), None);
        assert_eq!(message(r#"{"class":"VERSION","release":"3.25"}"#), None);
        assert_eq!(message("not json"), None);
    }

    #[test]
    fn counts_satellites_when_gpsd_does_not() {
        let sky = r#"{"class":"SKY","device":"/dev/ttyS0","satellites":[{"used":true},{"used":false},{"used":true}]}"#;
        let Some(Message::Sky { sky, .. }) = message(sky) else {
            panic!("a SKY");
        };
        assert_eq!((sky.used, sky.seen), (Some(2), Some(3)));
    }

    #[test]
    fn parses_addresses() {
        let address = |text: &str| text.parse::<Address>().map(|a| a.to_string());
        assert_eq!(address("localhost"), Ok("localhost:2947".to_owned()));
        assert_eq!(address("apu-0:2948"), Ok("apu-0:2948".to_owned()));
        assert_eq!(address("[::1]"), Ok("[::1]:2947".to_owned()));
        assert_eq!(address("[::1]:2948"), Ok("[::1]:2948".to_owned()));
        assert!(address("").is_err());
    }

    #[test]
    fn a_fix_wavering_once_is_not_journaled() {
        let mut device = Device::default();
        assert_eq!(device.observe_fix(true), None);
        assert_eq!(device.observe_fix(false), None);
        assert_eq!(device.observe_fix(true), None);
        assert_eq!(device.observe_fix(false), None);
        assert_eq!(device.observe_fix(false), Some(EventKind::FixLost));
        assert_eq!(device.observe_fix(false), None);
        assert_eq!(device.observe_fix(true), None);
        assert_eq!(device.observe_fix(true), Some(EventKind::FixRegained));
        // No fix when first seen is said so.
        assert_eq!(
            Device::default().observe_fix(false),
            Some(EventKind::FixLost)
        );
    }

    #[test]
    fn a_timing_receivers_surveyed_position_is_a_fix() {
        assert!(has_fix(3, None));
        assert!(has_fix(1, Some(STATUS_TIME)));
        assert!(!has_fix(2, Some(1)));
        assert!(!has_fix(1, None));
    }

    /// A gpsd watcher fed by the test, and the sender feeding it.
    fn fed() -> (Gpsd, Sender<Message>) {
        let (send, messages) = mpsc::channel();
        let gpsd = Gpsd {
            messages,
            up: None,
            devices: HashMap::new(),
        };
        (gpsd, send)
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::new(1_800_000_000 + second, 0).expect("in range")
    }

    fn kinds(db: &Database) -> Vec<String> {
        let mut statement = db
            .reader
            .prepare("SELECT kind FROM event ORDER BY rowid")
            .expect("a query");
        statement
            .query_map([], |row| row.get(0))
            .expect("events")
            .collect::<rusqlite::Result<_>>()
            .expect("events")
    }

    #[test]
    fn the_troubles_are_gpsd_down_and_receivers_without_a_fix() {
        let (mut gpsd, _send) = fed();
        assert!(gpsd.troubles().is_empty());
        gpsd.take(0.0, Message::Down("refused".to_owned()));
        gpsd.take(1.0, Message::Up);
        gpsd.take(1.0, tpv(1, None));
        assert_eq!(
            gpsd.troubles(),
            [Trouble {
                kind: EventKind::FixLost,
                owner: ACM.to_owned()
            }]
        );
        gpsd.take(2.0, Message::Down("refused".to_owned()));
        assert_eq!(
            gpsd.troubles().iter().map(|t| t.kind).collect::<Vec<_>>(),
            [EventKind::FixLost, EventKind::GpsdDown]
        );
    }

    #[test]
    fn records_rows_on_change_and_journals_gpsd_and_the_fix() {
        let mut db = Database::new("gpsd");
        let (mut gpsd, send) = fed();
        let mut poll = |db: &mut Database, second, messages: Vec<Message>| {
            for message in messages {
                send.send(message).expect("sent");
            }
            db.log
                .write(|batch| gpsd.poll(batch, at(second)))
                .expect("polled");
        };
        let sky = || message(SKY).expect("a SKY");
        poll(&mut db, 0, vec![Message::Up, sky(), tpv(3, Some(7))]);
        poll(&mut db, 1, vec![sky(), tpv(3, Some(7))]);
        assert_eq!(db.count("gnss"), 1, "nothing changed");
        assert!(kinds(&db).is_empty());
        // A satellite flipping in and out of use: written once a minute
        // at most, the latest count.
        let flipped = || {
            let Some(Message::Sky { device, sky }) = message(SKY) else {
                panic!("a SKY");
            };
            Message::Sky {
                device,
                sky: Sky {
                    used: Some(10),
                    ..sky
                },
            }
        };
        poll(&mut db, 2, vec![flipped()]);
        poll(&mut db, 3, vec![sky()]);
        poll(&mut db, 4, vec![flipped()]);
        assert_eq!(db.count("gnss"), 1, "within the minute");
        poll(&mut db, 60, vec![flipped()]);
        assert_eq!(db.count("gnss"), 2, "a minute on");
        // The fix lost, confirmed by a second TPV: written at once, within
        // the minute.
        poll(&mut db, 61, vec![tpv(1, None)]);
        poll(&mut db, 62, vec![tpv(1, None)]);
        assert_eq!(db.count("gnss"), 3);
        assert_eq!(kinds(&db), ["fix_lost"]);
        // gpsd gone, then back.
        poll(&mut db, 63, vec![Message::Down("refused".to_owned())]);
        poll(&mut db, 64, vec![Message::Down("refused".to_owned())]);
        poll(
            &mut db,
            65,
            vec![Message::Up, tpv(3, Some(7)), tpv(3, Some(7))],
        );
        assert_eq!(
            kinds(&db),
            ["fix_lost", "gpsd_down", "gpsd_up", "fix_regained"]
        );
        // A device silent too long has no fix.
        poll(&mut db, 90, vec![]);
        poll(&mut db, 91, vec![]);
        assert_eq!(kinds(&db).last().map(String::as_str), Some("fix_lost"));
    }
}
