//! Quantities chronyd reports, as types rather than bare floats, so an
//! offset cannot be passed where a frequency is meant.

use std::fmt;

/// A time interval or offset, in seconds.  Signed: an offset is
/// positive when the local clock is fast.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Seconds(f64);

/// A frequency or frequency error, in parts per million.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Ppm(f64);

macro_rules! unit {
    ($t:ident, $suffix:literal) => {
        impl $t {
            /// Wrap a value in this unit.
            pub const fn new(value: f64) -> Self {
                Self(value)
            }

            /// The underlying value.
            pub const fn get(self) -> f64 {
                self.0
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} {}", self.0, $suffix)
            }
        }
    };
}

unit!(Seconds, "s");
unit!(Ppm, "ppm");
