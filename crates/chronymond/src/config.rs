//! What chronymond needs from chronyd's configuration: its `makestep`,
//! which decides when a correction is a step.
//!
//! Read as chronyd reads it (`CNF_ReadFile`, `conf.c`, and the
//! `chrony.conf` documentation, 4.3 to 4.9): the main file, the files
//! its `include` patterns name, and the `.conf` files of its `confdir`
//! directories.  `sourcedir` files can hold only NTP sources, so are
//! not read.  The last `makestep` wins, as in chronyd.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use chronymon::units::Seconds;

/// The correction recorded as a large offset when chronyd's
/// configuration has no `makestep` threshold to go by.
pub(crate) const DEFAULT_LARGE_OFFSET: Seconds = Seconds::new(1.0);

/// How deep `include`s are followed, against a loop.
const MAX_DEPTH: usize = 8;

/// The characters a comment line starts with (`CPS_NormalizeLine`,
/// `cmdparse.c`): only as a line's first, not after a directive.
const COMMENT: [char; 4] = ['!', ';', '#', '%'];

/// A `confdir` file's suffix.
const CONFDIR_SUFFIX: &str = ".conf";

/// A `makestep` directive: chronyd steps the clock when an update's
/// correction is past `threshold`, in its first `limit` updates, or in
/// any if `limit` is negative (`is_step_limit_reached`, `reference.c`,
/// 4.3 to 4.9).  Without one chronyd never steps: `make_step_limit`
/// is 0 (`conf.c`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MakeStep {
    pub(crate) threshold: Seconds,
    pub(crate) limit: i32,
}

impl MakeStep {
    /// Whether chronyd may step on its `update`th clock update since it
    /// started, counted from 1; `None` when the count is not known.
    pub(crate) fn steps_on(self, update: Option<u32>) -> bool {
        self.limit < 0 || update.is_some_and(|n| i64::from(n) <= i64::from(self.limit))
    }
}

impl fmt::Display for MakeStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "makestep {} {}", self.threshold.get(), self.limit)
    }
}

/// What chronymond reads of chronyd's configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Config {
    /// The last `makestep`, if any.
    pub(crate) make_step: Option<MakeStep>,
}

impl Config {
    /// The configuration at `path`; empty where it cannot be read.
    pub(crate) fn read(path: &Path) -> Self {
        let mut config = Self::default();
        config.read_file(path, 0);
        config
    }

    fn read_file(&mut self, path: &Path, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let Some(directive) = words.next().filter(|w| !w.starts_with(COMMENT)) else {
                continue;
            };
            let words: Vec<&str> = words.collect();
            // Directives are case-insensitive (`CNF_ParseLine`).
            match directive.to_ascii_lowercase().as_str() {
                "makestep" => self.make_step(&words),
                "include" => {
                    for file in words.iter().flat_map(|w| glob(w)) {
                        self.read_file(&file, depth + 1);
                    }
                }
                "confdir" => {
                    for file in confdir(&words) {
                        self.read_file(&file, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    /// chronyd refuses to start on any other form (`parse_makestep`).
    fn make_step(&mut self, words: &[&str]) {
        if let [threshold, limit] = words
            && let (Ok(threshold), Ok(limit)) = (threshold.parse::<f64>(), limit.parse::<i32>())
        {
            self.make_step = Some(MakeStep {
                threshold: Seconds::new(threshold),
                limit,
            });
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.make_step {
            Some(make_step) => write!(f, "{make_step}"),
            None => write!(f, "no makestep"),
        }
    }
}

/// The files a `confdir` directive includes: each `.conf` file of its
/// directories, in the order of their names, and of a name in more than
/// one only the first directory's (`search_dirs`, `conf.c`).
fn confdir(directories: &[&str]) -> Vec<PathBuf> {
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for directory in directories {
        for file in glob(&format!("{directory}/*{CONFDIR_SUFFIX}")) {
            if let Some(name) = file.file_name() {
                files
                    .entry(name.to_string_lossy().into_owned())
                    .or_insert(file);
            }
        }
    }
    files.into_values().collect()
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

    const FIRST_THREE: MakeStep = MakeStep {
        threshold: Seconds::new(1.0),
        limit: 3,
    };

    /// A fresh directory for a test's files.
    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("chronymond-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("a directory");
        directory
    }

    #[test]
    fn finds_makestep_through_confdir() {
        let directory = directory("confdir");
        let conf_d = directory.join("conf.d");
        let override_d = directory.join("override.d");
        fs::create_dir_all(&conf_d).expect("directories");
        fs::create_dir_all(&override_d).expect("directories");
        let main = directory.join("chrony.conf");
        fs::write(
            &main,
            format!(
                "pool 2.debian.pool.ntp.org iburst\n# makestep 9 9\nMakeStep 1 3\nconfdir {} {}\n",
                override_d.display(),
                conf_d.display()
            ),
        )
        .expect("written");
        assert_eq!(Config::read(&main).make_step, Some(FIRST_THREE));
        fs::write(conf_d.join("local.conf"), "makestep 0.1 -1\n").expect("written");
        let unlimited = MakeStep {
            threshold: Seconds::new(0.1),
            limit: -1,
        };
        assert_eq!(Config::read(&main).make_step, Some(unlimited));
        // A file of the same name in the directory listed first is read
        // in its place.
        fs::write(override_d.join("local.conf"), "\n").expect("written");
        assert_eq!(Config::read(&main).make_step, Some(FIRST_THREE));
        assert_eq!(
            Config::read(&directory.join("missing.conf")).make_step,
            None
        );
        fs::remove_dir_all(&directory).expect("removed");
    }

    #[test]
    fn steps_only_within_the_limit() {
        assert!(FIRST_THREE.steps_on(Some(1)));
        assert!(FIRST_THREE.steps_on(Some(3)));
        assert!(!FIRST_THREE.steps_on(Some(4)));
        assert!(!FIRST_THREE.steps_on(None));
        let never = MakeStep {
            limit: 0,
            ..FIRST_THREE
        };
        assert!(!never.steps_on(Some(1)));
        let always = MakeStep {
            limit: -1,
            ..FIRST_THREE
        };
        assert!(always.steps_on(Some(1000)));
        assert!(always.steps_on(None));
    }
}
