//! The events of the daemon's own start and stop, and of the host's
//! boots, shutdowns and crashes, told apart by what the last run left
//! in the database.

use chronymon::event::EventKind;
use chronymon_log::error::Result;
use chronymon_log::writer::Batch;
use jiff::Timestamp;

/// `meta` keys: whether a run is in progress, and the boot it is on.
const RUNNING: &str = "daemon_running";
const BOOT_ID: &str = "boot_id";
const RUNNING_YES: &str = "1";
const RUNNING_NO: &str = "0";

/// What the host says about itself, given rather than read so tests
/// can say it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Host {
    /// This boot's ID.
    pub(crate) boot_id: Option<String>,
    /// Whether pstore holds a kernel crash record.
    pub(crate) kernel_crash_recorded: bool,
}

/// Record a start at `at`: a boot if the boot ID is new, and how the
/// last run ended if it did not stop.
pub(crate) fn start(batch: &Batch<'_>, at: Timestamp, host: &Host) -> Result<()> {
    let unclean = batch.meta(RUNNING)?.as_deref() == Some(RUNNING_YES);
    let last_boot = batch.meta(BOOT_ID)?;
    let new_boot = match (&last_boot, &host.boot_id) {
        (Some(last), Some(now)) => last != now,
        // A first run, or a host without boot IDs: no boot to report.
        _ => false,
    };
    if unclean {
        if new_boot {
            let detail = if host.kernel_crash_recorded {
                "kernel crash: pstore holds a record"
            } else {
                "power loss, hard reset or kernel crash"
            };
            batch.event(at, EventKind::HostCrash, None, Some(detail))?;
        } else {
            batch.event(at, EventKind::DaemonCrash, None, None)?;
        }
    }
    if new_boot {
        batch.event(at, EventKind::Boot, None, host.boot_id.as_deref())?;
    }
    batch.event(
        at,
        EventKind::DaemonStart,
        None,
        Some(env!("CARGO_PKG_VERSION")),
    )?;
    batch.set_meta(RUNNING, RUNNING_YES)?;
    if let Some(id) = &host.boot_id {
        batch.set_meta(BOOT_ID, id)?;
    }
    Ok(())
}

/// Record a stop at `at`, and a shutdown if the system is going down.
pub(crate) fn stop(batch: &Batch<'_>, at: Timestamp, system_stopping: bool) -> Result<()> {
    if system_stopping {
        batch.event(at, EventKind::Shutdown, None, None)?;
    }
    batch.event(at, EventKind::DaemonStop, None, None)?;
    batch.set_meta(RUNNING, RUNNING_NO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::tests::Database;

    fn host(boot: &str) -> Host {
        Host {
            boot_id: Some(boot.to_owned()),
            kernel_crash_recorded: false,
        }
    }

    #[test]
    fn tells_boots_and_crashes_apart() {
        let mut db = Database::new("lifecycle");
        let at = Timestamp::new(1_800_000_000, 0).expect("in range");
        let start = |db: &mut Database, boot: &str| {
            db.log
                .write(|batch| super::start(batch, at, &host(boot)))
                .expect("started");
        };
        let stop = |db: &mut Database, stopping: bool| {
            db.log
                .write(|batch| super::stop(batch, at, stopping))
                .expect("stopped");
        };
        // First run; then a restart of the service.
        start(&mut db, "a");
        stop(&mut db, false);
        start(&mut db, "a");
        // Killed; started again on the same boot.
        start(&mut db, "a");
        // The host powers off cleanly and boots.
        stop(&mut db, true);
        start(&mut db, "b");
        // The host loses power and boots.
        start(&mut db, "c");
        assert_eq!(
            db.events(),
            [
                "daemon_start",
                "daemon_stop",
                "daemon_start",
                "daemon_crash",
                "daemon_start",
                "shutdown",
                "daemon_stop",
                "boot",
                "daemon_start",
                "host_crash",
                "boot",
                "daemon_start",
            ]
        );
    }
}
