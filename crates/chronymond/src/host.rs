//! Facts about the host and chronyd's process, from `/proc`, `/run`
//! and systemd: what tells a boot, a shutdown and a crash apart.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

/// The kernel's ID for this boot, new on every boot.
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// Where pstore keeps what a crashing kernel wrote, and where
/// systemd-pstore moves it at the next boot.
const PSTORE: [&str; 2] = ["/sys/fs/pstore", "/var/lib/systemd/pstore"];

/// This boot's ID, if the kernel says.
pub(crate) fn boot_id() -> Option<String> {
    fs::read_to_string(BOOT_ID)
        .ok()
        .map(|id| id.trim().to_owned())
}

/// Whether systemd is taking the system down: what tells a shutdown or
/// power-off from someone stopping the service.
pub(crate) fn system_stopping() -> bool {
    Command::new("systemctl")
        .arg("is-system-running")
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "stopping")
}

/// Whether pstore holds a kernel's crash record, which tells a kernel
/// crash from a power loss when the last boot ended uncleanly.
pub(crate) fn kernel_crash_recorded() -> bool {
    PSTORE
        .iter()
        .any(|directory| fs::read_dir(directory).is_ok_and(|mut entries| entries.next().is_some()))
}

/// What chronyd's PID file says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Process {
    /// No PID file: chronyd is not running, or exited cleanly.
    Absent,
    /// The PID file names a running process.
    Running(u32),
    /// The PID file names a process that is gone: chronyd died without
    /// removing it.
    Dead(u32),
    /// The file could not be read: no permission, or not a PID.
    Unknown,
}

/// What the PID file at `path` says.
pub(crate) fn chronyd(path: &Path) -> Process {
    match fs::read_to_string(path) {
        Ok(text) => match text.trim().parse::<u32>() {
            Ok(pid) if Path::new(&format!("/proc/{pid}")).exists() => Process::Running(pid),
            Ok(pid) => Process::Dead(pid),
            Err(_) => Process::Unknown,
        },
        Err(e) if e.kind() == ErrorKind::NotFound => Process::Absent,
        Err(_) => Process::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_pid_file() {
        let directory =
            std::env::temp_dir().join(format!("chronymond-host-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("chronyd.pid");
        assert_eq!(chronyd(&path), Process::Absent);
        fs::write(&path, format!("{}\n", std::process::id())).expect("written");
        assert_eq!(chronyd(&path), Process::Running(std::process::id()));
        // PID_MAX_LIMIT is 2^22, so this one is never running.
        fs::write(&path, "4194305\n").expect("written");
        assert_eq!(chronyd(&path), Process::Dead(4_194_305));
        fs::write(&path, "nonsense").expect("written");
        assert_eq!(chronyd(&path), Process::Unknown);
        fs::remove_dir_all(&directory).expect("removed");
    }

    #[test]
    fn this_boot_has_an_id() {
        assert!(boot_id().is_some_and(|id| !id.is_empty()));
    }
}
