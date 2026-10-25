//! The codec against chronyc itself: every reply chronyc received in
//! `tests/fixtures/<chrony version>/`, decoded here and printed as
//! chronyc's CSV mode prints it (`print_report` in `client.c`), must
//! match chronyc's own output line for line.  Our requests must match
//! chronyc's but for the sequence number and padding.

use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::path::PathBuf;

use crate::protocol::Decode;
use crate::protocol::REPLY_HEADER_LENGTH;
use crate::protocol::ReplyHeader;
use crate::protocol::Request;
use crate::protocol::SourceCount;
use crate::protocol::SourceName;
use crate::report::Activity;
use crate::report::Address;
use crate::report::Leap;
use crate::report::NtpData;
use crate::report::ReferenceId;
use crate::report::SelectData;
use crate::report::SelectOptions;
use crate::report::SourceData;
use crate::report::SourceMode;
use crate::report::SourceState;
use crate::report::SourceStats;
use crate::report::Tracking;

/// Bytes of a request's header before the sequence number, and the
/// sequence number's: the part of chronyc's request ours must repeat.
const REQUEST_COMMAND_END: usize = 6;
const REQUEST_SEQUENCE: std::ops::Range<usize> = 8..12;
const REQUEST_HEADER_LENGTH: usize = 20;

/// One captured chronyc command.
#[derive(Debug, Default)]
struct Fixture {
    /// Where it came from, for failure messages.
    path: PathBuf,
    /// Request and reply datagrams, in order.
    exchanges: Vec<(Vec<u8>, Vec<u8>)>,
    /// chronyc's CSV output.
    csv: Vec<String>,
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn parse(path: &Path) -> Fixture {
    let text = fs::read_to_string(path).expect("a fixture");
    let mut fixture = Fixture {
        path: path.to_path_buf(),
        ..Fixture::default()
    };
    let mut request = None;
    for line in text.lines() {
        if let Some(bytes) = line.strip_prefix("request ") {
            request = Some(hex(bytes.trim()));
        } else if let Some(bytes) = line.strip_prefix("reply ") {
            let sent = request.take().expect("a request before each reply");
            fixture.exchanges.push((sent, hex(bytes.trim())));
        } else if let Some(row) = line.strip_prefix("csv ") {
            fixture.csv.push(row.to_owned());
        }
    }
    fixture
}

/// Every fixture of one command, across chrony versions.
fn fixtures(command: &str) -> Vec<Fixture> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut found: Vec<Fixture> = fs::read_dir(&root)
        .expect("tests/fixtures")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(format!("{command}.txt")))
        .filter(|path| path.is_file())
        .map(|path| parse(&path))
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(!found.is_empty(), "no fixtures for {command}");
    found
}

/// The reply decoded, after checking that our request for it would
/// say what chronyc's did.
fn decode<T: Decode>(fixture: &Fixture, (sent, received): &(Vec<u8>, Vec<u8>), ours: Request) -> T {
    let encoded = ours.encode(
        u32::from_be_bytes(sent[REQUEST_SEQUENCE].try_into().expect("4 bytes")),
        0,
    );
    let data_end = REQUEST_HEADER_LENGTH
        + match ours {
            Request::SourceCount | Request::Tracking | Request::Activity => 0,
            Request::SourceData(_) | Request::SourceStats(_) | Request::SelectData(_) => 4,
            Request::NtpData(_) | Request::SourceName(_) => 20,
        };
    assert_eq!(
        &encoded[..REQUEST_COMMAND_END],
        &sent[..REQUEST_COMMAND_END],
        "{}: request header",
        fixture.path.display()
    );
    assert_eq!(
        &encoded[REQUEST_SEQUENCE.end..data_end],
        &sent[REQUEST_SEQUENCE.end..data_end],
        "{}: request data",
        fixture.path.display()
    );
    let header = ReplyHeader::parse(received).expect("a reply");
    header.status().expect("success");
    T::decode(
        header.command,
        header.reply,
        &received[REPLY_HEADER_LENGTH..],
    )
    .unwrap_or_else(|e| panic!("{}: {e}", fixture.path.display()))
}

fn leap(leap: Leap) -> &'static str {
    match leap {
        Leap::Normal => "Normal",
        Leap::Insert => "Insert second",
        Leap::Delete => "Delete second",
        Leap::Unsynchronized => "Not synchronised",
    }
}

/// chronyc's `UTI_RefidToString`: the ID's printable characters.
fn reference_text(id: ReferenceId) -> String {
    id.0.to_be_bytes()
        .into_iter()
        .take_while(|byte| *byte != 0)
        .filter(u8::is_ascii_graphic)
        .map(char::from)
        .collect()
}

/// A source's name in numeric CSV output: its address, or a refclock's
/// reference ID as text.
fn name(address: Address) -> String {
    match address {
        Address::Ip(address) => address.to_string(),
        Address::ReferenceId(id) => reference_text(ReferenceId(id)),
        Address::Unresolved(_) => address.to_string(),
        Address::Unspecified => "[UNSPEC]".to_owned(),
    }
}

fn timestamp(t: jiff::Timestamp) -> String {
    format!("{}.{:09}", t.as_second(), t.subsec_nanosecond())
}

#[test]
fn tracking() {
    for fixture in fixtures("tracking") {
        let t: Tracking = decode(&fixture, &fixture.exchanges[0], Request::Tracking);
        let reference = match t.address {
            Address::Ip(address) => address.to_string(),
            _ => reference_text(t.reference_id),
        };
        let row = format!(
            "{},{},{},{},{:.9},{:.9},{:.9},{:.3},{:.3},{:.3},{:.9},{:.9},{:.1},{}",
            t.reference_id,
            reference,
            t.stratum,
            timestamp(t.reference_time),
            t.current_correction.get(),
            t.last_offset.get(),
            t.rms_offset.get(),
            t.frequency.get(),
            t.residual_frequency.get(),
            t.skew.get(),
            t.root_delay.get(),
            t.root_dispersion.get(),
            t.last_update_interval.get(),
            leap(t.leap),
        );
        assert_eq!(row, fixture.csv[0], "{}", fixture.path.display());
    }
}

/// The reports asked for by index, one per source: the source count
/// first, then each source in turn.
fn by_index<T: Decode>(fixture: &Fixture, request: fn(u32) -> Request) -> Vec<T> {
    let count: SourceCount = decode(fixture, &fixture.exchanges[0], Request::SourceCount);
    (0..count.0)
        .map(|index| {
            let exchange = &fixture.exchanges[1 + usize::try_from(index).expect("small")];
            decode(fixture, exchange, request(index))
        })
        .collect()
}

#[test]
fn sources() {
    for fixture in fixtures("sources-n") {
        let rows: Vec<String> = by_index::<SourceData>(&fixture, Request::SourceData)
            .into_iter()
            .map(|s| {
                let mode = match s.mode {
                    SourceMode::Client => '^',
                    SourceMode::Peer => '=',
                    SourceMode::Refclock => '#',
                };
                let state = match s.state {
                    SourceState::Selected => '*',
                    SourceState::Nonselectable => '?',
                    SourceState::Falseticker => 'x',
                    SourceState::Jittery => '~',
                    SourceState::Combined => '+',
                    SourceState::NotCombined => '-',
                };
                format!(
                    "{mode},{state},{},{},{},{:o},{},{:.9},{:.9},{:.9}",
                    name(s.address),
                    s.stratum,
                    s.poll,
                    s.reachability,
                    s.since_sample
                        .map_or(u64::from(u32::MAX), |age| age.as_secs()),
                    s.latest_measurement.get(),
                    s.original_latest_measurement.get(),
                    s.latest_measurement_error.get(),
                )
            })
            .collect();
        assert_eq!(rows, fixture.csv, "{}", fixture.path.display());
    }
}

#[test]
fn sourcestats() {
    for fixture in fixtures("sourcestats") {
        let rows: Vec<String> = by_index::<SourceStats>(&fixture, Request::SourceStats)
            .into_iter()
            .map(|s| {
                format!(
                    "{},{},{},{},{:.3},{:.3},{:.9},{:.9}",
                    name(s.address),
                    s.samples,
                    s.runs,
                    s.span.as_secs(),
                    s.residual_frequency.get(),
                    s.skew.get(),
                    s.estimated_offset.get(),
                    s.standard_deviation.get(),
                )
            })
            .collect();
        assert_eq!(rows, fixture.csv, "{}", fixture.path.display());
    }
}

fn options(o: SelectOptions) -> String {
    [
        if o.noselect { 'N' } else { '-' },
        if o.prefer { 'P' } else { '-' },
        if o.trust { 'T' } else { '-' },
        if o.require { 'R' } else { '-' },
        '-',
    ]
    .iter()
    .map(char::to_string)
    .collect::<Vec<_>>()
    .join(",")
}

#[test]
fn selectdata() {
    for fixture in fixtures("selectdata") {
        let rows: Vec<String> = by_index::<SelectData>(&fixture, Request::SelectData)
            .into_iter()
            .map(|s| {
                format!(
                    "{},{},{},{},{},{},{:.1},{:.9},{:.9},{}",
                    s.state,
                    name(s.address),
                    if s.authenticated { 'Y' } else { 'N' },
                    options(s.configured_options),
                    options(s.effective_options),
                    s.last_sample_ago.as_secs(),
                    s.score,
                    s.low_limit.get(),
                    s.high_limit.get(),
                    leap(s.leap),
                )
            })
            .collect();
        assert_eq!(rows, fixture.csv, "{}", fixture.path.display());
    }
}

#[test]
fn activity() {
    for fixture in fixtures("activity") {
        let a: Activity = decode(&fixture, &fixture.exchanges[0], Request::Activity);
        let row = format!(
            "{},{},{},{},{}",
            a.online, a.offline, a.burst_online, a.burst_offline, a.unresolved
        );
        assert_eq!(row, fixture.csv[0], "{}", fixture.path.display());
    }
}

fn bits(value: u16, width: usize) -> String {
    (0..width)
        .rev()
        .map(|i| if value >> i & 1 == 1 { '1' } else { '0' })
        .collect()
}

/// chronyc's `%M`, an NTP mode.
fn mode(mode: u8) -> &'static str {
    const ACTIVE: u8 = 1;
    const PASSIVE: u8 = 2;
    const SERVER: u8 = 4;
    match mode {
        ACTIVE => "Symmetric active",
        PASSIVE => "Symmetric passive",
        SERVER => "Server",
        _ => "Invalid",
    }
}

fn timestamp_source(source: char) -> &'static str {
    match source {
        'D' => "Daemon",
        'K' => "Kernel",
        'H' => "Hardware",
        _ => "Invalid",
    }
}

fn ntp_leap(value: u8) -> &'static str {
    match value {
        0 => "Normal",
        1 => "Insert second",
        2 => "Delete second",
        3 => "Not synchronised",
        _ => "Invalid",
    }
}

/// chronyc's `UTI_IPToRefid` for IPv4, the address itself, or zero
/// for none.
fn ipv4_refid(address: Address) -> String {
    match address {
        Address::Ip(IpAddr::V4(v4)) => format!("{:08X}", u32::from(v4)),
        Address::Unspecified => format!("{:08X}", 0),
        other => panic!("IPv4 only in fixtures: {other:?}"),
    }
}

#[test]
fn ntpdata() {
    for fixture in fixtures("ntpdata") {
        // chronyc asks for the source count, then each source's data
        // and, for an NTP source, its NTP data straight after.
        let mut exchanges = fixture.exchanges.iter();
        let count: SourceCount = decode(
            &fixture,
            exchanges.next().expect("a count"),
            Request::SourceCount,
        );
        let mut rows = Vec::new();
        for index in 0..count.0 {
            let source: SourceData = decode(
                &fixture,
                exchanges.next().expect("a source"),
                Request::SourceData(index),
            );
            let Address::Ip(address) = source.address else {
                continue;
            };
            let n: NtpData = decode(
                &fixture,
                exchanges.next().expect("NTP data"),
                Request::NtpData(address),
            );
            rows.push(ntp_row(&n));
        }
        assert_eq!(rows, fixture.csv, "{}", fixture.path.display());
    }
}

fn ntp_row(n: &NtpData) -> String {
    let reference = if n.stratum <= 1 {
        reference_text(n.reference_id)
    } else {
        String::new()
    };
    let row = [
        name(n.remote_address),
        ipv4_refid(n.remote_address),
        n.remote_port.to_string(),
        name(n.local_address),
        ipv4_refid(n.local_address),
        ntp_leap(n.leap).to_owned(),
        n.version.to_string(),
        mode(n.mode).to_owned(),
        n.stratum.to_string(),
        n.poll.to_string(),
        format!("{:.0}", 2f64.powi(n.poll.into())),
        n.precision.to_string(),
        format!("{:.9}", 2f64.powi(n.precision.into())),
        format!("{:.6}", n.root_delay.get()),
        format!("{:.6}", n.root_dispersion.get()),
        n.reference_id.to_string(),
        reference,
        timestamp(n.reference_time),
        format!("{:.9}", n.offset.get()),
        format!("{:.9}", n.peer_delay.get()),
        format!("{:.9}", n.peer_dispersion.get()),
        format!("{:.9}", n.response_time.get()),
        format!("{:.2}", n.jitter_asymmetry),
        bits(n.tests >> 7, 3),
        bits(n.tests >> 4 & 0x7, 3),
        bits(n.tests & 0xf, 4),
        yes(n.interleaved).to_owned(),
        yes(n.authenticated).to_owned(),
        timestamp_source(n.transmit_timestamp_source).to_owned(),
        timestamp_source(n.receive_timestamp_source).to_owned(),
        n.total_transmitted.to_string(),
        n.total_received.to_string(),
        n.total_valid.to_string(),
        n.total_good.to_string(),
    ];
    let counts = n.timestamps.map(|t| {
        [
            t.kernel_transmit,
            t.kernel_receive,
            t.hardware_transmit,
            t.hardware_receive,
        ]
        .map(|count| count.to_string())
    });
    row.into_iter()
        .chain(counts.into_iter().flatten())
        .collect::<Vec<_>>()
        .join(",")
}

fn yes(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

#[test]
fn source_names() {
    for fixture in fixtures("sources") {
        // As in `ntpdata`: each NTP source's data, then its name.
        let mut exchanges = fixture.exchanges.iter();
        let count: SourceCount = decode(
            &fixture,
            exchanges.next().expect("a count"),
            Request::SourceCount,
        );
        let mut names = Vec::new();
        for index in 0..count.0 {
            let source: SourceData = decode(
                &fixture,
                exchanges.next().expect("a source"),
                Request::SourceData(index),
            );
            names.push(match source.address {
                Address::Ip(address) => {
                    let named: SourceName = decode(
                        &fixture,
                        exchanges.next().expect("a name"),
                        Request::SourceName(address),
                    );
                    named.0
                }
                other => name(other),
            });
        }
        let expected: Vec<&str> = fixture
            .csv
            .iter()
            .map(|row| row.split(',').nth(2).expect("a name column"))
            .collect();
        assert_eq!(names, expected, "{}", fixture.path.display());
    }
}
