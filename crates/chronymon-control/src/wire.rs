//! chronyd's encodings of numbers, times and addresses, all big-endian
//! (`candm.h`, `util.c` in chrony 4.3 to 4.9).

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

use jiff::Timestamp;

use crate::error::DecodeError;
use crate::report::Address;

/// Bits of exponent in chrony's `Float`; the rest of the 32 are the
/// coefficient, both signed, with no hidden bit.
const FLOAT_EXPONENT_BITS: u32 = 7;
const FLOAT_COEFFICIENT_BITS: u32 = 32 - FLOAT_EXPONENT_BITS;

/// `tv_sec_high` of a timestamp that fits in 32 bits (`TV_NOHIGHSEC`).
const NO_HIGH_SECONDS: u32 = 0x7fff_ffff;
/// The most nanoseconds chrony accepts in a timestamp; it clamps more.
const MAX_NANOSECONDS: u32 = 999_999_999;

/// Bytes of an `IPAddr`: a 16-byte union, the family, padding.
pub(crate) const ADDRESS_LENGTH: usize = 20;
const ADDRESS_UNION_LENGTH: usize = 16;

/// `IPAddr` family codes (`addressing.h`).
const FAMILY_UNSPECIFIED: u16 = 0;
const FAMILY_INET4: u16 = 1;
const FAMILY_INET6: u16 = 2;
const FAMILY_ID: u16 = 3;

/// A cursor over a reply's bytes.  Every read checks the length, so a
/// short reply is an error, never a panic.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub(crate) fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let end = self.at + N;
        let slice = self.bytes.get(self.at..end).ok_or(DecodeError::Truncated {
            length: self.bytes.len(),
            needed: end,
        })?;
        self.at = end;
        // The slice is exactly N long, so this cannot fail.
        Ok(slice.try_into().unwrap_or([0; N]))
    }

    pub(crate) fn skip(&mut self, count: usize) -> Result<(), DecodeError> {
        let end = self.at + count;
        if end > self.bytes.len() {
            return Err(DecodeError::Truncated {
                length: self.bytes.len(),
                needed: end,
            });
        }
        self.at = end;
        Ok(())
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(u8::from_be_bytes(self.take()?))
    }

    pub(crate) fn i8(&mut self) -> Result<i8, DecodeError> {
        Ok(i8::from_be_bytes(self.take()?))
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.take()?))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, DecodeError> {
        Ok(i16::from_be_bytes(self.take()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.take()?))
    }

    /// chrony's `Float`: `2^(exponent - 25) * coefficient`.
    pub(crate) fn float(&mut self) -> Result<f64, DecodeError> {
        Ok(float_from_bits(self.u32()?))
    }

    /// chrony's `Timespec`: high and low halves of the seconds, then
    /// nanoseconds.
    pub(crate) fn timestamp(&mut self) -> Result<Timestamp, DecodeError> {
        let high = self.u32()?;
        let low = self.u32()?;
        let nanoseconds = self.u32()?.min(MAX_NANOSECONDS);
        let high = if high == NO_HIGH_SECONDS { 0 } else { high };
        let seconds = i64::from(high) << 32 | i64::from(low);
        // In range: at most 999,999,999 nanoseconds.
        let nanoseconds = i32::try_from(nanoseconds).unwrap_or(0);
        Timestamp::new(seconds, nanoseconds).map_err(|_| DecodeError::Timestamp(seconds))
    }

    /// chrony's `IPAddr`.
    pub(crate) fn address(&mut self) -> Result<Address, DecodeError> {
        let union: [u8; ADDRESS_UNION_LENGTH] = self.take()?;
        let family = self.u16()?;
        self.skip(2)?;
        let [a, b, c, d, ..] = union;
        Ok(match family {
            FAMILY_UNSPECIFIED => Address::Unspecified,
            FAMILY_INET4 => Address::Ip(IpAddr::V4(Ipv4Addr::new(a, b, c, d))),
            FAMILY_INET6 => Address::Ip(IpAddr::V6(Ipv6Addr::from(union))),
            FAMILY_ID => Address::Unresolved(u32::from_be_bytes([a, b, c, d])),
            other => return Err(DecodeError::AddressFamily(other)),
        })
    }
}

fn float_from_bits(bits: u32) -> f64 {
    let exponent_range = 1i32 << FLOAT_EXPONENT_BITS;
    let coefficient_range = 1i64 << FLOAT_COEFFICIENT_BITS;
    // Both fit their types: 7 and 25 bits.
    let exponent = i32::try_from(bits >> FLOAT_COEFFICIENT_BITS).unwrap_or(0);
    let exponent = if exponent >= exponent_range / 2 {
        exponent - exponent_range
    } else {
        exponent
    };
    let coefficient = i64::from(bits) % coefficient_range;
    let coefficient = if coefficient >= coefficient_range / 2 {
        coefficient - coefficient_range
    } else {
        coefficient
    };
    // A 25-bit coefficient is exact in an f64.
    #[expect(clippy::cast_precision_loss, reason = "25 bits fit an f64 exactly")]
    let coefficient = coefficient as f64;
    coefficient * 2f64.powi(exponent - i32::try_from(FLOAT_COEFFICIENT_BITS).unwrap_or(0))
}

/// chrony's `IPAddr` for an address in a request.
pub(crate) fn address_bytes(address: IpAddr) -> [u8; ADDRESS_LENGTH] {
    let mut bytes = [0; ADDRESS_LENGTH];
    let family = match address {
        IpAddr::V4(v4) => {
            bytes[..4].copy_from_slice(&v4.octets());
            FAMILY_INET4
        }
        IpAddr::V6(v6) => {
            bytes[..ADDRESS_UNION_LENGTH].copy_from_slice(&v6.octets());
            FAMILY_INET6
        }
    };
    bytes[ADDRESS_UNION_LENGTH..ADDRESS_UNION_LENGTH + 2].copy_from_slice(&family.to_be_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits for `coefficient * 2^(exponent - 25)`, as chrony packs them.
    fn bits(exponent: i32, coefficient: i32) -> u32 {
        let exponent = u32::from_ne_bytes(exponent.to_ne_bytes()) & 0x7f;
        let coefficient = u32::from_ne_bytes(coefficient.to_ne_bytes()) & 0x01ff_ffff;
        exponent << FLOAT_COEFFICIENT_BITS | coefficient
    }

    #[test]
    fn float_decodes_sign_and_exponent() {
        assert_eq!(float_from_bits(0), 0.0);
        assert_eq!(float_from_bits(bits(25, 1)), 1.0);
        assert_eq!(float_from_bits(bits(25, -1)), -1.0);
        assert_eq!(float_from_bits(bits(1, 1 << 23)), 0.5);
        assert_eq!(float_from_bits(bits(-9, 1 << 23)), 2f64.powi(-11));
        assert_eq!(
            float_from_bits(bits(-64, -(1 << 24))),
            -(2f64.powi(-89 + 24))
        );
    }

    #[test]
    fn timestamp_drops_no_high_marker() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&NO_HIGH_SECONDS.to_be_bytes());
        bytes.extend_from_slice(&1_700_000_000u32.to_be_bytes());
        bytes.extend_from_slice(&5u32.to_be_bytes());
        let timestamp = Reader::new(&bytes).timestamp().expect("a timestamp");
        assert_eq!(
            timestamp,
            Timestamp::new(1_700_000_000, 5).expect("in range")
        );
    }

    #[test]
    fn address_round_trips() {
        for address in [
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        ] {
            let bytes = address_bytes(address);
            let decoded = Reader::new(&bytes).address().expect("an address");
            assert_eq!(decoded, Address::Ip(address));
        }
    }

    #[test]
    fn an_unresolved_source_is_its_placeholder() {
        let mut bytes = [0; ADDRESS_LENGTH];
        bytes[..4].copy_from_slice(&7u32.to_be_bytes());
        bytes[ADDRESS_UNION_LENGTH..ADDRESS_UNION_LENGTH + 2]
            .copy_from_slice(&FAMILY_ID.to_be_bytes());
        let decoded = Reader::new(&bytes).address().expect("an address");
        assert_eq!(decoded, Address::Unresolved(7));
        assert_eq!(decoded.to_string(), "ID#0000000007");
    }

    #[test]
    fn short_input_is_an_error() {
        assert!(matches!(
            Reader::new(&[0, 1]).u32(),
            Err(DecodeError::Truncated {
                length: 2,
                needed: 4
            })
        ));
    }
}
