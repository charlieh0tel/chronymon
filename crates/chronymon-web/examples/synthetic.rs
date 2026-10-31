//! Fill a database with made-up but plausible readings, through the
//! daemon's own writer, so the web view can be looked at without a
//! chronyd: a clock disciplined by a PPS refclock and six NTP servers,
//! one with a long name, as many sources as a kiosk must hold, its
//! frequency following the day's temperature, with an outage, a
//! restart, a step and a time unsynchronized, then on the local
//! reference, along the way, a seventh server removed, and a
//! `noselect` NMEA refclock whose samples stop,
//! the PPS locked to it (`refclock ... lock NMEA`), as the daemon
//! records chronyd's configuration; and chronyd's logs as
//! the daemon reads them: refclock and tempcomp samples throughout,
//! and tracking for some hours before the daemon started, for the
//! charts to fill from.  Later, chronyd restarted with tracking.log on and a 64 s
//! update interval, a row only every few buckets, the daemon stopped
//! for a while and then recording beside the log: the log fills the
//! charts only while the daemon was stopped.
//!
//!     cargo run -p chronymon-web --example synthetic -- DATABASE [HOURS [HOST]]
//!
//! The readings end now, so the live page shows them as current.  HOST
//! says how they end, as a kind of host the pages must read right:
//! `pps` (the default) synchronized to its PPS; `ntp`, its refclocks
//! gone and its selected server, the long-named one, missing its last
//! 4 polls of 512 s, half an hour since an update, which is not
//! coasting; `gpsd-stopped`, gpsd down and the PPS locked to its NMEA
//! without samples, chronyd keeping it selected, coasting; `backups`,
//! just started, an NTP server selected before the PPS.

use std::env;
use std::error::Error;
use std::f64::consts::TAU;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use chronymon::event::EventKind;
use chronymon::units::Ppm;
use chronymon::units::Seconds;
use chronymon_control::report::Address;
use chronymon_control::report::Leap;
use chronymon_control::report::NtpData;
use chronymon_control::report::ReferenceId;
use chronymon_control::report::SelectData;
use chronymon_control::report::SelectOptions;
use chronymon_control::report::SourceData;
use chronymon_control::report::SourceMode;
use chronymon_control::report::SourceState;
use chronymon_control::report::SourceStats;
use chronymon_control::report::Tracking;
use chronymon_log::chrony_log::Line;
use chronymon_log::chrony_log::RefclockLine;
use chronymon_log::chrony_log::TempcompLine;
use chronymon_log::chrony_log::TrackingLine;
use chronymon_log::schema::RefclockLock;
use chronymon_log::schema::Trouble;
use chronymon_log::writer::Batch;
use chronymon_log::writer::ChronydState;
use chronymon_log::writer::Gnss;
use chronymon_log::writer::Log;
use jiff::Timestamp;

/// How the readings end: the kind of host, at the end of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    Pps,
    Ntp,
    GpsdStopped,
    Backups,
}

impl Host {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "pps" => Some(Self::Pps),
            "ntp" => Some(Self::Ntp),
            "gpsd-stopped" => Some(Self::GpsdStopped),
            "backups" => Some(Self::Backups),
            _ => None,
        }
    }
}

/// The `ntp` host's poll of its selected server, in seconds, and its
/// reachability, the last 4 polls lost.
const NTP_HOST_POLL: i64 = 512;
const NTP_HOST_REACH: u16 = 0o360;
/// Seconds since the clock's last update, on the `ntp` host and on the
/// `gpsd-stopped` one.
const NTP_HOST_QUIET: i64 = 1800;
const GPSD_STOPPED_FOR: i64 = 600;
/// The `backups` host's reachability of its PPS, its first 4 polls in.
const BACKUPS_PPS_REACH: u16 = 0o17;

/// Hours of readings when none are asked for.
const DEFAULT_HOURS: i64 = 48;
/// Seconds between clock updates, and between each server's samples.
const UPDATE_EVERY: i64 = 16;
const POLL_EVERY: i64 = 64;
/// A day, for the temperature's cycle.
const DAY: f64 = 86_400.0;
/// When, as a fraction of the span, chronyd is down and for how long,
/// and when the clock steps.
const OUTAGE_AT: f64 = 0.4;
const OUTAGE_FOR: i64 = 1800;
const STEP_AT: f64 = 0.7;
/// When, likewise, chronyd loses sync, how long it is unsynchronized,
/// and then on its local reference, in seconds: before the last six
/// hours, which the README's screenshots show.
const UNSYNCHRONIZED_AT: f64 = 0.6;
const UNSYNCHRONIZED_FOR: i64 = 50 * UPDATE_EVERY;
const LOCAL_FOR: i64 = 100 * UPDATE_EVERY;
/// `local`'s default stratum (chrony 4.8 `chrony.conf.adoc`).
const LOCAL_STRATUM: u16 = 10;
/// When, likewise, the removed server goes, and the NMEA refclock's
/// samples stop: both within the last day.
const REMOVED_AT: f64 = 0.85;
const SILENT_AT: f64 = 0.9;
/// Hours of `tracking.log` from before the daemon's first poll.
const LOG_ONLY_HOURS: i64 = 6;
/// When, as a fraction of the span, chronyd restarts with tracking.log
/// on and updates the clock every `SLOW_UPDATE_EVERY` s, the daemon
/// stopped for its first `SLOW_STOPPED_FOR` s; and when the clock is
/// back to updating every `UPDATE_EVERY` s.  Before the last six hours,
/// which the README's screenshots show.
const SLOW_AT: f64 = 0.78;
const SLOW_UNTIL: f64 = 0.86;
const SLOW_UPDATE_EVERY: i64 = 64;
const SLOW_STOPPED_FOR: i64 = 1200;
/// Seconds a clock update's reference time, its sample's, is before
/// chronyd logs the update: so the log and the daemon's rows are in
/// different buckets.
const REFERENCE_LAG: i64 = 23;
/// Degrees of the tempcomp sensor's daily swing about its mean.
const TEMPERATURE_MEAN: f64 = 41.0;
const TEMPERATURE_SWING: f64 = 4.0;
/// A gpsd receiver: a row this often, in seconds, while recording, on
/// the loop's grid, and its fix lost this far through the range for so
/// long.
const GNSS_EVERY: i64 = 4 * UPDATE_EVERY;
const GNSS_DEVICE: &str = "/dev/ttyACM0";
const FIX_LOST_AT: f64 = 0.3;
const FIX_LOST_FOR: i64 = 10 * GNSS_EVERY;
/// Its satellites in view, and how many of them are used at most.
const SATELLITES_SEEN: i64 = 14;
const SATELLITES_USED: f64 = 9.0;
/// gpsd's TPV mode of a 3D fix and of none, and status of a
/// surveyed-in position.
const MODE_3D: i64 = 3;
const MODE_NO_FIX: i64 = 1;
const STATUS_TIME: i64 = 7;

/// A small, seeded generator: the same database every run.
struct Noise(u64);

impl Noise {
    /// Uniform in [0, 1).
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Normal, mean 0, deviation `sigma`.
    fn normal(&mut self, sigma: f64) -> f64 {
        let (u, v) = (self.uniform().max(1e-12), self.uniform());
        sigma * (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }
}

/// One source the clock is fed from.
struct Feed {
    address: Address,
    reference_id: u32,
    mode: SourceMode,
    name: &'static str,
    /// Typical offset and its noise, in seconds.
    bias: f64,
    jitter: f64,
    /// Round trip, for an NTP server.
    delay: f64,
    /// When, as a fraction of the span, chronyd removes it.
    removed: Option<f64>,
    /// When, likewise, its samples stop, and chronyd has none.
    silent: Option<f64>,
    /// Whether it has the `noselect` option.
    noselect: bool,
}

impl Feed {
    /// Whether it still has samples at `second` of a span from `start`.
    fn sampled(&self, second: i64, start: i64, span: i64) -> bool {
        [self.removed, self.silent]
            .into_iter()
            .flatten()
            .all(|at| second < start + (span as f64 * at) as i64)
    }
}

fn feeds() -> [Feed; 9] {
    [
        Feed {
            address: Address::ReferenceId(u32::from_be_bytes(*b"PPS\0")),
            reference_id: u32::from_be_bytes(*b"PPS\0"),
            mode: SourceMode::Refclock,
            name: "PPS",
            bias: 0.0,
            jitter: 2e-7,
            delay: 0.0,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::ReferenceId(u32::from_be_bytes(*b"NMEA")),
            reference_id: u32::from_be_bytes(*b"NMEA"),
            mode: SourceMode::Refclock,
            name: "NMEA",
            bias: 0.043,
            jitter: 2e-3,
            delay: 0.0,
            removed: None,
            silent: Some(SILENT_AT),
            noselect: true,
        },
        Feed {
            address: Address::Ip(IpAddr::from([192, 0, 2, 10])),
            reference_id: u32::from_be_bytes([192, 0, 2, 10]),
            mode: SourceMode::Client,
            name: "ntp1.example.net",
            bias: 4e-5,
            jitter: 3e-5,
            delay: 0.012,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::Ip(IpAddr::from([198, 51, 100, 20])),
            reference_id: u32::from_be_bytes([198, 51, 100, 20]),
            mode: SourceMode::Client,
            name: "ntp2.example.org",
            bias: -1.2e-4,
            jitter: 8e-5,
            delay: 0.031,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::Ip(IpAddr::from([192, 0, 2, 11])),
            reference_id: u32::from_be_bytes([192, 0, 2, 11]),
            mode: SourceMode::Client,
            name: "ntp4.example.net",
            bias: 2e-5,
            jitter: 4e-5,
            delay: 0.018,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::Ip(IpAddr::from([198, 51, 100, 21])),
            reference_id: u32::from_be_bytes([198, 51, 100, 21]),
            mode: SourceMode::Client,
            name: "time-a.long-hostname.example.org",
            bias: -6e-5,
            jitter: 1e-4,
            delay: 0.042,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::Ip(IpAddr::from([203, 0, 113, 31])),
            reference_id: u32::from_be_bytes([203, 0, 113, 31]),
            mode: SourceMode::Client,
            name: "b.ntp.example.com",
            bias: 9e-5,
            jitter: 6e-5,
            delay: 0.025,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::Ip(IpAddr::from([192, 0, 2, 12])),
            reference_id: u32::from_be_bytes([192, 0, 2, 12]),
            mode: SourceMode::Client,
            name: "clock.example.edu",
            bias: -3e-5,
            jitter: 5e-5,
            delay: 0.021,
            removed: None,
            silent: None,
            noselect: false,
        },
        Feed {
            address: Address::Ip(IpAddr::from([203, 0, 113, 30])),
            reference_id: u32::from_be_bytes([203, 0, 113, 30]),
            mode: SourceMode::Client,
            name: "ntp3.example.com",
            bias: 2.5e-4,
            jitter: 1.5e-4,
            delay: 0.054,
            removed: Some(REMOVED_AT),
            silent: None,
            noselect: false,
        },
    ]
}

/// A `tracking.log` line at `second`.
fn logged(second: i64, frequency: f64, noise: &mut Noise) -> Line {
    Line::Tracking(TrackingLine {
        at: Timestamp::new(second, 0).expect("in range"),
        reference: "PPS".to_owned(),
        stratum: 1,
        frequency,
        skew: 0.004 + noise.uniform() * 0.002,
        offset: noise.normal(3e-7),
        leap: 'N',
        combined_sources: 1,
        offset_sd: 3e-7,
        remaining_correction: noise.normal(1e-7),
        root_delay: 1e-9,
        root_dispersion: 1.5e-5 + noise.uniform() * 5e-6,
        max_error: 2e-5,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let usage = "usage: synthetic DATABASE [HOURS [pps|ntp|gpsd-stopped|backups]]";
    let path = PathBuf::from(args.next().ok_or(usage)?);
    let hours = args
        .next()
        .map(|h| h.to_string_lossy().parse::<i64>())
        .transpose()?
        .unwrap_or(DEFAULT_HOURS);
    let host = args
        .next()
        .map(|h| Host::parse(&h.to_string_lossy()).ok_or(usage))
        .transpose()?
        .unwrap_or(Host::Pps);
    let mut log = Log::open(&path)?;
    let end = Timestamp::now().as_second();
    let span = hours * 3600;
    let start = end - span;
    let outage = start + (span as f64 * OUTAGE_AT) as i64;
    let step = start + (span as f64 * STEP_AT) as i64;
    let lost = start + (span as f64 * UNSYNCHRONIZED_AT) as i64 / UPDATE_EVERY * UPDATE_EVERY;
    let local = lost + UNSYNCHRONIZED_FOR;
    let regained = local + LOCAL_FOR;
    // On the grid of updates, so that the loop below meets them.
    let on_grid = |fraction: f64| {
        let after = (span as f64 * fraction) as i64;
        start + after / SLOW_UPDATE_EVERY * SLOW_UPDATE_EVERY
    };
    let fix_lost = start + (span as f64 * FIX_LOST_AT) as i64 / GNSS_EVERY * GNSS_EVERY;
    let fix_regained = fix_lost + FIX_LOST_FOR;
    let slow = on_grid(SLOW_AT);
    let resumed = slow + SLOW_STOPPED_FOR;
    let fast = on_grid(SLOW_UNTIL);
    let mut noise = Noise(0x2545_f491_4f6c_dd1d);
    let feeds = feeds();
    let at = |second: i64| Timestamp::new(second, 0);

    log.write(|batch| {
        let ids: Vec<i64> = feeds
            .iter()
            .map(|f| {
                batch.source(
                    at(start).expect("in range"),
                    f.address,
                    ReferenceId(f.reference_id),
                    f.mode,
                    Some(f.name),
                )
            })
            .collect::<Result<_, _>>()?;
        // The PPS's pulses numbered by the NMEA's time of day.
        batch.refclock_locks(&[RefclockLock {
            refclock: ReferenceId(feeds[0].reference_id),
            locked_to: ReferenceId(feeds[1].reference_id),
        }])?;
        batch.event(at(start).expect("in range"), EventKind::Boot, None, None)?;
        batch.event(
            at(start).expect("in range"),
            EventKind::DaemonStart,
            None,
            Some("synthetic"),
        )?;
        // A placeholder for a name not resolved yet, as the daemon once
        // stored one (`IPADDR_ID` 1, in hex) and its removal: never shown.
        let placeholder = batch.source(
            at(start).expect("in range"),
            Address::ReferenceId(1),
            ReferenceId(0),
            SourceMode::Client,
            None,
        )?;
        batch.event(
            at(start).expect("in range"),
            EventKind::SourceRemoved,
            Some(placeholder),
            Some("00000001"),
        )?;
        for second in (start - LOG_ONLY_HOURS * 3600..start).step_by(UPDATE_EVERY as usize) {
            let phase = (second as f64 % DAY) / DAY * TAU;
            let frequency = -12.4 + 0.35 * phase.sin() + noise.normal(0.002);
            batch.log_line(&logged(second, frequency, &mut noise))?;
        }
        let mut offset = 0.0_f64;
        let mut valid = vec![0_u32; feeds.len()];
        // Each feed's last `sources` report, and when it was removed.
        let mut reports: Vec<Option<SourceData>> = vec![None; feeds.len()];
        let mut removed: Vec<Option<i64>> = vec![None; feeds.len()];
        let mut down = false;
        for second in (start..=end).step_by(UPDATE_EVERY as usize) {
            let when = at(second).expect("in range");
            if (outage..outage + OUTAGE_FOR).contains(&second) {
                if !down {
                    batch.event(
                        when,
                        EventKind::ChronydDown,
                        None,
                        Some("io: Connection refused"),
                    )?;
                }
                down = true;
                continue;
            }
            if down {
                down = false;
                batch.event(when, EventKind::ChronydUp, None, None)?;
                batch.event(
                    when,
                    EventKind::ChronydRestart,
                    None,
                    Some("pid 812 to 1907"),
                )?;
            }
            if second == slow {
                batch.event(when, EventKind::DaemonStop, None, None)?;
            }
            if second == resumed {
                batch.event(when, EventKind::DaemonStart, None, Some("synthetic"))?;
            }
            let recording = !(slow..resumed).contains(&second);
            let updated =
                !(slow..fast).contains(&second) || (second - slow) % SLOW_UPDATE_EVERY == 0;
            let phase = (second as f64 % DAY) / DAY * TAU;
            let frequency = -12.4 + 0.35 * phase.sin() + noise.normal(0.002);
            if updated && second >= slow {
                batch.log_line(&logged(second, frequency, &mut noise))?;
            }
            let temperature = TEMPERATURE_MEAN + TEMPERATURE_SWING * phase.sin();
            batch.log_line(&Line::Tempcomp(TempcompLine {
                at: when,
                temperature: temperature + noise.normal(0.05),
                compensation: (temperature - TEMPERATURE_MEAN) * 0.08,
            }))?;
            for (reference, bias, jitter) in [("PPS", 0.0, 2e-8), ("NMEA", 0.043, 2e-3)] {
                let feed = feeds.iter().find(|f| f.name == reference);
                if feed.is_some_and(|f| !f.sampled(second, start, span)) {
                    continue;
                }
                let cooked = bias + noise.normal(jitter) + offset;
                batch.log_line(&Line::Refclock(RefclockLine {
                    at: when,
                    reference: reference.to_owned(),
                    driver_polls: Some(0),
                    leap: 'N',
                    pulse: Some(reference == "PPS"),
                    raw_offset: Some(cooked + noise.normal(jitter)),
                    cooked_offset: cooked,
                    dispersion: jitter,
                }))?;
            }
            if !recording {
                continue;
            }
            if (second - start) % GNSS_EVERY == 0 {
                let fix = !(fix_lost..fix_regained).contains(&second);
                if second == fix_lost {
                    batch.event(
                        when,
                        EventKind::FixLost,
                        None,
                        Some(&format!("{GNSS_DEVICE}: mode {MODE_NO_FIX}, status -")),
                    )?;
                }
                if second == fix_regained {
                    batch.event(
                        when,
                        EventKind::FixRegained,
                        None,
                        Some(&format!(
                            "{GNSS_DEVICE}: mode {MODE_3D}, status {STATUS_TIME}"
                        )),
                    )?;
                }
                let used = (SATELLITES_USED - 2.0 * phase.cos().abs()).round() as i64;
                let dop = |base: f64| ((base + 0.2 * phase.sin()) * 10.0).round() / 10.0;
                batch.gnss(
                    when,
                    GNSS_DEVICE,
                    &Gnss {
                        mode: if fix { MODE_3D } else { MODE_NO_FIX },
                        status: fix.then_some(STATUS_TIME),
                        satellites_used: Some(if fix { used } else { 0 }),
                        satellites_seen: Some(SATELLITES_SEEN),
                        gdop: Some(dop(2.1)),
                        pdop: Some(dop(1.8)),
                        hdop: Some(dop(0.9)),
                        vdop: Some(dop(1.6)),
                        tdop: Some(dop(1.0)),
                    },
                )?;
            }
            offset = 0.6 * offset + noise.normal(3e-7);
            let stepped = (step..step + UPDATE_EVERY).contains(&second);
            if stepped {
                offset = 0.18;
                batch.event(when, EventKind::Step, None, Some("0.18 s"))?;
            }
            let synchronized = !(lost..regained).contains(&second);
            let interval = if (slow..fast).contains(&second) {
                SLOW_UPDATE_EVERY
            } else {
                UPDATE_EVERY
            };
            let tracking = Tracking {
                reference_id: ReferenceId(feeds[0].reference_id),
                // chronyd's tracking leaves a refclock's address
                // unspecified.
                address: Address::Unspecified,
                stratum: 1,
                leap: Leap::Normal,
                reference_time: at(second - REFERENCE_LAG).expect("in range"),
                current_correction: Seconds::new(offset * 0.5),
                last_offset: Seconds::new(offset),
                rms_offset: Seconds::new(4e-7 + offset.abs() * 0.1),
                frequency: Ppm::new(frequency),
                residual_frequency: Ppm::new(noise.normal(0.001)),
                skew: Ppm::new(0.004 + noise.uniform() * 0.002),
                root_delay: Seconds::new(1e-9),
                root_dispersion: Seconds::new(1.5e-5 + noise.uniform() * 5e-6),
                last_update_interval: Seconds::new(interval as f64),
            };
            // As the daemon records it: a leap change to unsynchronized
            // and back, then on the local reference, a reference change
            // each time, and no update between (`REF_SetUnsynchronised`,
            // chrony 4.8 `reference.c:1078`; `REF_GetReferenceParams`,
            // `reference.c:1184`).
            let unsynchronized = Tracking {
                reference_id: ReferenceId(0),
                stratum: 0,
                leap: Leap::Unsynchronized,
                reference_time: Timestamp::UNIX_EPOCH,
                ..tracking
            };
            let on_local = Tracking {
                reference_id: ReferenceId::LOCAL,
                stratum: LOCAL_STRATUM,
                ..tracking
            };
            if second == lost {
                batch.leap_change(when, Leap::Normal, Leap::Unsynchronized)?;
                batch.reference_change(when, &tracking, &unsynchronized)?;
            }
            if second == local {
                batch.leap_change(when, Leap::Unsynchronized, Leap::Normal)?;
                batch.reference_change(when, &unsynchronized, &on_local)?;
            }
            if second == regained {
                batch.reference_change(when, &on_local, &tracking)?;
            }
            if updated && synchronized {
                batch.tracking(when, &tracking)?;
                batch.tracking_report(when, &tracking)?;
            }
            for (n, f) in feeds.iter().enumerate() {
                if removed[n].is_none() && f.removed.is_some() && !f.sampled(second, start, span) {
                    removed[n] = Some(second);
                    batch.event(
                        when,
                        EventKind::SourceRemoved,
                        Some(ids[n]),
                        Some(&f.address.to_string()),
                    )?;
                }
                if !f.sampled(second, start, span) {
                    continue;
                }
                let every = if f.mode == SourceMode::Refclock {
                    UPDATE_EVERY
                } else {
                    POLL_EVERY
                };
                if (second - start) % every != 0 {
                    continue;
                }
                valid[n] += 1;
                let measured = f.bias + noise.normal(f.jitter) + offset;
                let delay = f.delay * (1.0 + noise.uniform() * 0.3);
                let data = SourceData {
                    address: f.address,
                    poll: if f.mode == SourceMode::Refclock { 4 } else { 6 },
                    stratum: if f.mode == SourceMode::Refclock { 0 } else { 1 },
                    // chronyd never selects a `noselect` source, and
                    // reports it as not selectable (`sources.c`, 4.9).
                    state: if n == 0 {
                        SourceState::Selected
                    } else if f.noselect {
                        SourceState::Nonselectable
                    } else {
                        SourceState::NotCombined
                    },
                    mode: f.mode,
                    reachability: 0o377,
                    since_sample: Some(Duration::ZERO),
                    original_latest_measurement: Seconds::new(measured),
                    latest_measurement: Seconds::new(measured),
                    latest_measurement_error: Seconds::new(f.jitter + delay / 2.0),
                };
                batch.source_sample(when, ids[n], &data)?;
                reports[n] = Some(data);
                batch.source_stats(
                    when,
                    ids[n],
                    &SourceStats {
                        reference_id: ReferenceId(f.reference_id),
                        address: f.address,
                        samples: 16 + valid[n] % 8,
                        runs: 6,
                        span: Duration::from_secs((16 * every) as u64),
                        standard_deviation: Seconds::new(f.jitter * (0.8 + noise.uniform() * 0.4)),
                        residual_frequency: Ppm::new(noise.normal(f.jitter * 1e3)),
                        skew: Ppm::new(f.jitter * 2e3 * (0.5 + noise.uniform())),
                        estimated_offset: Seconds::new(f.bias + noise.normal(f.jitter / 4.0)),
                        estimated_offset_error: Seconds::new(f.jitter / 3.0),
                    },
                )?;
                batch.selection(
                    when,
                    ids[n],
                    &SelectData {
                        reference_id: ReferenceId(f.reference_id),
                        address: f.address,
                        state: if n == 0 {
                            '*'
                        } else if f.noselect {
                            'N'
                        } else {
                            '-'
                        },
                        authenticated: false,
                        leap: Leap::Normal,
                        configured_options: SelectOptions {
                            noselect: f.noselect,
                            prefer: n == 0,
                            ..SelectOptions::default()
                        },
                        effective_options: SelectOptions {
                            noselect: f.noselect,
                            prefer: n == 0,
                            ..SelectOptions::default()
                        },
                        last_sample_ago: Duration::ZERO,
                        score: if n == 0 {
                            1.0
                        } else {
                            2.0 + noise.uniform() * 3.0
                        },
                        low_limit: Seconds::new(measured - f.jitter * 3.0),
                        high_limit: Seconds::new(measured + f.jitter * 3.0),
                    },
                )?;
                if f.mode == SourceMode::Client {
                    batch.ntp_sample(
                        when,
                        ids[n],
                        &NtpData {
                            remote_address: f.address,
                            local_address: Address::Ip(IpAddr::from([192, 0, 2, 1])),
                            remote_port: 123,
                            leap: 0,
                            version: 4,
                            mode: 4,
                            stratum: 1,
                            poll: 6,
                            precision: -24,
                            root_delay: Seconds::new(1e-5),
                            root_dispersion: Seconds::new(2e-5),
                            reference_id: ReferenceId(u32::from_be_bytes(*b"GPS\0")),
                            reference_time: when,
                            offset: Seconds::new(measured),
                            peer_delay: Seconds::new(delay),
                            peer_dispersion: Seconds::new(f.jitter / 2.0),
                            response_time: Seconds::new(2e-5),
                            jitter_asymmetry: 0.0,
                            tests: 0x3ff,
                            interleaved: false,
                            authenticated: false,
                            transmit_timestamp_source: 'K',
                            receive_timestamp_source: 'K',
                            total_transmitted: valid[n],
                            total_received: valid[n],
                            total_valid: valid[n],
                            total_good: valid[n],
                            timestamps: None,
                        },
                    )?;
                }
            }
        }
        for (n, f) in feeds.iter().enumerate() {
            let seen = at(removed[n].unwrap_or(end)).expect("in range");
            batch.source(
                seen,
                f.address,
                ReferenceId(f.reference_id),
                f.mode,
                Some(f.name),
            )?;
            let Some(last) = reports[n] else {
                continue;
            };
            // What chronyd reports of a source whose samples stopped:
            // reach run down to 0, and no sample (`SST_DoSourceReport`).
            let report = match f.silent {
                Some(_) => SourceData {
                    state: SourceState::Nonselectable,
                    stratum: 0,
                    reachability: 0,
                    since_sample: None,
                    ..last
                },
                None => last,
            };
            batch.source_report(seen, ids[n], &report)?;
        }
        end_as(batch, host, end, &feeds, &ids, &reports)?;
        batch.status(
            at(end).expect("in range"),
            ChronydState::Up,
            None,
            Some(1907),
        )?;
        Ok(())
    })?;
    eprintln!(
        "wrote {hours} h of synthetic readings to {}",
        path.display()
    );
    Ok(())
}

/// End the readings as `host` would: its sources' last reports, the
/// clock's last update, and what the watchers find, at `end`, over
/// what the run wrote.  `reports` is each feed's last.
fn end_as(
    batch: &Batch<'_>,
    host: Host,
    end: i64,
    feeds: &[Feed],
    ids: &[i64],
    reports: &[Option<SourceData>],
) -> chronymon_log::error::Result<()> {
    let at = |second: i64| Timestamp::new(second, 0).expect("in range");
    let index = |name: &str| feeds.iter().position(|f| f.name == name).expect("a feed");
    let (pps, nmea) = (index("PPS"), index("NMEA"));
    // Each source's report as it ends: `selected` chronyd's choice, and
    // a reachability for some.
    let report = |selected: usize, reach: &[(usize, u16)]| -> chronymon_log::error::Result<()> {
        for (n, last) in reports.iter().enumerate() {
            let Some(last) = last else { continue };
            let state = if n == selected {
                SourceState::Selected
            } else if feeds[n].noselect {
                SourceState::Nonselectable
            } else {
                SourceState::NotCombined
            };
            let reachability = reach
                .iter()
                .find(|(r, _)| *r == n)
                .map_or(last.reachability, |(_, reach)| *reach);
            let state = if reachability == 0 && n != selected {
                SourceState::Nonselectable
            } else {
                state
            };
            batch.source_report(
                at(end),
                ids[n],
                &SourceData {
                    state,
                    reachability,
                    ..*last
                },
            )?;
        }
        Ok(())
    };
    // The clock's last update, `quiet` seconds ago, from `feed`.
    let tracking = |feed: &Feed, stratum: u16, quiet: i64, interval: i64| Tracking {
        reference_id: ReferenceId(feed.reference_id),
        address: match feed.mode {
            SourceMode::Refclock => Address::Unspecified,
            _ => feed.address,
        },
        stratum,
        leap: Leap::Normal,
        reference_time: at(end - quiet),
        current_correction: Seconds::new(1e-6),
        last_offset: Seconds::new(2e-6),
        rms_offset: Seconds::new(4e-5),
        frequency: Ppm::new(-12.4),
        residual_frequency: Ppm::new(0.001),
        skew: Ppm::new(0.05),
        root_delay: Seconds::new(0.02),
        root_dispersion: Seconds::new(0.004),
        last_update_interval: Seconds::new(interval as f64),
    };
    match host {
        Host::Pps => Ok(()),
        Host::Ntp => {
            let selected = index("time-a.long-hostname.example.org");
            for n in [pps, nmea] {
                batch.event(
                    at(end - NTP_HOST_QUIET * 2),
                    EventKind::SourceRemoved,
                    Some(ids[n]),
                    Some(&feeds[n].address.to_string()),
                )?;
            }
            report(selected, &[(selected, NTP_HOST_REACH)])?;
            batch.tracking_report(
                at(end),
                &tracking(&feeds[selected], 2, NTP_HOST_QUIET, NTP_HOST_POLL),
            )
        }
        Host::GpsdStopped => {
            batch.event(
                at(end - GPSD_STOPPED_FOR),
                EventKind::GpsdDown,
                None,
                Some("gpsd closed the connection"),
            )?;
            batch.troubles(&[Trouble {
                kind: EventKind::GpsdDown,
                owner: "gpsd".to_owned(),
            }])?;
            report(pps, &[(pps, 0), (nmea, 0)])?;
            batch.tracking_report(
                at(end),
                &tracking(&feeds[pps], 1, GPSD_STOPPED_FOR, UPDATE_EVERY),
            )
        }
        Host::Backups => {
            let selected = index("ntp1.example.net");
            report(selected, &[(pps, BACKUPS_PPS_REACH)])?;
            batch.tracking_report(
                at(end),
                &tracking(&feeds[selected], 2, UPDATE_EVERY, POLL_EVERY),
            )
        }
    }
}
