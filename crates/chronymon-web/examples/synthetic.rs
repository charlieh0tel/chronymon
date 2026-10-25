//! Fill a database with made-up but plausible readings, through the
//! daemon's own writer, so the web view can be looked at without a
//! chronyd: a clock disciplined by a PPS refclock and two NTP servers,
//! its frequency following the day's temperature, with an outage, a
//! restart and a step along the way.
//!
//!     cargo run -p chronymon-web --example synthetic -- DATABASE [HOURS]
//!
//! The readings end now, so the live page shows them as current.

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
use chronymon_log::writer::ChronydState;
use chronymon_log::writer::Log;
use jiff::Timestamp;

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
}

fn feeds() -> [Feed; 3] {
    [
        Feed {
            address: Address::ReferenceId(u32::from_be_bytes(*b"PPS\0")),
            reference_id: u32::from_be_bytes(*b"PPS\0"),
            mode: SourceMode::Refclock,
            name: "PPS",
            bias: 0.0,
            jitter: 2e-7,
            delay: 0.0,
        },
        Feed {
            address: Address::Ip(IpAddr::from([192, 0, 2, 10])),
            reference_id: u32::from_be_bytes([192, 0, 2, 10]),
            mode: SourceMode::Client,
            name: "ntp1.example.net",
            bias: 4e-5,
            jitter: 3e-5,
            delay: 0.012,
        },
        Feed {
            address: Address::Ip(IpAddr::from([198, 51, 100, 20])),
            reference_id: u32::from_be_bytes([198, 51, 100, 20]),
            mode: SourceMode::Client,
            name: "ntp2.example.org",
            bias: -1.2e-4,
            jitter: 8e-5,
            delay: 0.031,
        },
    ]
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("usage: synthetic DATABASE [HOURS]")?);
    let hours = args
        .next()
        .map(|h| h.to_string_lossy().parse::<i64>())
        .transpose()?
        .unwrap_or(DEFAULT_HOURS);
    let mut log = Log::open(&path)?;
    let end = Timestamp::now().as_second();
    let span = hours * 3600;
    let start = end - span;
    let outage = start + (span as f64 * OUTAGE_AT) as i64;
    let step = start + (span as f64 * STEP_AT) as i64;
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
        batch.event(at(start).expect("in range"), EventKind::Boot, None, None)?;
        batch.event(
            at(start).expect("in range"),
            EventKind::DaemonStart,
            None,
            Some("synthetic"),
        )?;
        let mut offset = 0.0_f64;
        let mut valid = vec![0_u32; feeds.len()];
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
            let phase = (second as f64 % DAY) / DAY * TAU;
            let frequency = -12.4 + 0.35 * phase.sin() + noise.normal(0.002);
            offset = 0.6 * offset + noise.normal(3e-7);
            let stepped = (step..step + UPDATE_EVERY).contains(&second);
            if stepped {
                offset = 0.18;
                batch.event(when, EventKind::Step, None, Some("0.18 s"))?;
            }
            batch.tracking(
                when,
                &Tracking {
                    reference_id: ReferenceId(feeds[0].reference_id),
                    address: feeds[0].address,
                    stratum: 1,
                    leap: Leap::Normal,
                    reference_time: when,
                    current_correction: Seconds::new(offset * 0.5),
                    last_offset: Seconds::new(offset),
                    rms_offset: Seconds::new(4e-7 + offset.abs() * 0.1),
                    frequency: Ppm::new(frequency),
                    residual_frequency: Ppm::new(noise.normal(0.001)),
                    skew: Ppm::new(0.004 + noise.uniform() * 0.002),
                    root_delay: Seconds::new(1e-9),
                    root_dispersion: Seconds::new(1.5e-5 + noise.uniform() * 5e-6),
                    last_update_interval: Seconds::new(UPDATE_EVERY as f64),
                },
            )?;
            for (n, f) in feeds.iter().enumerate() {
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
                    state: if n == 0 {
                        SourceState::Selected
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
                        state: if n == 0 { '*' } else { '-' },
                        authenticated: false,
                        leap: Leap::Normal,
                        configured_options: SelectOptions {
                            prefer: n == 0,
                            ..SelectOptions::default()
                        },
                        effective_options: SelectOptions {
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
        for f in &feeds {
            batch.source(
                at(end).expect("in range"),
                f.address,
                ReferenceId(f.reference_id),
                f.mode,
                Some(f.name),
            )?;
        }
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
