//! Requests and replies of chronyd's control protocol, version 6, as
//! chrony 4.3 to 4.9 define them (`candm.h`, `pktlength.c`,
//! `cmdmon.c`).  Only read-only reports.

use std::net::IpAddr;
use std::time::Duration;

use chronymon::units::Ppm;
use chronymon::units::Seconds;

use crate::error::DecodeError;
use crate::error::Status;
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
use crate::report::TimestampCounts;
use crate::report::Tracking;
use crate::wire::Reader;
use crate::wire::address_bytes;

/// `PROTO_VERSION_NUMBER`, unchanged since chrony 2.2.
const PROTOCOL_VERSION: u8 = 6;
const PACKET_TYPE_REQUEST: u8 = 1;
const PACKET_TYPE_REPLY: u8 = 2;

/// Bytes before a request's data: version, type, two reserved, command,
/// attempt, sequence, two pads.
const REQUEST_HEADER_LENGTH: usize = 20;
/// Bytes before a reply's data: version, type, two reserved, command,
/// reply type, status, three pads, sequence, two pads.
pub(crate) const REPLY_HEADER_LENGTH: usize = 28;

/// Command codes (`REQ_*`).
mod command {
    pub(super) const N_SOURCES: u16 = 14;
    pub(super) const SOURCE_DATA: u16 = 15;
    pub(super) const TRACKING: u16 = 33;
    pub(super) const SOURCESTATS: u16 = 34;
    pub(super) const ACTIVITY: u16 = 44;
    pub(super) const NTP_DATA: u16 = 57;
    pub(super) const NTP_SOURCE_NAME: u16 = 65;
    pub(super) const SELECT_DATA: u16 = 69;
}

/// Reply type codes (`RPY_*`) and the length of each one's data, up to
/// but not including its `EOR`.
mod reply {
    pub(super) const N_SOURCES: (u16, usize) = (2, 4);
    pub(super) const SOURCE_DATA: (u16, usize) = (3, 48);
    pub(super) const TRACKING: (u16, usize) = (5, 76);
    pub(super) const SOURCESTATS: (u16, usize) = (6, 56);
    pub(super) const ACTIVITY: (u16, usize) = (12, 20);
    /// chrony 4.3 to 4.5.
    pub(super) const NTP_DATA: (u16, usize) = (16, 124);
    pub(super) const NTP_SOURCE_NAME: (u16, usize) = (19, 256);
    pub(super) const SELECT_DATA: (u16, usize) = (23, 48);
    /// chrony 4.6 on: four timestamp counters out of the reserved words.
    pub(super) const NTP_DATA2: (u16, usize) = (26, 144);
}

/// `STT_SUCCESS` and the failures worth naming.
const STATUS_SUCCESS: u16 = 0;
const STATUS_FAILED: u16 = 1;
const STATUS_UNAUTHORIZED: u16 = 2;
const STATUS_INVALID: u16 = 3;
const STATUS_NO_SUCH_SOURCE: u16 = 4;
const STATUS_BAD_PACKET_VERSION: u16 = 18;
const STATUS_BAD_PACKET_LENGTH: u16 = 19;

/// `RPY_NTP_FLAG*` in `ntpdata`'s flags.
const NTP_FLAGS_TESTS: u16 = 0x3ff;
const NTP_FLAG_INTERLEAVED: u16 = 0x4000;
const NTP_FLAG_AUTHENTICATED: u16 = 0x8000;

/// `RPY_SD_OPTION_*` in `selectdata`'s options.
const OPTION_NOSELECT: u16 = 0x1;
const OPTION_PREFER: u16 = 0x2;
const OPTION_TRUST: u16 = 0x4;
const OPTION_REQUIRE: u16 = 0x8;

/// `since_sample` of a source with no sample yet.
const NO_SAMPLE: u32 = u32::MAX;

/// Bytes of padding words the NTP data replies end with.
const NTP_DATA_RESERVED: usize = 3 * 4;
const NTP_DATA2_RESERVED: usize = 4 * 4;

/// A report to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Request {
    /// How many sources there are.
    SourceCount,
    /// `sources`, for the source at an index.
    SourceData(u32),
    /// `tracking`.
    Tracking,
    /// `sourcestats`, for the source at an index.
    SourceStats(u32),
    /// `activity`.
    Activity,
    /// `ntpdata`, for the NTP source at an address.
    NtpData(IpAddr),
    /// `sourcename`, the name a source was configured with.
    SourceName(IpAddr),
    /// `selectdata`, for the source at an index.
    SelectData(u32),
}

impl Request {
    pub(crate) fn command(self) -> u16 {
        match self {
            Self::SourceCount => command::N_SOURCES,
            Self::SourceData(_) => command::SOURCE_DATA,
            Self::Tracking => command::TRACKING,
            Self::SourceStats(_) => command::SOURCESTATS,
            Self::Activity => command::ACTIVITY,
            Self::NtpData(_) => command::NTP_DATA,
            Self::SourceName(_) => command::NTP_SOURCE_NAME,
            Self::SelectData(_) => command::SELECT_DATA,
        }
    }

    fn data(self) -> Vec<u8> {
        match self {
            Self::SourceCount | Self::Tracking | Self::Activity => Vec::new(),
            Self::SourceData(index) | Self::SourceStats(index) | Self::SelectData(index) => {
                index.to_be_bytes().to_vec()
            }
            Self::NtpData(address) | Self::SourceName(address) => address_bytes(address).to_vec(),
        }
    }

    /// The data length of the longest reply this client accepts.
    /// chronyd sends no reply longer than its request, against
    /// amplification, so the request is padded out to it.
    fn longest_reply(self) -> usize {
        match self {
            Self::SourceCount => reply::N_SOURCES.1,
            Self::SourceData(_) => reply::SOURCE_DATA.1,
            Self::Tracking => reply::TRACKING.1,
            Self::SourceStats(_) => reply::SOURCESTATS.1,
            Self::Activity => reply::ACTIVITY.1,
            Self::NtpData(_) => reply::NTP_DATA.1.max(reply::NTP_DATA2.1),
            Self::SourceName(_) => reply::NTP_SOURCE_NAME.1,
            Self::SelectData(_) => reply::SELECT_DATA.1,
        }
    }

    /// The request as sent: header, data, zero padding.
    pub(crate) fn encode(self, sequence: u32, attempt: u16) -> Vec<u8> {
        let data = self.data();
        let length =
            (REQUEST_HEADER_LENGTH + data.len()).max(REPLY_HEADER_LENGTH + self.longest_reply());
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(&[PROTOCOL_VERSION, PACKET_TYPE_REQUEST, 0, 0]);
        bytes.extend_from_slice(&self.command().to_be_bytes());
        bytes.extend_from_slice(&attempt.to_be_bytes());
        bytes.extend_from_slice(&sequence.to_be_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&data);
        bytes.resize(length, 0);
        bytes
    }
}

/// A reply's header, as far as a client needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplyHeader {
    pub(crate) command: u16,
    pub(crate) reply: u16,
    pub(crate) status: u16,
    pub(crate) sequence: u32,
}

impl ReplyHeader {
    /// The header of a datagram that is a reply, or `None` for anything
    /// else, which a client ignores as chronyc does: one shorter than
    /// the whole header included (`client.c`, chrony 4.3 to 4.9).
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < REPLY_HEADER_LENGTH {
            return None;
        }
        let mut reader = Reader::new(bytes);
        let [version, packet_type, reserved1, reserved2] = reader.take().ok()?;
        if version != PROTOCOL_VERSION
            || packet_type != PACKET_TYPE_REPLY
            || reserved1 != 0
            || reserved2 != 0
        {
            return None;
        }
        let command = reader.u16().ok()?;
        let reply = reader.u16().ok()?;
        let status = reader.u16().ok()?;
        reader.skip(6).ok()?;
        let sequence = reader.u32().ok()?;
        Some(Self {
            command,
            reply,
            status,
            sequence,
        })
    }

    pub(crate) fn status(self) -> Result<(), Status> {
        match self.status {
            STATUS_SUCCESS => Ok(()),
            STATUS_FAILED => Err(Status::Failed),
            STATUS_UNAUTHORIZED => Err(Status::Unauthorized),
            STATUS_INVALID => Err(Status::Invalid),
            STATUS_NO_SUCH_SOURCE => Err(Status::NoSuchSource),
            STATUS_BAD_PACKET_VERSION => Err(Status::BadPacketVersion),
            STATUS_BAD_PACKET_LENGTH => Err(Status::BadPacketLength),
            other => Err(Status::Other(other)),
        }
    }
}

/// A report decoded from a successful reply's data.
pub(crate) trait Decode: Sized {
    /// Decode `data`, the bytes after the header, of reply type `reply`
    /// to `command`.
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError>;
}

/// The data of a reply that must be of type `expected`, checked for
/// length.
fn expect(
    command: u16,
    reply: u16,
    data: &[u8],
    expected: (u16, usize),
) -> Result<Reader<'_>, DecodeError> {
    if reply != expected.0 {
        return Err(DecodeError::UnsupportedReply { command, reply });
    }
    if data.len() < expected.1 {
        return Err(DecodeError::Truncated {
            length: REPLY_HEADER_LENGTH + data.len(),
            needed: REPLY_HEADER_LENGTH + expected.1,
        });
    }
    Ok(Reader::new(data))
}

fn leap(value: u16) -> Result<Leap, DecodeError> {
    Ok(match value {
        0 => Leap::Normal,
        1 => Leap::Insert,
        2 => Leap::Delete,
        3 => Leap::Unsynchronized,
        other => {
            return Err(DecodeError::UnknownValue {
                field: "leap",
                value: other.into(),
            });
        }
    })
}

fn seconds(reader: &mut Reader<'_>) -> Result<Seconds, DecodeError> {
    Ok(Seconds::new(reader.float()?))
}

fn ppm(reader: &mut Reader<'_>) -> Result<Ppm, DecodeError> {
    Ok(Ppm::new(reader.float()?))
}

fn whole_seconds(reader: &mut Reader<'_>) -> Result<Duration, DecodeError> {
    Ok(Duration::from_secs(reader.u32()?.into()))
}

/// A source's address, or for a refclock, whose address chronyd
/// leaves unspecified, its reference ID: so a source has one address
/// in every report.
fn or_reference_id(address: Address, reference_id: ReferenceId) -> Address {
    match address {
        Address::Unspecified => Address::ReferenceId(reference_id.0),
        other => other,
    }
}

fn select_options(bits: u16) -> SelectOptions {
    SelectOptions {
        noselect: bits & OPTION_NOSELECT != 0,
        prefer: bits & OPTION_PREFER != 0,
        trust: bits & OPTION_TRUST != 0,
        require: bits & OPTION_REQUIRE != 0,
    }
}

/// The number of sources, `RPY_N_SOURCES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceCount(pub(crate) u32);

impl Decode for SourceCount {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self(expect(command, reply, data, reply::N_SOURCES)?.u32()?))
    }
}

/// A source's configured name, `RPY_NTP_SOURCE_NAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceName(pub(crate) String);

impl Decode for SourceName {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let name: [u8; reply::NTP_SOURCE_NAME.1] =
            expect(command, reply, data, reply::NTP_SOURCE_NAME)?.take()?;
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        Ok(Self(String::from_utf8_lossy(&name[..end]).into_owned()))
    }
}

impl Decode for Tracking {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = expect(command, reply, data, reply::TRACKING)?;
        Ok(Self {
            reference_id: ReferenceId(r.u32()?),
            address: r.address()?,
            stratum: r.u16()?,
            leap: leap(r.u16()?)?,
            reference_time: r.timestamp()?,
            current_correction: seconds(&mut r)?,
            last_offset: seconds(&mut r)?,
            rms_offset: seconds(&mut r)?,
            frequency: ppm(&mut r)?,
            residual_frequency: ppm(&mut r)?,
            skew: ppm(&mut r)?,
            root_delay: seconds(&mut r)?,
            root_dispersion: seconds(&mut r)?,
            last_update_interval: seconds(&mut r)?,
        })
    }
}

impl Decode for SourceData {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = expect(command, reply, data, reply::SOURCE_DATA)?;
        let address = r.address()?;
        let poll = r.i16()?;
        let stratum = r.u16()?;
        let state = match r.u16()? {
            0 => SourceState::Selected,
            1 => SourceState::Nonselectable,
            2 => SourceState::Falseticker,
            3 => SourceState::Jittery,
            4 => SourceState::Combined,
            5 => SourceState::NotCombined,
            other => {
                return Err(DecodeError::UnknownValue {
                    field: "source state",
                    value: other.into(),
                });
            }
        };
        let mode = match r.u16()? {
            0 => SourceMode::Client,
            1 => SourceMode::Peer,
            2 => SourceMode::Refclock,
            other => {
                return Err(DecodeError::UnknownValue {
                    field: "source mode",
                    value: other.into(),
                });
            }
        };
        // Flags: always zero since chrony 3.
        r.skip(2)?;
        // chronyd sends a refclock's reference ID as an IPv4 address
        // here, where the other reports leave its address unspecified.
        let address = match (mode, address) {
            (SourceMode::Refclock, Address::Ip(IpAddr::V4(id))) => {
                Address::ReferenceId(u32::from(id))
            }
            _ => address,
        };
        Ok(Self {
            address,
            poll,
            stratum,
            state,
            mode,
            reachability: r.u16()?,
            since_sample: match r.u32()? {
                NO_SAMPLE => None,
                seconds => Some(Duration::from_secs(seconds.into())),
            },
            original_latest_measurement: seconds(&mut r)?,
            latest_measurement: seconds(&mut r)?,
            latest_measurement_error: seconds(&mut r)?,
        })
    }
}

impl Decode for SourceStats {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = expect(command, reply, data, reply::SOURCESTATS)?;
        let reference_id = ReferenceId(r.u32()?);
        Ok(Self {
            reference_id,
            address: or_reference_id(r.address()?, reference_id),
            samples: r.u32()?,
            runs: r.u32()?,
            span: whole_seconds(&mut r)?,
            standard_deviation: seconds(&mut r)?,
            residual_frequency: ppm(&mut r)?,
            skew: ppm(&mut r)?,
            estimated_offset: seconds(&mut r)?,
            estimated_offset_error: seconds(&mut r)?,
        })
    }
}

impl Decode for NtpData {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let layout = match reply {
            r if r == reply::NTP_DATA.0 => reply::NTP_DATA,
            _ => reply::NTP_DATA2,
        };
        let mut r = expect(command, reply, data, layout)?;
        let remote_address = r.address()?;
        let local_address = r.address()?;
        let remote_port = r.u16()?;
        let leap = r.u8()?;
        let version = r.u8()?;
        let mode = r.u8()?;
        let stratum = r.u8()?;
        let poll = r.i8()?;
        let precision = r.i8()?;
        let root_delay = seconds(&mut r)?;
        let root_dispersion = seconds(&mut r)?;
        let reference_id = ReferenceId(r.u32()?);
        let reference_time = r.timestamp()?;
        let offset = seconds(&mut r)?;
        let peer_delay = seconds(&mut r)?;
        let peer_dispersion = seconds(&mut r)?;
        let response_time = seconds(&mut r)?;
        let jitter_asymmetry = r.float()?;
        let flags = r.u16()?;
        let transmit_timestamp_source = char::from(r.u8()?);
        let receive_timestamp_source = char::from(r.u8()?);
        let total_transmitted = r.u32()?;
        let total_received = r.u32()?;
        let total_valid = r.u32()?;
        let total_good = r.u32()?;
        let timestamps = if layout == reply::NTP_DATA2 {
            let counts = TimestampCounts {
                kernel_transmit: r.u32()?,
                kernel_receive: r.u32()?,
                hardware_transmit: r.u32()?,
                hardware_receive: r.u32()?,
            };
            r.skip(NTP_DATA2_RESERVED)?;
            Some(counts)
        } else {
            r.skip(NTP_DATA_RESERVED)?;
            None
        };
        Ok(Self {
            remote_address,
            local_address,
            remote_port,
            leap,
            version,
            mode,
            stratum,
            poll,
            precision,
            root_delay,
            root_dispersion,
            reference_id,
            reference_time,
            offset,
            peer_delay,
            peer_dispersion,
            response_time,
            jitter_asymmetry,
            tests: flags & NTP_FLAGS_TESTS,
            interleaved: flags & NTP_FLAG_INTERLEAVED != 0,
            authenticated: flags & NTP_FLAG_AUTHENTICATED != 0,
            transmit_timestamp_source,
            receive_timestamp_source,
            total_transmitted,
            total_received,
            total_valid,
            total_good,
            timestamps,
        })
    }
}

impl Decode for SelectData {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = expect(command, reply, data, reply::SELECT_DATA)?;
        let reference_id = ReferenceId(r.u32()?);
        let address = or_reference_id(r.address()?, reference_id);
        let state = char::from(r.u8()?);
        let authenticated = r.u8()? != 0;
        let leap = leap(r.u8()?.into())?;
        r.skip(1)?;
        Ok(Self {
            reference_id,
            address,
            state,
            authenticated,
            leap,
            configured_options: select_options(r.u16()?),
            effective_options: select_options(r.u16()?),
            last_sample_ago: whole_seconds(&mut r)?,
            score: r.float()?,
            low_limit: seconds(&mut r)?,
            high_limit: seconds(&mut r)?,
        })
    }
}

impl Decode for Activity {
    fn decode(command: u16, reply: u16, data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = expect(command, reply, data, reply::ACTIVITY)?;
        Ok(Self {
            online: r.i32()?,
            offline: r.i32()?,
            burst_online: r.i32()?,
            burst_offline: r.i32()?,
            unresolved: r.i32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::ADDRESS_LENGTH;

    #[test]
    fn requests_are_padded_to_their_longest_reply() {
        let tracking = Request::Tracking.encode(7, 0);
        assert_eq!(tracking.len(), REPLY_HEADER_LENGTH + reply::TRACKING.1);
        assert_eq!(
            &tracking[..4],
            &[PROTOCOL_VERSION, PACKET_TYPE_REQUEST, 0, 0]
        );
        assert_eq!(&tracking[4..6], &command::TRACKING.to_be_bytes());
        assert_eq!(&tracking[8..12], &7u32.to_be_bytes());
        let ntp = Request::NtpData(IpAddr::from([192, 0, 2, 1])).encode(1, 0);
        assert_eq!(ntp.len(), REPLY_HEADER_LENGTH + reply::NTP_DATA2.1);
        assert_eq!(
            &ntp[REQUEST_HEADER_LENGTH..REQUEST_HEADER_LENGTH + ADDRESS_LENGTH],
            &address_bytes(IpAddr::from([192, 0, 2, 1]))
        );
    }

    #[test]
    fn a_reply_shorter_than_its_header_is_ignored() {
        let mut header = vec![PROTOCOL_VERSION, PACKET_TYPE_REPLY, 0, 0];
        header.extend_from_slice(&command::TRACKING.to_be_bytes());
        header.extend_from_slice(&reply::TRACKING.0.to_be_bytes());
        header.extend_from_slice(&STATUS_SUCCESS.to_be_bytes());
        header.extend_from_slice(&[0; 6]);
        header.extend_from_slice(&7u32.to_be_bytes());
        header.extend_from_slice(&[0; 8]);
        assert_eq!(header.len(), REPLY_HEADER_LENGTH);
        for length in 0..REPLY_HEADER_LENGTH {
            assert_eq!(
                ReplyHeader::parse(&header[..length]),
                None,
                "{length} bytes"
            );
        }
        let parsed = ReplyHeader::parse(&header).expect("a header");
        assert_eq!((parsed.command, parsed.sequence), (command::TRACKING, 7));
    }

    #[test]
    fn unknown_reply_type_is_unsupported() {
        let error = Tracking::decode(command::TRACKING, 99, &[0; 200]).unwrap_err();
        assert_eq!(
            error,
            DecodeError::UnsupportedReply {
                command: command::TRACKING,
                reply: 99
            }
        );
    }
}
