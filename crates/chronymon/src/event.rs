//! Things that happen to chronyd or its sources, which explain the
//! gaps and jumps in the charts.

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// chronymond started.
    DaemonStart,
    /// chronymond stopped.
    DaemonStop,
    /// chronymond's last run ended without stopping, on a boot that is
    /// still running: chronymond itself crashed or was killed.
    DaemonCrash,
    /// The host booted.
    Boot,
    /// The host began shutting down or powering off.
    Shutdown,
    /// The host's last boot ended without a shutdown: a kernel crash,
    /// a power loss or a hard reset.
    HostCrash,
    /// chronyd stopped answering on its socket.
    ChronydDown,
    /// chronyd answered again after being down.
    ChronydUp,
    /// chronyd restarted: its PID changed.
    ChronydRestart,
    /// chronyd died without cleaning up: its PID file names a process
    /// that is gone.
    ChronydCrash,
    /// The clock was stepped: an update's correction was past chronyd's
    /// `makestep` threshold, within its update limit.
    Step,
    /// An update's correction was past chronyd's `makestep` threshold
    /// (1 s without one), but slewed: past the update limit, or with no
    /// `makestep` at all.
    LargeOffset,
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
    /// chronyd's configuration could not be read as chronyd reads it;
    /// the daemon goes on with what it read last.
    ChronyConfigError,
    /// chronyd's configuration read again after a failure.
    ChronyConfigRead,
    /// A PPS device's pulse stopped landing: an edge missed.
    PpsLost,
    /// Its pulses landing again, after so many edges missed.
    PpsBack,
    /// A loss soon after the last: losses from here until pulses run
    /// unbroken a while are one flapping span.
    PpsFlapping,
    /// Pulses unbroken a while after flapping: the edges missed and the
    /// losses in the span.
    PpsSteady,
    /// A PPS device could not be read.
    PpsDown,
    /// It could again.
    PpsUp,
    /// gpsd could not be reached, or went away.
    GpsdDown,
    /// gpsd reached again.
    GpsdUp,
    /// A receiver lost its fix, as gpsd reports it: less than a 3D fix
    /// and not a surveyed-in position.
    FixLost,
    /// It has one again.
    FixRegained,
    /// The clock jumped against boot time between polls by more than any
    /// slew could, and `makestep`'s rule said no step: `chronyc
    /// makestep`, chronyd's `-R`, or something else setting the clock.
    ClockStep,
    /// `makestep`'s rule said a step, and the clock did not jump.
    StepUnseen,
}

impl EventKind {
    /// Every kind, for tests and for a reader decoding the stored name.
    pub const ALL: [Self; 31] = [
        Self::DaemonStart,
        Self::DaemonStop,
        Self::DaemonCrash,
        Self::Boot,
        Self::Shutdown,
        Self::HostCrash,
        Self::ChronydDown,
        Self::ChronydUp,
        Self::ChronydRestart,
        Self::ChronydCrash,
        Self::Step,
        Self::LargeOffset,
        Self::ReferenceChange,
        Self::LeapChange,
        Self::SourceAdded,
        Self::SourceRemoved,
        Self::Unsupported,
        Self::ChronyConfigError,
        Self::ChronyConfigRead,
        Self::PpsLost,
        Self::PpsBack,
        Self::PpsFlapping,
        Self::PpsSteady,
        Self::PpsDown,
        Self::PpsUp,
        Self::GpsdDown,
        Self::GpsdUp,
        Self::FixLost,
        Self::FixRegained,
        Self::ClockStep,
        Self::StepUnseen,
    ];

    /// The name stored for it and shown in the journal.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DaemonStart => "daemon_start",
            Self::DaemonStop => "daemon_stop",
            Self::DaemonCrash => "daemon_crash",
            Self::Boot => "boot",
            Self::Shutdown => "shutdown",
            Self::HostCrash => "host_crash",
            Self::ChronydDown => "chronyd_down",
            Self::ChronydUp => "chronyd_up",
            Self::ChronydRestart => "chronyd_restart",
            Self::ChronydCrash => "chronyd_crash",
            Self::Step => "step",
            Self::LargeOffset => "large_offset",
            Self::ReferenceChange => "reference_change",
            Self::LeapChange => "leap_change",
            Self::SourceAdded => "source_added",
            Self::SourceRemoved => "source_removed",
            Self::Unsupported => "unsupported",
            Self::ChronyConfigError => "chrony_config_error",
            Self::ChronyConfigRead => "chrony_config_read",
            Self::PpsLost => "pps_lost",
            Self::PpsBack => "pps_back",
            Self::PpsFlapping => "pps_flapping",
            Self::PpsSteady => "pps_steady",
            Self::PpsDown => "pps_down",
            Self::PpsUp => "pps_up",
            Self::GpsdDown => "gpsd_down",
            Self::GpsdUp => "gpsd_up",
            Self::FixLost => "fix_lost",
            Self::FixRegained => "fix_regained",
            Self::ClockStep => "clock_step",
            Self::StepUnseen => "step_unseen",
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
