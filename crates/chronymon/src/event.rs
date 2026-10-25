//! Things that happen to chronyd or its sources, which explain the
//! gaps and jumps in the charts.

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// chronymond started.
    DaemonStart,
    /// chronymond stopped.
    DaemonStop,
    /// chronyd stopped answering on its socket.
    ChronydDown,
    /// chronyd answered again after being down.
    ChronydUp,
    /// chronyd restarted: its PID changed.
    ChronydRestart,
    /// The clock was stepped: the correction jumped past chronyd's
    /// `makestep` threshold.
    Step,
    /// The clock's reference changed to another source.
    ReferenceChange,
    /// The clock's leap status changed.
    LeapChange,
    /// A source appeared.
    SourceAdded,
    /// A source went away.
    SourceRemoved,
    /// chronyd sent a reply this version cannot decode.
    Unsupported,
}

impl EventKind {
    /// Every kind, for tests and for a reader decoding the stored name.
    pub const ALL: [Self; 11] = [
        Self::DaemonStart,
        Self::DaemonStop,
        Self::ChronydDown,
        Self::ChronydUp,
        Self::ChronydRestart,
        Self::Step,
        Self::ReferenceChange,
        Self::LeapChange,
        Self::SourceAdded,
        Self::SourceRemoved,
        Self::Unsupported,
    ];

    /// The name stored for it and shown in the journal.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DaemonStart => "daemon_start",
            Self::DaemonStop => "daemon_stop",
            Self::ChronydDown => "chronyd_down",
            Self::ChronydUp => "chronyd_up",
            Self::ChronydRestart => "chronyd_restart",
            Self::Step => "step",
            Self::ReferenceChange => "reference_change",
            Self::LeapChange => "leap_change",
            Self::SourceAdded => "source_added",
            Self::SourceRemoved => "source_removed",
            Self::Unsupported => "unsupported",
        }
    }

    /// The kind stored under `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_name(kind.name()), Some(kind));
        }
    }
}
