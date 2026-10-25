//! What chronymond needs from chronyd's configuration: its `makestep`
//! threshold, which decides when a correction is a step.
//!
//! Read from the main file and the directories its `confdir` and
//! `sourcedir` directives name, as chronyd reads them (`chrony.conf`
//! documentation, 4.3 to 4.9); `include` is followed too.  The last
//! `makestep` wins, as in chronyd.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use chronymon::units::Seconds;

/// The threshold assumed when chronyd's configuration sets none.
pub(crate) const DEFAULT_STEP_THRESHOLD: Seconds = Seconds::new(1.0);

/// How deep `include`s are followed, against a loop.
const MAX_DEPTH: usize = 8;

/// The `makestep` threshold in the configuration at `path`, or
/// [`DEFAULT_STEP_THRESHOLD`].
pub(crate) fn step_threshold(path: &Path) -> Seconds {
    let mut threshold = None;
    read(path, 0, &mut threshold);
    threshold.unwrap_or(DEFAULT_STEP_THRESHOLD)
}

fn read(path: &Path, depth: usize, threshold: &mut Option<Seconds>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.split(['#', ';', '!', '%']).next().unwrap_or("");
        let mut words = line.split_whitespace();
        match words.next() {
            Some("makestep") => {
                if let Some(value) = words.next().and_then(|word| word.parse::<f64>().ok()) {
                    *threshold = Some(Seconds::new(value));
                }
            }
            Some("include") => {
                for file in words.flat_map(glob) {
                    read(&file, depth + 1, threshold);
                }
            }
            Some("confdir" | "sourcedir") => {
                for directory in words {
                    for file in glob(&format!("{directory}/*.conf")) {
                        read(&file, depth + 1, threshold);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The files a pattern names: a literal path, or `*` in its last
/// component, which is all chrony's own configuration uses.
fn glob(pattern: &str) -> Vec<PathBuf> {
    let path = Path::new(pattern);
    let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let name = name.to_string_lossy();
    let Some((prefix, suffix)) = name.split_once('*') else {
        return vec![path.to_path_buf()];
    };
    let mut files: Vec<PathBuf> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|file| {
            file.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
        })
        .collect();
    // chronyd reads them in name order.
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_makestep_through_confdir() {
        let directory =
            std::env::temp_dir().join(format!("chronymond-config-{}", std::process::id()));
        let conf_d = directory.join("conf.d");
        fs::create_dir_all(&conf_d).expect("directories");
        let main = directory.join("chrony.conf");
        fs::write(
            &main,
            format!(
                "pool 2.debian.pool.ntp.org iburst\nmakestep 1 3 # comment\nconfdir {}\n",
                conf_d.display()
            ),
        )
        .expect("written");
        assert_eq!(step_threshold(&main), Seconds::new(1.0));
        fs::write(conf_d.join("local.conf"), "makestep 0.1 -1\n").expect("written");
        assert_eq!(step_threshold(&main), Seconds::new(0.1));
        assert_eq!(
            step_threshold(&directory.join("missing.conf")),
            DEFAULT_STEP_THRESHOLD
        );
        fs::remove_dir_all(&directory).expect("removed");
    }
}
