//! chronyd's own log files: `tracking`, `measurements`, `statistics`,
//! `refclocks` and `tempcomp`, as chrony 4.3 to 4.9 write them
//! (`LOG_FileWrite` calls in `reference.c`, `ntp_core.c`,
//! `sourcestats.c`, `refclock.c`, `tempcomp.c`; the same in every one
//! of those versions).
//!
//! A log is known by its header, which chronyd repeats between `=`
//! rules every `logbanner` lines.  Its column names cannot be split
//! from one another (`Date (UTC) Time`, `Peer del.`), so the header is
//! matched whole, and a file with a header this does not know -- a
//! chrony that has changed the layout -- is refused rather than read
//! by guesswork.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

/// The logs chronymon reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogKind {
    /// `tracking.log`: one line per clock update.
    Tracking,
    /// `measurements.log`: one line per NTP packet.
    Measurements,
    /// `statistics.log`: one line per regression over a source's
    /// samples.
    Statistics,
    /// `refclocks.log`: one line per refclock sample.
    Refclocks,
    /// `tempcomp.log`: one line per temperature compensation.
    Tempcomp,
}

impl LogKind {
    /// Every kind.
    pub const ALL: [Self; 5] = [
        Self::Tracking,
        Self::Measurements,
        Self::Statistics,
        Self::Refclocks,
        Self::Tempcomp,
    ];

    /// The file's name without `.log`, as chronyd names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tracking => "tracking",
            Self::Measurements => "measurements",
            Self::Statistics => "statistics",
            Self::Refclocks => "refclocks",
            Self::Tempcomp => "tempcomp",
        }
    }

    /// The header chronyd writes, verbatim.
    const fn header(self) -> &'static str {
        match self {
            Self::Tracking => {
                "   Date (UTC) Time     IP Address   St   Freq ppm   Skew ppm     Offset L Co  Offset sd Rem. corr. Root delay Root disp. Max. error"
            }
            Self::Measurements => {
                "   Date (UTC) Time     IP Address   L St 123 567 ABCD  LP RP Score    Offset  Peer del. Peer disp.  Root del. Root disp. Refid     MTxRx"
            }
            Self::Statistics => {
                "   Date (UTC) Time     IP Address    Std dev'n Est offset  Offset sd  Diff freq   Est skew  Stress  Ns  Bs  Nr  Asym"
            }
            Self::Refclocks => {
                "   Date (UTC) Time         Refid  DP L P  Raw offset   Cooked offset      Disp."
            }
            Self::Tempcomp => "   Date (UTC) Time        Temp.       Comp.",
        }
    }

    /// The kind of a log file, by its name: `tracking.log`, and as
    /// logrotate leaves it, `tracking.log.1`, `tracking.log.2.gz`,
    /// `tracking.log-20261024.gz`.
    pub fn of_file(name: &str) -> Option<Self> {
        let (stem, rest) = name.split_once(".log")?;
        let rotated = rest.is_empty() || rest.starts_with('.') || rest.starts_with('-');
        Self::ALL
            .into_iter()
            .find(|kind| rotated && kind.name() == stem)
    }
}

/// A leap status as the logs write it: `N`, `+`, `-` or `?`.
pub type LeapCode = char;

/// A line of `tracking.log`.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingLine {
    /// When the clock was updated, to the second.
    pub at: Timestamp,
    /// The reference: its address, or a refclock's reference ID.
    pub reference: String,
    /// The local stratum.
    pub stratum: u8,
    /// The clock's frequency, ppm, as `tracking` reports it.
    pub frequency: f64,
    /// Its error bound, ppm.
    pub skew: f64,
    /// The offset measured, seconds.
    pub offset: f64,
    /// The leap status.
    pub leap: LeapCode,
    /// Sources combined in the update.
    pub combined_sources: u32,
    /// The offset's estimated standard deviation, seconds.
    pub offset_sd: f64,
    /// Correction remaining from earlier updates, seconds.
    pub remaining_correction: f64,
    /// Root delay, seconds.
    pub root_delay: f64,
    /// Root dispersion, seconds.
    pub root_dispersion: f64,
    /// The maximum error of the clock, seconds.
    pub max_error: f64,
}

/// A line of `measurements.log`.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasurementLine {
    /// When the packet arrived, to the second.
    pub at: Timestamp,
    /// The source's address.
    pub source: String,
    /// The leap status in the packet.
    pub leap: LeapCode,
    /// The source's stratum.
    pub stratum: u8,
    /// The ten NTP tests, test 1 in the high bit, as `ntpdata`'s flags.
    pub tests: u16,
    /// The local polling interval, log2 seconds.
    pub local_poll: i8,
    /// The remote polling interval, log2 seconds.
    pub remote_poll: i8,
    /// The poll score.
    pub score: f64,
    /// The measured offset, seconds.
    pub offset: f64,
    /// The measured delay, seconds.
    pub peer_delay: f64,
    /// The measured dispersion, seconds.
    pub peer_dispersion: f64,
    /// The packet's root delay, seconds.
    pub root_delay: f64,
    /// The packet's root dispersion, seconds.
    pub root_dispersion: f64,
    /// The packet's reference ID.
    pub reference_id: u32,
    /// The packet's NTP mode.
    pub mode: u8,
    /// Whether it was interleaved (`I`) rather than basic (`B`).
    pub interleaved: bool,
    /// Where the transmit timestamp was taken: `D`, `K` or `H`.
    pub transmit_timestamp_source: char,
    /// Where the receive timestamp was taken.
    pub receive_timestamp_source: char,
}

/// A line of `statistics.log`.
#[derive(Debug, Clone, PartialEq)]
pub struct StatisticsLine {
    /// When the regression was made, to the second.
    pub at: Timestamp,
    /// The source: its address, or a refclock's reference ID.
    pub source: String,
    /// The samples' standard deviation, seconds.
    pub standard_deviation: f64,
    /// The estimated offset, seconds.
    pub estimated_offset: f64,
    /// Its standard deviation, seconds.
    pub offset_sd: f64,
    /// The source's frequency relative to the local clock's, as a
    /// fraction (not ppm).
    pub residual_frequency: f64,
    /// Its error bound, as a fraction.
    pub skew: f64,
    /// The regression's stress.
    pub stress: f64,
    /// Samples used.
    pub samples: u32,
    /// Samples dropped as old.
    pub best_start: u32,
    /// Runs of residuals of one sign.
    pub runs: u32,
    /// The estimated asymmetry.
    pub asymmetry: f64,
}

/// A line of `refclocks.log`.
#[derive(Debug, Clone, PartialEq)]
pub struct RefclockLine {
    /// The sample's time, to the microsecond.
    pub at: Timestamp,
    /// The refclock's reference ID, as text.
    pub reference: String,
    /// How many driver polls the sample took; `None` for a sample the
    /// filter produced.
    pub driver_polls: Option<u32>,
    /// The leap status.
    pub leap: LeapCode,
    /// Whether it was a pulse; `None` for a filtered sample.
    pub pulse: Option<bool>,
    /// The offset as read, seconds; `None` for a filtered sample.
    pub raw_offset: Option<f64>,
    /// The offset after corrections and filtering, seconds.
    pub cooked_offset: f64,
    /// Its dispersion, seconds.
    pub dispersion: f64,
}

/// A line of `tempcomp.log`.
#[derive(Debug, Clone, PartialEq)]
pub struct TempcompLine {
    /// When the compensation was computed, to the second.
    pub at: Timestamp,
    /// The temperature read, in the sensor file's units.
    pub temperature: f64,
    /// The frequency compensation applied, ppm.
    pub compensation: f64,
}

/// One line of data from any log.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// From `tracking.log`.
    Tracking(TrackingLine),
    /// From `measurements.log`.
    Measurement(MeasurementLine),
    /// From `statistics.log`.
    Statistics(StatisticsLine),
    /// From `refclocks.log`.
    Refclock(RefclockLine),
    /// From `tempcomp.log`.
    Tempcomp(TempcompLine),
}

/// Why a line could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The file's header is not the one chrony 4.3 to 4.9 write: its
    /// layout has changed, so nothing in it can be trusted.
    #[error("unknown {kind} header: {header:?}")]
    UnknownHeader {
        /// The log's name.
        kind: &'static str,
        /// The header found.
        header: String,
    },
    /// A data line that does not fit the layout.
    #[error("bad {kind} line: {line:?}")]
    BadLine {
        /// The log's name.
        kind: &'static str,
        /// The line.
        line: String,
    },
}

/// Read one line of a log of `kind`: `Ok(None)` for a rule, a header
/// or a blank.
pub fn parse(kind: LogKind, line: &str) -> Result<Option<Line>, ParseError> {
    let trimmed = line.trim_end();
    if trimmed.is_empty() || trimmed.bytes().all(|byte| byte == b'=') {
        return Ok(None);
    }
    if trimmed.starts_with("   Date") {
        return if trimmed == kind.header() {
            Ok(None)
        } else {
            Err(ParseError::UnknownHeader {
                kind: kind.name(),
                header: trimmed.to_owned(),
            })
        };
    }
    let bad = || ParseError::BadLine {
        kind: kind.name(),
        line: trimmed.to_owned(),
    };
    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    let parsed = match kind {
        LogKind::Tracking => tracking(&fields).map(Line::Tracking),
        LogKind::Measurements => measurement(&fields).map(Line::Measurement),
        LogKind::Statistics => statistics(&fields).map(Line::Statistics),
        LogKind::Refclocks => refclock(&fields).map(Line::Refclock),
        LogKind::Tempcomp => tempcomp(&fields).map(Line::Tempcomp),
    };
    parsed.map(Some).ok_or_else(bad)
}

/// A date and time as chronyd logs them, UTC: `2026-10-25 05:39:33`,
/// with `.ffffff` in `refclocks.log`.
fn time(date: &str, time: &str) -> Option<Timestamp> {
    let civil: DateTime = format!("{date}T{time}").parse().ok()?;
    civil
        .to_zoned(TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp())
}

fn number<T: std::str::FromStr>(field: &str) -> Option<T> {
    field.parse().ok()
}

fn leap(field: &str) -> Option<LeapCode> {
    match field {
        "N" | "+" | "-" | "?" => field.chars().next(),
        _ => None,
    }
}

fn tracking(f: &[&str]) -> Option<TrackingLine> {
    let [
        date,
        at,
        reference,
        stratum,
        frequency,
        skew,
        offset,
        leap_code,
        combined,
        offset_sd,
        remaining,
        root_delay,
        root_dispersion,
        max_error,
    ] = f
    else {
        return None;
    };
    Some(TrackingLine {
        at: time(date, at)?,
        reference: (*reference).to_owned(),
        stratum: number(stratum)?,
        frequency: number(frequency)?,
        skew: number(skew)?,
        offset: number(offset)?,
        leap: leap(leap_code)?,
        combined_sources: number(combined)?,
        offset_sd: number(offset_sd)?,
        remaining_correction: number(remaining)?,
        root_delay: number(root_delay)?,
        root_dispersion: number(root_dispersion)?,
        max_error: number(max_error)?,
    })
}

/// The tests, written as `111 111 1111`, as bits, test 1 highest.
fn tests(groups: [&str; 3]) -> Option<u16> {
    let digits: String = groups.concat();
    if digits.len() != 10 || !digits.bytes().all(|b| b == b'0' || b == b'1') {
        return None;
    }
    u16::from_str_radix(&digits, 2).ok()
}

fn measurement(f: &[&str]) -> Option<MeasurementLine> {
    let [
        date,
        at,
        source,
        leap_code,
        stratum,
        tests1,
        tests2,
        tests3,
        local_poll,
        remote_poll,
        score,
        offset,
        peer_delay,
        peer_dispersion,
        root_delay,
        root_dispersion,
        reference_id,
        mode,
        transmit,
        receive,
    ] = f
    else {
        return None;
    };
    // `4B`: the mode, then `I`nterleaved or `B`asic.
    let (mode_digit, basic) = mode.split_at_checked(1)?;
    let interleaved = match basic {
        "I" => true,
        "B" => false,
        _ => return None,
    };
    Some(MeasurementLine {
        at: time(date, at)?,
        source: (*source).to_owned(),
        leap: leap(leap_code)?,
        stratum: number(stratum)?,
        tests: tests([tests1, tests2, tests3])?,
        local_poll: number(local_poll)?,
        remote_poll: number(remote_poll)?,
        score: number(score)?,
        offset: number(offset)?,
        peer_delay: number(peer_delay)?,
        peer_dispersion: number(peer_dispersion)?,
        root_delay: number(root_delay)?,
        root_dispersion: number(root_dispersion)?,
        reference_id: u32::from_str_radix(reference_id, 16).ok()?,
        mode: number(mode_digit)?,
        interleaved,
        transmit_timestamp_source: single(transmit)?,
        receive_timestamp_source: single(receive)?,
    })
}

fn single(field: &str) -> Option<char> {
    let mut chars = field.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

fn statistics(f: &[&str]) -> Option<StatisticsLine> {
    let [
        date,
        at,
        source,
        standard_deviation,
        estimated_offset,
        offset_sd,
        residual_frequency,
        skew,
        stress,
        samples,
        best_start,
        runs,
        asymmetry,
    ] = f
    else {
        return None;
    };
    Some(StatisticsLine {
        at: time(date, at)?,
        source: (*source).to_owned(),
        standard_deviation: number(standard_deviation)?,
        estimated_offset: number(estimated_offset)?,
        offset_sd: number(offset_sd)?,
        residual_frequency: number(residual_frequency)?,
        skew: number(skew)?,
        stress: number(stress)?,
        samples: number(samples)?,
        best_start: number(best_start)?,
        runs: number(runs)?,
        asymmetry: number(asymmetry)?,
    })
}

/// A field a filtered refclock sample leaves as `-`.
fn optional<T: std::str::FromStr>(field: &str) -> Option<Option<T>> {
    if field == "-" {
        Some(None)
    } else {
        number(field).map(Some)
    }
}

fn refclock(f: &[&str]) -> Option<RefclockLine> {
    let [
        date,
        at,
        reference,
        driver_polls,
        leap_code,
        pulse,
        raw_offset,
        cooked_offset,
        dispersion,
    ] = f
    else {
        return None;
    };
    Some(RefclockLine {
        at: time(date, at)?,
        reference: (*reference).to_owned(),
        driver_polls: optional(driver_polls)?,
        leap: leap(leap_code)?,
        pulse: optional::<u8>(pulse)?.map(|pulse| pulse != 0),
        raw_offset: optional(raw_offset)?,
        cooked_offset: number(cooked_offset)?,
        dispersion: number(dispersion)?,
    })
}

fn tempcomp(f: &[&str]) -> Option<TempcompLine> {
    let [date, at, temperature, compensation] = f else {
        return None;
    };
    Some(TempcompLine {
        at: time(date, at)?,
        temperature: number(temperature)?,
        compensation: number(compensation)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    /// Every data line of a captured log, parsed.
    fn read(kind: LogKind) -> Vec<Line> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/logs")
            .join(format!("{}.log", kind.name()));
        fs::read_to_string(&path)
            .expect("a log")
            .lines()
            .filter_map(|line| parse(kind, line).expect("a known line"))
            .collect()
    }

    fn at(text: &str) -> Timestamp {
        text.parse().expect("a timestamp")
    }

    #[test]
    fn names_files() {
        assert_eq!(LogKind::of_file("tracking.log"), Some(LogKind::Tracking));
        assert_eq!(
            LogKind::of_file("measurements.log.1"),
            Some(LogKind::Measurements)
        );
        assert_eq!(
            LogKind::of_file("statistics.log.3.gz"),
            Some(LogKind::Statistics)
        );
        assert_eq!(
            LogKind::of_file("tempcomp.log-20261024.gz"),
            Some(LogKind::Tempcomp)
        );
        assert_eq!(LogKind::of_file("tracking.logx"), None);
        assert_eq!(LogKind::of_file("rtc.log"), None);
    }

    #[test]
    fn reads_tracking() {
        let lines = read(LogKind::Tracking);
        assert_eq!(lines.len(), 18);
        let Line::Tracking(first) = &lines[1] else {
            panic!("a tracking line");
        };
        assert_eq!(
            *first,
            TrackingLine {
                at: at("2026-10-25T05:39:33Z"),
                reference: "172.18.0.2".to_owned(),
                stratum: 9,
                frequency: 0.0,
                skew: 1_000_000.0,
                offset: 8.297e-06,
                leap: 'N',
                combined_sources: 1,
                offset_sd: 4.207e-06,
                remaining_correction: 2.932e-06,
                root_delay: 1.692e-05,
                root_dispersion: 5.518e-01,
                max_error: 1.5,
            }
        );
    }

    #[test]
    fn reads_measurements() {
        let lines = read(LogKind::Measurements);
        assert_eq!(lines.len(), 18);
        let Line::Measurement(first) = &lines[0] else {
            panic!("a measurement");
        };
        assert_eq!(first.source, "172.18.0.2");
        assert_eq!(first.tests, 0x3ff);
        assert_eq!(first.offset, -9.938e-06);
        assert_eq!(first.reference_id, 0x7f7f_0101);
        assert_eq!(first.mode, 4);
        assert!(!first.interleaved);
        assert_eq!(first.receive_timestamp_source, 'K');
    }

    #[test]
    fn reads_statistics_and_tempcomp() {
        assert_eq!(read(LogKind::Statistics).len(), 18);
        let lines = read(LogKind::Tempcomp);
        assert_eq!(lines.len(), 18);
        assert!(matches!(
            &lines[0],
            Line::Tempcomp(t) if t.temperature == 41_500.0 && t.compensation == 4.1459
        ));
    }

    #[test]
    fn reads_refclocks() {
        // Written to refclock.c's formats: the containers have no
        // refclock with samples to log.
        let lines = read(LogKind::Refclocks);
        assert_eq!(
            lines[0],
            Line::Refclock(RefclockLine {
                at: at("2026-10-25T05:39:33.000123Z"),
                reference: "PPS".to_owned(),
                driver_polls: Some(0),
                leap: 'N',
                pulse: Some(true),
                raw_offset: Some(1.234_567e-6),
                cooked_offset: 1.234_567e-6,
                dispersion: 1.0e-9,
            })
        );
        assert!(matches!(
            &lines[1],
            Line::Refclock(RefclockLine {
                driver_polls: None,
                pulse: None,
                raw_offset: None,
                ..
            })
        ));
    }

    #[test]
    fn refuses_an_unknown_header() {
        assert!(matches!(
            parse(
                LogKind::Tracking,
                "   Date (UTC) Time     IP Address   St   Freq ppm   New column"
            ),
            Err(ParseError::UnknownHeader { .. })
        ));
        assert!(matches!(
            parse(LogKind::Tempcomp, "2026-10-25 05:39:33  4.1500e+04"),
            Err(ParseError::BadLine { .. })
        ));
    }
}
