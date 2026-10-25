//! Reading chronyd's log files into the database: every file of a log
//! chronymon knows in chronyd's log directory, rotated and compressed
//! ones included, each from where it was last read.
//!
//! A file is followed by inode, which logrotate's renames keep, not
//! device, whose number can change across boots (LVM, btrfs): the `tracking.log` read yesterday is today's `tracking.log.1`,
//! already read, and only what was appended since is new.  It is known
//! for the same file by the bytes before where its reading stopped: a
//! file truncated and written again, or a new one on a reused inode, is
//! read from its start.  A compressed file is read whole, once.  Lines read twice -- a file compressed under a
//! new inode -- are recognized by their contents and not written
//! again.

use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io;
use std::io::BufRead as _;
use std::io::BufReader;
use std::io::ErrorKind;
use std::io::Seek as _;
use std::io::SeekFrom;
use std::os::unix::fs::FileExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::path::PathBuf;

use chronymon::event::EventKind;
use chronymon_log::chrony_log::LogKind;
use chronymon_log::chrony_log::ParseError;
use chronymon_log::chrony_log::parse;
use chronymon_log::writer::FileId;
use chronymon_log::writer::Log;
use chronymon_log::writer::LogFile;
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
    /// Files that could not be read, left to try again.
    pub(crate) failed: usize,
}

/// chronyd's log directory, and what is wrong with the files in it that
/// could not be read.
#[derive(Debug)]
pub(crate) struct Ingest {
    directory: PathBuf,
    /// Each failing file's error, as last reported: one reported once,
    /// until it changes or the file reads.
    failing: HashMap<PathBuf, String>,
}

impl Ingest {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            failing: HashMap::new(),
        }
    }

    /// Read what is new in every log file.  A directory that cannot be
    /// read -- chronyd keeps no logs here -- is no error, and a file
    /// that cannot be read is reported, not read past, and tried again
    /// next scan: one damaged rotation stops nothing else.
    pub(crate) fn scan(&mut self, log: &mut Log) -> Scan {
        let mut total = Scan::default();
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return total;
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
        self.failing
            .retain(|failed, _| files.iter().any(|(_, _, path)| path == failed));
        for (_, kind, path) in files {
            match read_file(log, kind, &path) {
                Ok(scanned) => {
                    self.failing.remove(&path);
                    total.files += usize::from(scanned.lines > 0 || scanned.bad_lines > 0);
                    total.lines += scanned.lines;
                    total.bad_lines += scanned.bad_lines;
                }
                Err(e) => {
                    total.failed += 1;
                    let error = e.to_string();
                    if self.report(&path, &error) {
                        eprintln!("chronymond: {}: {error}", path.display());
                    }
                }
            }
        }
        total
    }

    /// Note `path` failing with `error`.  Whether that is news.
    fn report(&mut self, path: &Path, error: &str) -> bool {
        if self.failing.get(path).is_some_and(|known| known == error) {
            return false;
        }
        self.failing.insert(path.to_owned(), error.to_owned());
        true
    }
}

/// How many rotations old a file is: 0 for the live `.log`.
fn rotation(name: &str) -> u32 {
    name.split_once(".log")
        .map(|(_, rest)| rest.trim_start_matches(['.', '-']))
        .and_then(|rest| rest.trim_end_matches(".gz").parse().ok())
        .unwrap_or(0)
}

/// How many bytes before a file's checkpoint are kept to know it by: a
/// line or two, timestamps included.
const FINGERPRINT_LENGTH: u64 = 512;

/// Read what is new in one file.  An error leaves its checkpoint, and
/// the database, as they were.
fn read_file(log: &mut Log, kind: LogKind, path: &Path) -> anyhow::Result<Scan> {
    // Opened first, then the open file examined: what is read is what
    // was examined, whatever logrotate renames meanwhile.
    let file = match File::open(path) {
        // Gone since the directory was listed: rotated away.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Scan::default()),
        opened => opened?,
    };
    let metadata = file.metadata()?;
    let id = FileId {
        inode: metadata.ino(),
    };
    let length = metadata.len();
    let compressed = path.extension().is_some_and(|extension| extension == "gz");
    let known = log.write(|batch| batch.log_file(id))?;
    // Still the file read, not one truncated and rewritten, or a new one
    // on a reused inode: the bytes before its checkpoint are unchanged.
    let known = match known {
        Some(known)
            if known.read <= length && fingerprint(&file, known.read)? == known.fingerprint =>
        {
            Some(known)
        }
        _ => None,
    };
    let start = match known {
        Some(known) if known.rejected || known.length == length => return Ok(Scan::default()),
        Some(_) if compressed => 0,
        // Grown since: read on from where the last read stopped.
        Some(known) => known.read,
        // Replaced, or new.
        None => 0,
    };
    let mut reader: Box<dyn io::BufRead + '_> = if compressed {
        Box::new(BufReader::new(MultiGzDecoder::new(&file)))
    } else {
        (&file).seek(SeekFrom::Start(start))?;
        Box::new(BufReader::new(&file))
    };
    let path_text = path.to_string_lossy();
    log.write(|batch| {
        let mut scan = Scan::default();
        let mut read = start;
        let mut rejected = false;
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
                    // can be read, now or when it grows.
                    let detail = format!("{path_text}: {e}");
                    eprintln!("chronymond: {detail}");
                    batch.event(
                        Timestamp::now(),
                        EventKind::Unsupported,
                        None,
                        Some(&detail),
                    )?;
                    rejected = true;
                    break;
                }
            }
        }
        if compressed || rejected {
            read = length;
        }
        let checkpoint = LogFile {
            length: length.max(read),
            read,
            fingerprint: fingerprint(&file, read).map_err(io_error)?,
            rejected,
        };
        batch.set_log_file(id, &path_text, &checkpoint)?;
        Ok(scan)
    })
    .map_err(Into::into)
}

/// The bytes of `file` just before `end`, up to [`FINGERPRINT_LENGTH`].
fn fingerprint(file: &File, end: u64) -> io::Result<Vec<u8>> {
    let start = end.saturating_sub(FINGERPRINT_LENGTH);
    let mut bytes = vec![0; usize::try_from(end - start).unwrap_or(0)];
    file.read_exact_at(&mut bytes, start)?;
    Ok(bytes)
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
        let mut ingest = Ingest::new(logs.clone());
        let tracking = fixture("tracking.log");
        let lines: Vec<&str> = tracking.lines().collect();
        let (first, rest) = lines.split_at(10);
        let live = logs.join("tracking.log");

        // Part of a file, the last line unfinished.
        fs::write(&live, format!("{}\n2026-10-25 05:39", first.join("\n"))).expect("written");
        let partial = ingest.scan(&mut db.log);
        assert_eq!(partial.lines, 7, "{partial:?}");
        assert_eq!(db.count("log_tracking"), 7);

        // The rest arrives; then logrotate renames it and compresses it.
        fs::write(
            &live,
            format!("{}\n{}\n", first.join("\n"), rest.join("\n")),
        )
        .expect("written");
        assert_eq!(ingest.scan(&mut db.log).lines, 11);
        fs::rename(&live, logs.join("tracking.log.1")).expect("renamed");
        assert_eq!(ingest.scan(&mut db.log).lines, 0);
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(tracking.as_bytes()).expect("compressed");
        fs::write(
            logs.join("tracking.log.2.gz"),
            gz.finish().expect("finished"),
        )
        .expect("written");
        assert_eq!(ingest.scan(&mut db.log).lines, 0);
        assert_eq!(db.count("log_tracking"), 18);
    }

    #[test]
    fn refuses_a_changed_layout() {
        let mut db = Database::new("ingest-layout");
        let logs = db.directory().join("logs");
        fs::create_dir_all(&logs).expect("a directory");
        let mut ingest = Ingest::new(logs.clone());
        fs::write(
            logs.join("tempcomp.log"),
            "   Date (UTC) Time        Temp.       Comp.   New\n2026-10-25 05:39:33  4.1e+01  4.1e+00  1\n",
        )
        .expect("written");
        assert_eq!(ingest.scan(&mut db.log).lines, 0);
        assert_eq!(db.events(), ["unsupported"]);
        // And it is not reported again, nor read when it grows.
        ingest.scan(&mut db.log);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(logs.join("tempcomp.log"))
            .expect("opened");
        file.write_all(b"2026-10-25 05:39:34  4.1e+01  4.1e+00\n")
            .expect("appended");
        assert_eq!(ingest.scan(&mut db.log).lines, 0);
        assert_eq!(db.count("tempcomp_sample"), 0);
        assert_eq!(db.events(), ["unsupported"]);
    }

    /// `tempcomp.log` lines, one a second over `seconds`, each as long
    /// as the next.
    fn tempcomp(seconds: std::ops::Range<u32>) -> String {
        seconds
            .map(|second| format!("2026-10-25 05:39:{second:02}  4.1500e+04  4.1459e+00\n"))
            .collect()
    }

    #[test]
    fn rereads_a_file_rewritten_in_place() {
        let mut db = Database::new("ingest-rewritten");
        let logs = db.directory().join("logs");
        fs::create_dir_all(&logs).expect("a directory");
        let mut ingest = Ingest::new(logs.clone());
        let live = logs.join("tempcomp.log");
        fs::write(&live, tempcomp(0..10)).expect("written");
        assert_eq!(ingest.scan(&mut db.log).lines, 10);
        // Truncated, then grown past where it was read to: the same
        // inode, other lines.
        fs::write(&live, tempcomp(20..32)).expect("written");
        assert_eq!(ingest.scan(&mut db.log).lines, 12);
        // Rewritten to the same length.
        fs::write(&live, tempcomp(40..52)).expect("written");
        assert_eq!(ingest.scan(&mut db.log).lines, 12);
        assert_eq!(db.count("tempcomp_sample"), 34);
    }

    #[test]
    fn a_file_that_cannot_be_read_stops_nothing_else() {
        let mut db = Database::new("ingest-damaged");
        let logs = db.directory().join("logs");
        fs::create_dir_all(&logs).expect("a directory");
        let mut ingest = Ingest::new(logs.clone());
        let tracking = fixture("tracking.log");
        let damaged = logs.join("tracking.log.2.gz");
        fs::write(&damaged, b"\x1f\x8b\x08\0not gzip").expect("written");
        fs::write(logs.join("tracking.log"), &tracking).expect("written");
        // Listed, but gone by the time it is opened.
        std::os::unix::fs::symlink(logs.join("gone"), logs.join("tracking.log.1")).expect("linked");

        let scanned = ingest.scan(&mut db.log);
        assert_eq!((scanned.lines, scanned.failed), (18, 1), "{scanned:?}");
        // Reported once, not every scan.
        assert!(!ingest.report(&damaged, &ingest.failing[&damaged].clone()));

        // Mended, it is read: its checkpoint was never advanced.
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(tracking.as_bytes()).expect("compressed");
        fs::write(&damaged, gz.finish().expect("finished")).expect("written");
        assert_eq!(ingest.scan(&mut db.log).failed, 0);
        assert!(ingest.failing.is_empty());
    }
}
