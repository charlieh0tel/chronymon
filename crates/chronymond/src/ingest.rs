//! Reading chronyd's log files into the database: every file of a log
//! chronymon knows in chronyd's log directory, rotated and compressed
//! ones included, each from where it was last read.
//!
//! A file is followed by inode, which logrotate's renames keep: the
//! `tracking.log` read yesterday is today's `tracking.log.1`, already
//! read, and only what was appended since is new.  A compressed file
//! is read whole, once.  Lines read twice -- a file compressed under a
//! new inode -- are recognized by their contents and not written
//! again.

use std::fs;
use std::fs::File;
use std::io;
use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Seek as _;
use std::io::SeekFrom;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use chronymon::event::EventKind;
use chronymon_log::chrony_log::LogKind;
use chronymon_log::chrony_log::ParseError;
use chronymon_log::chrony_log::parse;
use chronymon_log::writer::Log;
use flate2::read::MultiGzDecoder;
use jiff::Timestamp;

/// What a scan found, for the daemon's log.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scan {
    /// Files read, wholly or in part.
    pub(crate) files: usize,
    /// Lines new to the database.
    pub(crate) lines: usize,
    /// Lines that fit no known layout, skipped.
    pub(crate) bad_lines: usize,
}

/// Read what is new in every log file in `directory`.  A directory
/// that cannot be read -- chronyd keeps no logs here -- is no error.
pub(crate) fn scan(log: &mut Log, directory: &Path) -> anyhow::Result<Scan> {
    let mut total = Scan::default();
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(total);
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            LogKind::of_file(&name).map(|kind| (name, kind, entry.path()))
        })
        .collect();
    // Oldest first, so a backfill writes history in order: chronyd's
    // `.log` last, rotations by their number down.
    files.sort_by_key(|(name, _, _)| std::cmp::Reverse(rotation(name)));
    for (_, kind, path) in files {
        let scanned = read_file(log, kind, &path)?;
        total.files += usize::from(scanned.lines > 0 || scanned.bad_lines > 0);
        total.lines += scanned.lines;
        total.bad_lines += scanned.bad_lines;
    }
    Ok(total)
}

/// How many rotations old a file is: 0 for the live `.log`.
fn rotation(name: &str) -> u32 {
    name.split_once(".log")
        .map(|(_, rest)| rest.trim_start_matches(['.', '-']))
        .and_then(|rest| rest.trim_end_matches(".gz").parse().ok())
        .unwrap_or(0)
}

fn read_file(log: &mut Log, kind: LogKind, path: &Path) -> anyhow::Result<Scan> {
    let metadata = fs::metadata(path)?;
    let (inode, length) = (metadata.ino(), metadata.len());
    let compressed = path.extension().is_some_and(|extension| extension == "gz");
    let known = log.write(|batch| batch.log_file(inode))?;
    let start = match known {
        Some((was, _)) if was == length => return Ok(Scan::default()),
        Some(_) if compressed => 0,
        // Grown since: read on from where the last read stopped.
        Some((_, read)) if read <= length => read,
        // Truncated, or new.
        _ => 0,
    };
    let file = File::open(path)?;
    let mut reader: Box<dyn io::BufRead> = if compressed {
        Box::new(BufReader::new(MultiGzDecoder::new(file)))
    } else {
        let mut file = file;
        file.seek(SeekFrom::Start(start))?;
        Box::new(BufReader::new(file))
    };
    let path_text = path.to_string_lossy();
    log.write(|batch| {
        let mut scan = Scan::default();
        let mut read = start;
        let mut line = Vec::new();
        loop {
            line.clear();
            let count = reader.read_until(b'\n', &mut line).map_err(io_error)?;
            // A line chronyd is still writing is left for next time.
            if count == 0 || (!compressed && line.last() != Some(&b'\n')) {
                break;
            }
            read += u64::try_from(count).unwrap_or(0);
            match parse(kind, &String::from_utf8_lossy(&line)) {
                Ok(Some(parsed)) => scan.lines += usize::from(batch.log_line(&parsed)?),
                Ok(None) => {}
                Err(ParseError::BadLine { .. }) => scan.bad_lines += 1,
                Err(e @ ParseError::UnknownHeader { .. }) => {
                    // The layout has changed: nothing more of this file
                    // can be read, and it is not to be tried again.
                    let detail = format!("{path_text}: {e}");
                    eprintln!("chronymond: {detail}");
                    batch.event(
                        Timestamp::now(),
                        EventKind::Unsupported,
                        None,
                        Some(&detail),
                    )?;
                    read = length;
                    break;
                }
            }
        }
        if compressed {
            read = length;
        }
        batch.set_log_file(inode, &path_text, length.max(read), read)?;
        Ok(scan)
    })
    .map_err(Into::into)
}

fn io_error(error: io::Error) -> chronymon_log::error::Error {
    chronymon_log::error::Error::Io(error)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;
    use crate::collect::tests::Database;

    fn fixture(name: &str) -> String {
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../chronymon-log/tests/logs")
                .join(name),
        )
        .expect("a fixture")
    }

    #[test]
    fn follows_files_through_rotation() {
        let mut db = Database::new("ingest");
        let logs = db.directory().join("logs");
        fs::create_dir_all(&logs).expect("a directory");
        let tracking = fixture("tracking.log");
        let lines: Vec<&str> = tracking.lines().collect();
        let (first, rest) = lines.split_at(10);
        let live = logs.join("tracking.log");

        // Part of a file, the last line unfinished.
        fs::write(&live, format!("{}\n2026-10-25 05:39", first.join("\n"))).expect("written");
        let partial = scan(&mut db.log, &logs).expect("scanned");
        assert_eq!(partial.lines, 7, "{partial:?}");
        assert_eq!(db.count("log_tracking"), 7);

        // The rest arrives; then logrotate renames it and compresses it.
        fs::write(
            &live,
            format!("{}\n{}\n", first.join("\n"), rest.join("\n")),
        )
        .expect("written");
        assert_eq!(scan(&mut db.log, &logs).expect("scanned").lines, 11);
        fs::rename(&live, logs.join("tracking.log.1")).expect("renamed");
        assert_eq!(scan(&mut db.log, &logs).expect("scanned").lines, 0);
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(tracking.as_bytes()).expect("compressed");
        fs::write(
            logs.join("tracking.log.2.gz"),
            gz.finish().expect("finished"),
        )
        .expect("written");
        assert_eq!(scan(&mut db.log, &logs).expect("scanned").lines, 0);
        assert_eq!(db.count("log_tracking"), 18);
    }

    #[test]
    fn refuses_a_changed_layout() {
        let mut db = Database::new("ingest-layout");
        let logs = db.directory().join("logs");
        fs::create_dir_all(&logs).expect("a directory");
        fs::write(
            logs.join("tempcomp.log"),
            "   Date (UTC) Time        Temp.       Comp.   New\n2026-10-25 05:39:33  4.1e+01  4.1e+00  1\n",
        )
        .expect("written");
        assert_eq!(scan(&mut db.log, &logs).expect("scanned").lines, 0);
        assert_eq!(db.events(), ["unsupported"]);
        // And it is not reported again.
        scan(&mut db.log, &logs).expect("scanned");
        assert_eq!(db.events(), ["unsupported"]);
    }
}
