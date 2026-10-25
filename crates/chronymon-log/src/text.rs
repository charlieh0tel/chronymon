//! The text the database stores for chronyd's enumerations, named in
//! the schema's comments.

use chronymon_control::report::Leap;
use chronymon_control::report::SelectOptions;
use chronymon_control::report::SourceMode;
use chronymon_control::report::SourceState;

pub(crate) const fn leap(leap: Leap) -> &'static str {
    match leap {
        Leap::Normal => "normal",
        Leap::Insert => "insert",
        Leap::Delete => "delete",
        Leap::Unsynchronized => "unsynchronized",
    }
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
