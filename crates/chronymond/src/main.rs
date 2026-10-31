//! chronymond: polls chronyd's control socket and records what it
//! reports, and what happens to it and the host, in chronymon's
//! database.  See `PLAN.md`.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use chronymon_control::client::Client;
use chronymon_control::client::DEFAULT_SOCKET;
use chronymon_control::client::Server;
use chronymon_control::error::Error as ControlError;
use chronymon_control::error::Status;
use chronymon_log::writer::ChronydState;
use chronymon_log::writer::Log;
use clap::Parser;
use jiff::Timestamp;
use signal_hook::consts::SIGINT;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;

use crate::collect::Collector;
use crate::config::ConfigSource;
use crate::ingest::Ingest;
use crate::lifecycle::Host;
use crate::pps::Device as PpsDevice;
use crate::pps::Pps;

mod collect;
mod config;
mod host;
mod ingest;
mod lifecycle;
mod pps;

/// Options, each also settable in the environment, which is how
/// `/etc/default/chronymond` sets them.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// chronyd's control socket, a path, or its UDP command port, an
    /// address: `127.0.0.1`, `[::1]:323`.  Over UDP chronyd must open
    /// the reports read (chrony 4.7's `opencommands`).
    #[arg(long, env = "CHRONYMOND_SOCKET", default_value = DEFAULT_SOCKET)]
    socket: Server,

    /// The database to write.
    #[arg(
        long,
        env = "CHRONYMOND_DATABASE",
        default_value = "/var/lib/chronymon/chronymon.sqlite"
    )]
    database: PathBuf,

    /// Seconds between polls.
    #[arg(long, env = "CHRONYMOND_INTERVAL", default_value_t = 1.0)]
    interval: f64,

    /// chronyd's configuration, for its `makestep` and its refclocks'
    /// locks; read again whenever chronyd restarts.  `none` for a
    /// chronyd configured on its command line or on another host.
    #[arg(
        long,
        env = "CHRONYMOND_CHRONY_CONFIG",
        default_value = "/etc/chrony/chrony.conf"
    )]
    chrony_config: ConfigSource,

    /// chronyd's log directory, read for what the control protocol does
    /// not give: every sample, refclocks, tempcomp, and history from
    /// before the daemon ran.
    #[arg(
        long,
        env = "CHRONYMOND_CHRONY_LOGS",
        default_value = "/var/log/chrony"
    )]
    chrony_logs: PathBuf,

    /// Seconds between reads of chronyd's logs.
    #[arg(long, env = "CHRONYMOND_LOG_INTERVAL", default_value_t = 10.0)]
    log_interval: f64,

    /// PPS devices to watch for missed edges, comma-separated:
    /// `/dev/ppsN`, or `/dev/ptpN[:channel]`, a clock timestamping the
    /// pulse on a pin (chrony's `refclock PHC ...:extpps`), channel 0 by
    /// default.  A clock needs Linux 6.7 or later.
    #[arg(long, env = "CHRONYMOND_PPS", value_delimiter = ',')]
    pps: Vec<PpsDevice>,

    /// chronyd's PID file, which tells a restart or a crash.
    #[arg(
        long,
        env = "CHRONYMOND_PID_FILE",
        default_value = "/run/chrony/chronyd.pid"
    )]
    pid_file: PathBuf,
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chronymond: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// What failed in reaching chronyd's socket, or setting up the client's.
fn io_kind(error: &ControlError) -> Option<ErrorKind> {
    match error {
        ControlError::Connect { source, .. } | ControlError::Socket { source, .. } => {
            Some(source.kind())
        }
        _ => None,
    }
}

fn refused(error: &ControlError) -> bool {
    io_kind(error) == Some(ErrorKind::PermissionDenied)
}

/// Whether the client's directory beside chronyd's socket could not be
/// made for a read-only file system: under systemd, `/run/chrony` made
/// after the daemon started, which its mount namespace has read-only.
/// Only a restart, with a new namespace, makes it writable.
fn read_only(error: &ControlError) -> bool {
    io_kind(error) == Some(ErrorKind::ReadOnlyFilesystem)
}

/// The reports a poll reads, as `opencommands` names them: what chronyd
/// must open for the daemon to use UDP.
const OPEN_COMMANDS: &str = "sources sourcestats sourcename tracking ntpdata selectdata";

/// Whether chronyd refused a report: over UDP, one it does not open.
fn unauthorized(error: &ControlError) -> bool {
    matches!(error, ControlError::Status(Status::Unauthorized))
}

fn seconds(value: f64, option: &str) -> Result<Duration> {
    Duration::try_from_secs_f64(value)
        .ok()
        .filter(|interval| !interval.is_zero())
        .with_context(|| format!("{option} must be a positive number of seconds"))
}

fn bad_lines(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(", {count} lines of no known layout skipped")
    }
}

fn run(cli: &Cli) -> Result<()> {
    let interval = seconds(cli.interval, "--interval")?;
    let log_interval = seconds(cli.log_interval, "--log-interval")?;
    // Refused permission is fatal: no retry fixes the user the daemon
    // runs as.  Anything else -- chronyd not running yet -- is recorded
    // as chronyd down, and retried.
    let mut client = match Client::connect(&cli.socket) {
        Ok(client) => Some(client),
        Err(e) if refused(&e) => anyhow::bail!(
            "{e}: the daemon must run as chronyd's user (_chrony on Debian and Ubuntu) or root"
        ),
        Err(_) => None,
    };
    let mut log =
        Log::open(&cli.database).with_context(|| format!("opening {}", cli.database.display()))?;
    let mut collector = Collector::new(cli.chrony_config.clone());
    eprintln!(
        "chronymond: version {}, {} every {interval:?}, {}",
        env!("CARGO_PKG_VERSION"),
        cli.socket,
        collector.config()
    );
    let (mut pps, refused) = Pps::new(&cli.pps);
    for refusal in &refused {
        eprintln!("chronymond: {refusal}");
    }
    if !cli.pps.is_empty() {
        let names: Vec<String> = cli.pps.iter().map(ToString::to_string).collect();
        eprintln!("chronymond: watching PPS {}", names.join(", "));
    }

    let (stop, stopped) = mpsc::channel();
    let mut signals = Signals::new([SIGTERM, SIGINT]).context("watching for signals")?;
    thread::Builder::new()
        .name("chronymond-signals".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                eprintln!("chronymond: signal {signal}, stopping");
                if stop.send(()).is_err() {
                    break;
                }
            }
        })
        .context("starting the signal thread")?;

    let host = Host {
        boot_id: host::boot_id(),
        kernel_crash_recorded: host::kernel_crash_recorded(),
    };
    log.write(|batch| lifecycle::start(batch, Timestamp::now(), &host))?;

    // The backfill: everything in chronyd's logs not yet recorded.
    let mut ingest = Ingest::new(cli.chrony_logs.clone());
    let backfill = ingest.scan(&mut log);
    eprintln!(
        "chronymond: {} lines from {} log files in {}{}",
        backfill.lines,
        backfill.files,
        cli.chrony_logs.display(),
        bad_lines(backfill.bad_lines)
    );
    let mut logs_read = Instant::now();

    let mut state = None;
    let mut next = Instant::now();
    // Why the loop stopped, if not for a signal.
    let mut failed = None;
    loop {
        let at = Timestamp::now();
        // A fresh socket after any failure: chronyd may have restarted.
        let read = match client
            .take()
            .map_or_else(|| Client::connect(&cli.socket), Ok)
        {
            Ok(mut connected) => {
                let read = collector.read(&mut connected);
                if read.is_ok() {
                    client = Some(connected);
                }
                read
            }
            Err(e) => Err(e),
        };
        if let Err(e) = &read
            && read_only(e)
        {
            failed = Some(format!(
                "{e}: exiting, for a restart to find its directory writable"
            ));
            break;
        }
        // Like refused permission, fatal: chronyd's configuration must
        // change.
        if let Err(e) = &read
            && unauthorized(e)
        {
            failed = Some(format!(
                "{e}: chronyd must open, in chrony.conf, `opencommands {OPEN_COMMANDS}` (chrony 4.7 on)"
            ));
            break;
        }
        if let Err(e) = &read {
            let now = collect::state_of(e);
            if state != Some(now) {
                eprintln!("chronymond: chronyd {}: {e}", now.name());
            }
            state = Some(now);
        } else {
            if state.is_some_and(|state| state != ChronydState::Up) {
                eprintln!("chronymond: chronyd up");
            }
            state = Some(ChronydState::Up);
        }
        let process = host::chronyd(&cli.pid_file);
        log.write(|batch| {
            collector.record(batch, at, read, process)?;
            pps.poll(batch, at)
        })?;
        if logs_read.elapsed() >= log_interval {
            let scan = ingest.scan(&mut log);
            if scan.bad_lines > 0 {
                eprintln!("chronymond: logs:{}", bad_lines(scan.bad_lines));
            }
            logs_read = Instant::now();
        }

        next += interval;
        let now = Instant::now();
        if next < now {
            // Fell behind (a suspend, a slow disk): start again from now
            // rather than poll in a burst to catch up.
            next = now;
        }
        match stopped.recv_timeout(next - now) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // What chronyd logged since the last read.
    ingest.scan(&mut log);
    let system_stopping = host::system_stopping();
    log.write(|batch| lifecycle::stop(batch, Timestamp::now(), system_stopping))?;
    if let Some(e) = failed {
        anyhow::bail!(e);
    }
    eprintln!("chronymond: stopped");
    Ok(())
}
