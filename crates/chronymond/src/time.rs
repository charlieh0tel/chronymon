//! Times as the PPS and gpsd watchers reckon them.

use jiff::Timestamp;

/// A nanosecond in seconds.
pub(crate) const NANOSECOND: f64 = 1e-9;

/// `at` in seconds since the epoch.
pub(crate) fn seconds(at: Timestamp) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "to the microsecond, enough for edges and reports a second apart"
    )]
    let nanoseconds = at.as_nanosecond() as f64;
    nanoseconds * NANOSECOND
}
