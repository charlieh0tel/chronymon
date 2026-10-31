//! What chronymond needs from chronyd's configuration: its `makestep`,
//! which decides when a correction is a step, which refclocks are
//! locked to which (`refclock ... lock`), which the web view pairs, and
//! its `tempcomp` sensor, which a line of fitted coefficients keeps.
//!
//! Read as chronyd reads it (`CNF_ReadFile`, `conf.c`, and the
//! `chrony.conf` documentation, 4.3 to 4.9): the main file, the files
//! its `include` patterns name, and the `.conf` files of its `confdir`
//! directories.  `sourcedir` files can hold only NTP sources, so are
//! not read.  The last `makestep` wins, as in chronyd.  Read again on
//! each restart of chronyd, which reads it only when it starts.
//!
//! A file chronyd would exit on is an error, not a file taken for
//! empty: one it cannot open, a search of an `include` pattern's
//! directories that fails, and includes past its depth (4.8 `conf.c`,
//! `CNF_ReadFile`, `parse_include`).  chronyd exits on any directive it
//! cannot parse, too; of those, only `include` and `confdir`, which
//! decide what else is read, are checked here.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::io::ErrorKind;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use glob::MatchOptions;
use glob::Pattern;

use chronymon::units::Seconds;
use chronymon_control::report::ReferenceId;
use chronymon_log::schema::RefclockLock;
use chronymon_log::schema::Tempcomp;

/// The correction recorded as a large offset when chronyd's
/// configuration has no `makestep` threshold to go by.
pub(crate) const DEFAULT_LARGE_OFFSET: Seconds = Seconds::new(1.0);

/// How many files deep, the main one first, chronyd follows `include`
/// and `confdir` before exiting (`MAX_INCLUDE_LEVEL`, `conf.c`).
const MAX_INCLUDE_LEVEL: usize = 10;

/// The most directories one `confdir` names (`MAX_CONF_DIRS`, `conf.c`).
const MAX_CONF_DIRS: usize = 10;

/// What makes a pattern a pattern, not a path, to glob(3).
const GLOB_SPECIAL: [char; 3] = ['*', '?', '['];

/// How glob(3) matches a name, without flags: case and a leading `.`
/// literal, and `*` not crossing a `/`.
const GLOB_MATCH: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: true,
};

/// Where chronyd's configuration is read from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConfigSource {
    File(PathBuf),
    /// Nowhere, written `none`: chronyd given its directives on its
    /// command line, which then reads no file (`main.c`), or reached
    /// over UDP on another host.  A file named so is `./none`.
    None,
}

/// What `--chrony-config` takes to read no configuration.
const NO_SOURCE: &str = "none";

impl FromStr for ConfigSource {
    type Err = std::convert::Infallible;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(if text == NO_SOURCE {
            Self::None
        } else {
            Self::File(PathBuf::from(text))
        })
    }
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::None => write!(f, "{NO_SOURCE}"),
        }
    }
}

/// Why chronyd's configuration could not be read as chronyd reads it:
/// each a configuration chronyd would refuse to start with.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("{}: {source}", path.display())]
    Open { path: PathBuf, source: io::Error },
    /// A directory an `include` pattern searches could not be listed:
    /// glob(3) failing with `GLOB_ERR` other than by matching nothing.
    #[error("include {pattern}: {}: {source}", directory.display())]
    Search {
        pattern: String,
        directory: PathBuf,
        source: io::Error,
    },
    #[error("{}: past chronyd's {MAX_INCLUDE_LEVEL} levels of include", path.display())]
    TooDeep { path: PathBuf },
    /// An `include` or `confdir` with the wrong number of arguments.
    #[error("{}: line {line}: {directive} malformed", path.display())]
    Malformed {
        path: PathBuf,
        line: usize,
        directive: &'static str,
    },
}

/// The characters a comment line starts with (`CPS_NormalizeLine`,
/// `cmdparse.c`): only as a line's first, not after a directive.
const COMMENT: [char; 4] = ['!', ';', '#', '%'];

/// The longest reference ID, in characters (`CPS_ParseRefid`,
/// `cmdparse.c`).
const REFERENCE_ID_LENGTH: usize = 4;

/// The words of a `tempcomp` directive after its name: its points-file
/// form's, and its coefficients' (`parse_tempcomp`, `conf.c`).
const TEMPCOMP_POINTS_WORDS: usize = 3;
const TEMPCOMP_COEFFICIENTS_WORDS: usize = 6;

/// How much of a refclock's driver name its default reference ID
/// takes (`RCL_AddRefclock`, `refclock.c`).
const DRIVER_PREFIX: usize = 3;

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
    /// Each `refclock`, in the order chronyd adds them.
    pub(crate) refclocks: Vec<Refclock>,
    /// The last `tempcomp`'s sensor and interval, if any.
    pub(crate) tempcomp: Option<Tempcomp>,
}

/// A `refclock` directive: the refclock's reference ID and the one its
/// `lock` option names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Refclock {
    pub(crate) reference_id: ReferenceId,
    pub(crate) lock: Option<ReferenceId>,
}

impl Config {
    /// The configuration at `path`, or why chronyd would not start with
    /// it.
    pub(crate) fn read(path: &Path) -> Result<Self, Error> {
        let mut config = Self::default();
        config.read_file(path, 1)?;
        Ok(config)
    }

    /// Read `path`, `level` files deep.
    fn read_file(&mut self, path: &Path, level: usize) -> Result<(), Error> {
        if level > MAX_INCLUDE_LEVEL {
            return Err(Error::TooDeep {
                path: path.to_path_buf(),
            });
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            // chronyd opens a directory and reads no line from it.
            Err(e) if e.kind() == ErrorKind::IsADirectory => return Ok(()),
            Err(source) => {
                return Err(Error::Open {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let malformed = |line, directive| Error::Malformed {
            path: path.to_path_buf(),
            line,
            directive,
        };
        for (index, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            let mut words = line.split_whitespace();
            let Some(directive) = words.next().filter(|w| !w.starts_with(COMMENT)) else {
                continue;
            };
            let words: Vec<&str> = words.collect();
            // Directives are case-insensitive (`CNF_ParseLine`).
            match directive.to_ascii_lowercase().as_str() {
                "makestep" => self.make_step(&words),
                "refclock" => self.refclock(&words),
                "tempcomp" => self.tempcomp(&words),
                "include" => {
                    let [pattern] = words[..] else {
                        return Err(malformed(index + 1, "include"));
                    };
                    for file in include(pattern)? {
                        self.read_file(&file, level + 1)?;
                    }
                }
                "confdir" => {
                    if words.is_empty() || words.len() > MAX_CONF_DIRS {
                        return Err(malformed(index + 1, "confdir"));
                    }
                    for file in confdir(&words) {
                        self.read_file(&file, level + 1)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
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

    /// A `tempcomp SENSOR INTERVAL T0 K0 K1 K2` or `tempcomp SENSOR
    /// INTERVAL POINTS` directive's sensor and interval; the last wins
    /// (`parse_tempcomp`, `conf.c`).  chronyd refuses to start on any
    /// other number of words.
    fn tempcomp(&mut self, words: &[&str]) {
        if let [sensor, interval, ..] = words
            && matches!(
                words.len(),
                TEMPCOMP_POINTS_WORDS | TEMPCOMP_COEFFICIENTS_WORDS
            )
        {
            self.tempcomp = Some(Tempcomp {
                sensor: (*sensor).to_owned(),
                interval: (*interval).to_owned(),
            });
        }
    }

    /// A `refclock DRIVER PARAMETER [OPTION]...` directive: its `refid`,
    /// or the default made of its driver's name and its place among the
    /// refclocks, and its `lock` (`parse_refclock`, `conf.c`;
    /// `RCL_AddRefclock`, `refclock.c`).  Options are case-insensitive,
    /// and the last of each wins.  Every other option's value is a
    /// number, so is never taken for `refid` or `lock`.  One chronyd
    /// would refuse to start on is left out.
    fn refclock(&mut self, words: &[&str]) {
        let [driver, _parameter, options @ ..] = words else {
            return;
        };
        let (mut reference_id, mut lock, mut local) = (None, None, false);
        let mut options = options.iter();
        while let Some(option) = options.next() {
            match option.to_ascii_lowercase().as_str() {
                "refid" => match options.next().and_then(|w| parse_reference_id(w)) {
                    Some(id) => reference_id = Some(id),
                    None => return,
                },
                "lock" => match options.next().and_then(|w| parse_reference_id(w)) {
                    Some(id) => lock = Some(id),
                    None => return,
                },
                "local" => local = true,
                _ => {}
            }
        }
        let reference_id =
            reference_id.unwrap_or_else(|| default_reference_id(driver, self.refclocks.len()));
        self.refclocks.push(Refclock {
            reference_id,
            // `local` locks a refclock to itself, in place of `lock`.
            lock: lock.filter(|_| !local),
        });
    }

    /// Each refclock locked to another, as chronyd resolves `lock`
    /// (`RCL_StartRefclocks`, `refclock.c`): to the first refclock of
    /// that ID, and none if that is itself or there is none.
    pub(crate) fn locks(&self) -> Vec<RefclockLock> {
        self.refclocks
            .iter()
            .enumerate()
            .filter_map(|(n, refclock)| {
                let locked_to = refclock.lock?;
                let at = self
                    .refclocks
                    .iter()
                    .position(|other| other.reference_id == locked_to)?;
                (at != n).then_some(RefclockLock {
                    refclock: refclock.reference_id,
                    locked_to,
                })
            })
            .collect()
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.make_step {
            Some(make_step) => write!(f, "{make_step}")?,
            None => write!(f, "no makestep")?,
        }
        for lock in self.locks() {
            write!(f, ", {lock}")?;
        }
        if let Some(tempcomp) = &self.tempcomp {
            write!(
                f,
                ", tempcomp {} every {} s",
                tempcomp.sensor, tempcomp.interval
            )?;
        }
        Ok(())
    }
}

/// A reference ID as chrony's configuration gives one: up to four
/// characters, packed from the most significant byte (`CPS_ParseRefid`,
/// `cmdparse.c`).
fn parse_reference_id(word: &str) -> Option<ReferenceId> {
    let bytes = word.as_bytes();
    if bytes.is_empty() || bytes.len() > REFERENCE_ID_LENGTH {
        return None;
    }
    let mut id = [0; REFERENCE_ID_LENGTH];
    id[..bytes.len()].copy_from_slice(bytes);
    Some(ReferenceId(u32::from_be_bytes(id)))
}

/// The reference ID chronyd gives the `index`th refclock, from 0, of
/// driver `driver` when its directive has no `refid`: the driver's
/// name to three characters, right-aligned (`%3.3s`), its last replaced
/// by the index's units digit, and the one before by its tens digit
/// from 10 on (`RCL_AddRefclock`, `refclock.c`, 4.3 to 4.9): `SHM0`,
/// `PPS1`, `SH10`.
fn default_reference_id(driver: &str, index: usize) -> ReferenceId {
    let name = &driver.as_bytes()[..driver.len().min(DRIVER_PREFIX)];
    let mut id = [b' '; REFERENCE_ID_LENGTH];
    id[DRIVER_PREFIX - name.len()..DRIVER_PREFIX].copy_from_slice(name);
    let digit = |n: usize| b'0' + (n % 10) as u8;
    id[DRIVER_PREFIX] = digit(index);
    if index >= 10 {
        id[DRIVER_PREFIX - 1] = digit(index / 10);
    }
    ReferenceId(u32::from_be_bytes(id))
}

/// The files a `confdir` directive includes: each `.conf` file of its
/// directories, in the order of their names, and of a name in more than
/// one only the first directory's (`search_dirs`, `conf.c`).  A
/// directory that cannot be listed has none, as chronyd ignores glob(3)
/// failing there.
fn confdir(directories: &[&str]) -> Vec<PathBuf> {
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for directory in directories {
        let pattern = format!("{directory}/*{CONFDIR_SUFFIX}");
        for file in search(&pattern).unwrap_or_default() {
            if let Some(name) = file.file_name() {
                files
                    .entry(name.to_string_lossy().into_owned())
                    .or_insert(file);
            }
        }
    }
    files.into_values().collect()
}

/// The files an `include` pattern names, as glob(3) finds them with
/// `GLOB_NOMAGIC | GLOB_ERR` (`parse_include`, `conf.c`): a path with
/// no pattern in it as it is, there or not; else the paths matching,
/// none if none do, and an error where a directory searched cannot be
/// listed.
fn include(pattern: &str) -> Result<Vec<PathBuf>, Error> {
    if !pattern.contains(GLOB_SPECIAL) {
        return Ok(vec![PathBuf::from(pattern)]);
    }
    search(pattern).map_err(|(directory, source)| Error::Search {
        pattern: pattern.to_owned(),
        directory,
        source,
    })
}

/// The paths matching `pattern`, sorted as glob(3) sorts them, or the
/// first directory that could not be listed and why.  The components
/// before the first with a pattern in them are taken as they are, as
/// glob(3) takes them: listing a directory missing there fails.  A
/// path that is not a directory has nothing in it to match.
fn search(pattern: &str) -> Result<Vec<PathBuf>, (PathBuf, io::Error)> {
    let mut components = Path::new(pattern).components().peekable();
    let mut base = PathBuf::new();
    while let Some(component) =
        components.next_if(|c| !c.as_os_str().to_string_lossy().contains(GLOB_SPECIAL))
    {
        base.push(component);
    }
    let mut paths = vec![base];
    for component in components {
        let Component::Normal(name) = component else {
            paths = paths.into_iter().map(|path| path.join(component)).collect();
            continue;
        };
        let name = name.to_string_lossy();
        if !name.contains(GLOB_SPECIAL) {
            paths = paths
                .into_iter()
                .map(|path| path.join(&*name))
                .filter(|path| fs::symlink_metadata(path).is_ok())
                .collect();
            continue;
        }
        // An unclosed `[` is no pattern to glob(3): itself.
        let matches = |file: &str| match Pattern::new(&name) {
            Ok(pattern) => pattern.matches_with(file, GLOB_MATCH),
            Err(_) => file == name,
        };
        let mut found = Vec::new();
        for path in paths {
            let listed = if path.as_os_str().is_empty() {
                Path::new(".")
            } else {
                &path
            };
            let entries = match fs::read_dir(listed) {
                Ok(entries) => entries,
                Err(e) if e.kind() == ErrorKind::NotADirectory => continue,
                Err(e) => return Err((listed.to_path_buf(), e)),
            };
            for entry in entries {
                let entry = entry.map_err(|e| (listed.to_path_buf(), e))?;
                let file = entry.file_name();
                if matches(&file.to_string_lossy()) {
                    found.push(path.join(file));
                }
            }
        }
        paths = found;
    }
    // By their bytes, as glob(3) sorts them in chronyd's C locale.
    paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    Ok(paths)
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
        assert_eq!(read(&main).make_step, Some(FIRST_THREE));
        fs::write(conf_d.join("local.conf"), "makestep 0.1 -1\n").expect("written");
        let unlimited = MakeStep {
            threshold: Seconds::new(0.1),
            limit: -1,
        };
        assert_eq!(read(&main).make_step, Some(unlimited));
        // A file of the same name in the directory listed first is read
        // in its place.
        fs::write(override_d.join("local.conf"), "\n").expect("written");
        assert_eq!(read(&main).make_step, Some(FIRST_THREE));
        // Hidden from glob(3), as from chronyd.
        fs::write(conf_d.join(".hidden.conf"), "makestep 9 9\n").expect("written");
        assert_eq!(read(&main).make_step, Some(FIRST_THREE));
        fs::remove_dir_all(&directory).expect("removed");
    }

    #[test]
    fn keeps_the_last_tempcomps_sensor_in_either_form() {
        let directory = directory("tempcomp");
        let main = directory.join("chrony.conf");
        let hwmon = "/sys/devices/platform/coretemp.0/hwmon/hwmon2/temp1_input";
        let points = "/sys/class/hwmon/hwmon0/temp1_input";
        fs::write(
            &main,
            format!(
                "tempcomp {hwmon} 30 0 0 0 0\nTempComp {points} 15 /etc/chrony/tempcomp.points\n"
            ),
        )
        .expect("written");
        let sensor = |sensor: &str, interval: &str| {
            Some(Tempcomp {
                sensor: sensor.to_owned(),
                interval: interval.to_owned(),
            })
        };
        assert_eq!(read(&main).tempcomp, sensor(points, "15"));
        // chronyd would refuse a line of any other length.
        fs::write(&main, format!("tempcomp {hwmon} 30 0 0\n")).expect("written");
        assert_eq!(read(&main).tempcomp, None);
        fs::write(&main, format!("tempcomp {hwmon} 30 26000 0 0.000183 0\n")).expect("written");
        assert_eq!(read(&main).tempcomp, sensor(hwmon, "30"));
        fs::remove_dir_all(&directory).expect("removed");
    }

    /// The configuration at `path`, which must be readable.
    fn read(path: &Path) -> Config {
        Config::read(path).expect("readable")
    }

    /// The configuration in `text`, written to `directory`'s main file.
    fn read_text(directory: &Path, text: &str) -> Result<Config, Error> {
        let main = directory.join("chrony.conf");
        fs::write(&main, text).expect("written");
        Config::read(&main)
    }

    #[test]
    fn refuses_what_chronyd_would_not_start_with() {
        let directory = directory("refuses");
        let d = directory.display();
        assert!(matches!(
            Config::read(&directory.join("missing.conf")),
            Err(Error::Open { .. })
        ));
        // A path with no pattern in it is read, there or not.
        assert!(matches!(
            read_text(&directory, &format!("include {d}/missing.conf\n")),
            Err(Error::Open { .. })
        ));
        // A pattern searching a directory that is not there.
        assert!(matches!(
            read_text(&directory, &format!("include {d}/missing/*.conf\n")),
            Err(Error::Search { .. })
        ));
        // A file a `confdir` matched that cannot be opened.
        let conf_d = directory.join("conf.d");
        fs::create_dir_all(&conf_d).expect("a directory");
        std::os::unix::fs::symlink(directory.join("gone"), conf_d.join("dangling.conf"))
            .expect("a link");
        assert!(matches!(
            read_text(&directory, &format!("confdir {}\n", conf_d.display())),
            Err(Error::Open { .. })
        ));
        // Including itself, past chronyd's depth.
        assert!(matches!(
            read_text(&directory, &format!("include {d}/chrony.conf\n")),
            Err(Error::TooDeep { .. })
        ));
        assert!(matches!(
            read_text(&directory, "include a.conf b.conf\n"),
            Err(Error::Malformed { line: 1, .. })
        ));
        assert!(matches!(
            read_text(&directory, "confdir\n"),
            Err(Error::Malformed { .. })
        ));
        fs::remove_dir_all(&directory).expect("removed");
    }

    #[test]
    fn reads_what_chronyd_reads() {
        let directory = directory("reads");
        let d = directory.display();
        // A pattern matching nothing, a missing `confdir` directory, and
        // a directory read as a file are nothing.
        assert_eq!(
            read_text(
                &directory,
                &format!("include {d}/*.none\nconfdir {d}/missing\ninclude {d}\nmakestep 1 3\n")
            )
            .expect("readable")
            .make_step,
            Some(FIRST_THREE)
        );
        // glob(3)'s `?` and `[...]`, and a pattern in a directory's name,
        // in the order of the paths' bytes.
        for name in ["a1", "a2", "b1"] {
            let sub = directory.join(name);
            fs::create_dir_all(&sub).expect("a directory");
            fs::write(
                sub.join("step.conf"),
                format!("makestep {} 3\n", &name[1..]),
            )
            .expect("written");
        }
        let threshold = |pattern: &str| {
            read_text(&directory, &format!("include {d}/{pattern}\n"))
                .expect("readable")
                .make_step
                .map(|step| step.threshold.get())
        };
        assert_eq!(threshold("a?/step.conf"), Some(2.0));
        assert_eq!(threshold("[ab]1/step.conf"), Some(1.0));
        assert_eq!(threshold("*/step.conf"), Some(1.0));
        assert_eq!(threshold("a*/missing.conf"), None);
        fs::remove_dir_all(&directory).expect("removed");
    }

    /// A reference ID from its text.
    fn id(text: &str) -> ReferenceId {
        parse_reference_id(text).expect("a reference ID")
    }

    #[test]
    fn pairs_refclocks_by_lock_through_include() {
        let directory = directory("refclock");
        let main = directory.join("chrony.conf");
        let included = directory.join("refclocks.conf");
        fs::write(
            &main,
            format!(
                "refclock SHM 0 refid NMEA poll 6 offset 43e-3 delay 10e-3 noselect\n\
                 include {}\n\
                 refclock SOCK /run/chrony.sock\n",
                included.display()
            ),
        )
        .expect("written");
        fs::write(
            &included,
            "REFCLOCK PHC /dev/ptp2:extpps:pin=0 refid PPS width 500e-3 precision 1e-9 poll 2 LOCK NMEA\n\
             ; refclock SHM 1 lock NMEA\n\
             refclock PPS /dev/pps1 lock GPS\n\
             refclock PPS /dev/pps2 lock PPS3\n\
             refclock PPS /dev/pps3 refid LOCL lock NMEA local\n",
        )
        .expect("written");
        let config = read(&main);
        let ids: Vec<ReferenceId> = config.refclocks.iter().map(|r| r.reference_id).collect();
        // Without `refid`, the driver's name and the refclock's place.
        assert_eq!(
            ids,
            [
                id("NMEA"),
                id("PPS"),
                id("PPS2"),
                id("PPS3"),
                id("LOCL"),
                id("SOC5")
            ]
        );
        // Not to an ID no refclock has, nor to itself, nor with `local`.
        assert_eq!(
            config.locks(),
            [RefclockLock {
                refclock: id("PPS"),
                locked_to: id("NMEA"),
            }]
        );
        assert_eq!(config.to_string(), "no makestep, PPS locked to NMEA");
        fs::remove_dir_all(&directory).expect("removed");
    }

    #[test]
    fn refuses_a_refclock_chronyd_would() {
        let mut config = Config::default();
        config.refclock(&["SHM"]);
        config.refclock(&["SHM", "0", "refid", "TOOLONG"]);
        config.refclock(&["SHM", "0", "lock"]);
        assert!(config.refclocks.is_empty());
    }

    #[test]
    fn makes_default_reference_ids_as_chronyd() {
        assert_eq!(default_reference_id("SHM", 0), id("SHM0"));
        assert_eq!(default_reference_id("SOCK", 9), id("SOC9"));
        assert_eq!(default_reference_id("SHM", 10), id("SH10"));
        assert_eq!(default_reference_id("SHM", 123), id("SH23"));
        assert_eq!(default_reference_id("RT", 1), id(" RT1"));
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
