//! The text the database stores for chronyd's enumerations, named in
//! the schema's comments.

use chronymon_control::report::Leap;
use chronymon_control::report::SelectOptions;
use chronymon_control::report::SourceMode;
use chronymon_control::report::SourceState;
use chronymon_control::report::Tracking;

pub(crate) const fn leap(leap: Leap) -> &'static str {
    match leap {
        Leap::Normal => "normal",
        Leap::Insert => "insert",
        Leap::Delete => "delete",
        Leap::Unsynchronized => "unsynchronized",
    }
}

/// A leap change's detail: the statuses before and after by their
/// `Debug` names, "Normal to Unsynchronized", as the daemon has always
/// written it.
pub(crate) fn leap_change(before: Leap, now: Leap) -> String {
    format!("{before:?} to {now:?}")
}

/// A reference change's detail: the references' addresses before and
/// after, "192.0.2.1 to 192.0.2.2".
pub(crate) fn reference_change(before: &Tracking, now: &Tracking) -> String {
    format!("{} to {}", before.address, now.address)
}

pub(crate) const fn mode(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Client => "client",
        SourceMode::Peer => "peer",
        SourceMode::Refclock => "refclock",
    }
}

pub(crate) const fn state(state: SourceState) -> &'static str {
    match state {
        SourceState::Selected => "selected",
        SourceState::Nonselectable => "nonselectable",
        SourceState::Falseticker => "falseticker",
        SourceState::Jittery => "jittery",
        SourceState::Combined => "combined",
        SourceState::NotCombined => "not_combined",
    }
}

/// `RPY_SD_OPTION_*` bits, as chronyd sends them.
pub(crate) fn options(options: SelectOptions) -> i64 {
    [
        options.noselect,
        options.prefer,
        options.trust,
        options.require,
    ]
    .into_iter()
    .enumerate()
    .map(|(bit, set)| i64::from(set) << bit)
    .sum()
}
