//! Timestamps as the database holds them: integer nanoseconds since
//! the Unix epoch, which 64 bits hold until 2262.

use jiff::Timestamp;

use crate::error::Error;
use crate::error::Result;

/// `at` as stored.
pub fn to_stored(at: Timestamp) -> Result<i64> {
    i64::try_from(at.as_nanosecond()).map_err(|_| Error::Timestamp)
}

/// A stored time as a timestamp.
pub fn from_stored(stored: i64) -> Result<Timestamp> {
    Timestamp::from_nanosecond(i128::from(stored)).map_err(|_| Error::Timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let at = Timestamp::new(1_792_903_905, 916_821_663).expect("in range");
        assert_eq!(
            from_stored(to_stored(at).expect("fits")).expect("valid"),
            at
        );
    }
}
