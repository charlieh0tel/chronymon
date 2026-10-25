//! The reports chronyd answers, decoded.  Field meanings follow
//! chronyc's documentation (`chronyc.adoc`) and `candm.h`.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use chronymon::units::Ppm;
use chronymon::units::Seconds;
use jiff::Timestamp;

/// A source's or reference's address as chronyd reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Address {
    /// No address: an unsynchronized reference, say.
    Unspecified,
    /// An NTP server or peer.
    Ip(IpAddr),
    /// A source with no address, a refclock, known by its reference ID.
    ReferenceId(u32),
    /// An NTP source whose name is not resolved yet, by the placeholder
    /// ID chronyd numbers it with (`IPADDR_ID`, `NSR_AddSourceByName`
    /// in `ntp_sources.c`, chrony 4.3 to 4.9).  chronyc's `sources`
    /// leaves these out unless asked for all (`-a`).
    Unresolved(u32),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unspecified => write!(f, "-"),
            Self::Ip(address) => write!(f, "{address}"),
            Self::ReferenceId(id) => write!(f, "{}", ReferenceId(*id)),
            // As chronyd prints it (`UTI_IPToString`).
            Self::Unresolved(id) => write!(f, "ID#{id:010}"),
        }
    }
}

/// An NTP reference ID: an IPv4 address, a hash of an IPv6 one, or up
/// to four ASCII characters naming a refclock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReferenceId(pub u32);

impl ReferenceId {
    /// The ID as text when it is printable ASCII (`PPS`, `GPS`), as
    /// chronyc shows a refclock's.
    pub fn text(self) -> Option<String> {
        let text: String = self
            .0
            .to_be_bytes()
            .into_iter()
            .take_while(|byte| *byte != 0)
            .map(char::from)
            .collect();
        (!text.is_empty() && text.chars().all(|c| c.is_ascii_graphic())).then_some(text)
    }
}

impl fmt::Display for ReferenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08X}", self.0)
    }
}

/// Leap second status, of the local clock or a source (`NTP_Leap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Leap {
    /// No leap second pending.
    Normal,
    /// A second will be inserted at the end of the day.
    Insert,
    /// A second will be deleted at the end of the day.
    Delete,
    /// Not synchronized.
    Unsynchronized,
}

/// What `tracking` reports: the local clock's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracking {
    /// The reference ID of the selected source.
    pub reference_id: ReferenceId,
    /// The selected source's address.
    pub address: Address,
    /// The local clock's stratum.
    pub stratum: u16,
    /// The local clock's leap status.
    pub leap: Leap,
    /// When the last measurement from the reference was processed.
    pub reference_time: Timestamp,
    /// How far the system clock is from true time now, being slewed.
    pub current_correction: Seconds,
    /// The offset measured on the last clock update.
    pub last_offset: Seconds,
    /// A long-term average of the offset.
    pub rms_offset: Seconds,
    /// How fast the system clock would run uncorrected; positive is
    /// fast.
    pub frequency: Ppm,
    /// The selected source's frequency less the current frequency.
    pub residual_frequency: Ppm,
    /// The error bound on the frequency.
    pub skew: Ppm,
    /// Total network path delay to the stratum-1 at the root.
    pub root_delay: Seconds,
    /// Total dispersion accumulated to the stratum-1 at the root.
    pub root_dispersion: Seconds,
    /// The interval between the last two clock updates.
    pub last_update_interval: Seconds,
}

/// How a source is configured (`RPY_SD_MD_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceMode {
    /// An NTP server, `server` or `pool`.
    Client,
    /// An NTP peer, `peer`.
    Peer,
    /// A reference clock, `refclock`.
    Refclock,
}

/// Where selection left a source (`RPY_SD_ST_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceState {
    /// The source synchronizing the clock (`*`).
    Selected,
    /// Not selectable: unreachable, `noselect`, or failing tests (`?`).
    Nonselectable,
    /// Inconsistent with the majority (`x`).
    Falseticker,
    /// Too variable to be trusted (`~`).
    Jittery,
    /// Acceptable and combined with the selected source (`+`;
    /// `RPY_SD_ST_UNSELECTED`, whose name says less than it means).
    Combined,
    /// Acceptable but not combined (`-`; `RPY_SD_ST_SELECTABLE`).
    NotCombined,
}

/// What `sources` reports for one source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceData {
    /// The source's address, or a refclock's reference ID.
    pub address: Address,
    /// The polling interval, as a power of two seconds.
    pub poll: i16,
    /// The source's stratum.
    pub stratum: u16,
    /// Where selection left it.
    pub state: SourceState,
    /// How it is configured.
    pub mode: SourceMode,
    /// The reachability register: one bit per poll, newest lowest.
    pub reachability: u16,
    /// How long ago the last sample arrived; `None` before the first.
    pub since_sample: Option<Duration>,
    /// The last sample's offset, as measured.
    pub original_latest_measurement: Seconds,
    /// The last sample's offset, adjusted for slewing since.
    pub latest_measurement: Seconds,
    /// The last sample's error bound.
    pub latest_measurement_error: Seconds,
}

/// What `sourcestats` reports for one source: chronyd's regression
/// over its recent samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceStats {
    /// The source's reference ID.
    pub reference_id: ReferenceId,
    /// The source's address, or a refclock's reference ID.
    pub address: Address,
    /// Samples retained.
    pub samples: u32,
    /// Runs of residuals of one sign; few runs suggests a poor fit.
    pub runs: u32,
    /// The time the samples span.
    pub span: Duration,
    /// The samples' estimated standard deviation.
    pub standard_deviation: Seconds,
    /// The source's frequency relative to the local clock's.
    pub residual_frequency: Ppm,
    /// The error bound on that frequency.
    pub skew: Ppm,
    /// The source's estimated offset.
    pub estimated_offset: Seconds,
    /// The error bound on that offset.
    pub estimated_offset_error: Seconds,
}

/// Counts of timestamps by where they were taken, from chrony 4.6
/// (`RPY_NTP_DATA2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampCounts {
    /// Transmit timestamps taken by the kernel.
    pub kernel_transmit: u32,
    /// Receive timestamps taken by the kernel.
    pub kernel_receive: u32,
    /// Transmit timestamps taken by the network hardware.
    pub hardware_transmit: u32,
    /// Receive timestamps taken by the network hardware.
    pub hardware_receive: u32,
}

/// What `ntpdata` reports for one NTP source: its last valid
/// measurement and counters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NtpData {
    /// The source's address.
    pub remote_address: Address,
    /// The local address used to reach it.
    pub local_address: Address,
    /// The source's port.
    pub remote_port: u16,
    /// The leap status in the source's last packet.
    pub leap: u8,
    /// The NTP version of its last packet.
    pub version: u8,
    /// The NTP mode of its last packet.
    pub mode: u8,
    /// Its stratum.
    pub stratum: u8,
    /// Its polling interval, as a power of two seconds.
    pub poll: i8,
    /// Its clock's precision, as a power of two seconds.
    pub precision: i8,
    /// Its root delay.
    pub root_delay: Seconds,
    /// Its root dispersion.
    pub root_dispersion: Seconds,
    /// Its reference ID.
    pub reference_id: ReferenceId,
    /// Its reference time.
    pub reference_time: Timestamp,
    /// The last measured offset.
    pub offset: Seconds,
    /// The last measured round-trip delay.
    pub peer_delay: Seconds,
    /// The last measured dispersion.
    pub peer_dispersion: Seconds,
    /// How long the server took to respond.
    pub response_time: Seconds,
    /// The assumed asymmetry of network jitter, from -0.5 to 0.5.
    pub jitter_asymmetry: f64,
    /// Which of the NTP tests the last packet passed, one bit each.
    pub tests: u16,
    /// Whether interleaved mode is in use.
    pub interleaved: bool,
    /// Whether the packets are authenticated.
    pub authenticated: bool,
    /// Where the transmit timestamp was taken: `D`aemon, `K`ernel or
    /// `H`ardware.
    pub transmit_timestamp_source: char,
    /// Where the receive timestamp was taken, likewise.
    pub receive_timestamp_source: char,
    /// Packets sent to the source.
    pub total_transmitted: u32,
    /// Packets received from it.
    pub total_received: u32,
    /// Received packets that were valid.
    pub total_valid: u32,
    /// Valid packets that were also good enough to use.
    pub total_good: u32,
    /// Timestamps by where they were taken; chrony before 4.6 does not
    /// count them.
    pub timestamps: Option<TimestampCounts>,
}

/// Selection options of a source (`RPY_SD_OPTION_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SelectOptions {
    /// `noselect`: never selected.
    pub noselect: bool,
    /// `prefer`: preferred over others.
    pub prefer: bool,
    /// `trust`: assumed correct.
    pub trust: bool,
    /// `require`: required for synchronization.
    pub require: bool,
}

/// What `selectdata` reports for one source: why selection treated it
/// as it did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectData {
    /// The source's reference ID.
    pub reference_id: ReferenceId,
    /// The source's address, or a refclock's reference ID.
    pub address: Address,
    /// chronyd's one-character selection state (`*`, `+`, `x`, `N`,
    /// ...; chronyc's legend).
    pub state: char,
    /// Whether authentication is enabled for it.
    pub authenticated: bool,
    /// The leap status it reports.
    pub leap: Leap,
    /// Options as configured.
    pub configured_options: SelectOptions,
    /// Options in effect.
    pub effective_options: SelectOptions,
    /// How long ago its last sample arrived.
    pub last_sample_ago: Duration,
    /// Its score in reselection; the selected source's is 1.
    pub score: f64,
    /// The low end of its offset interval.
    pub low_limit: Seconds,
    /// The high end of its offset interval.
    pub high_limit: Seconds,
}

/// What `activity` reports: how many sources are in each state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    /// Sources online.
    pub online: i32,
    /// Sources offline.
    pub offline: i32,
    /// Sources doing a burst, to return online.
    pub burst_online: i32,
    /// Sources doing a burst, to return offline.
    pub burst_offline: i32,
    /// Sources whose names are not yet resolved.
    pub unresolved: i32,
}
