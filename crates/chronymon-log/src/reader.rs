//! Reading the database, for the web view.
//!
//! Opened read-only, and never created: a missing database is the
//! daemon not having run yet, which the page says, rather than an empty
//! file this side made.
//!
//! Read-only all the way down, the WAL's files included.  A reader that
//! may write the directory creates `-wal` and `-shm` of its own when the
//! daemon is not running, owned by the web view's user, and the daemon,
//! another user, then cannot open its database at all: seen, in a
//! container, as "unable to open database file".  So the web view has
//! no write access to the directory, and while the daemon is not running
//! -- no `-wal` beside the database, since SQLite removes it on the last
//! clean close -- the file is opened `immutable`, which needs no WAL
//! index and is safe because nothing is writing.  While it runs, its own
//! `-shm` is read as it is (SQLite's read-only WAL, from 3.22).
//!
//! Bucketed in SQL rather than by the caller, so a year costs what a
//! day does to send and draw.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use rusqlite::types::Value;

use crate::error::Error;
use crate::error::Result;

/// Fewest buckets worth drawing, and the most a chart can show.
pub const MIN_POINTS: usize = 16;
/// See [`MIN_POINTS`].
pub const MAX_POINTS: usize = 5000;

/// How many empty buckets in a row, at least, make a gap worth breaking
/// a line at: a hole in a once-a-second series is a few buckets, and
/// smaller ones are buckets that simply fell between readings.
const GAP_BUCKETS: i64 = 4;

/// How many times the series' own typical spacing a hole must be to
/// break the line.  Sources are sampled every 64 s or more, which at an
/// hour's zoom leaves dozens of empty buckets between every two samples;
/// those are the cadence, not a gap.
const GAP_SPACINGS: i64 = 3;

/// Nanoseconds in a second.
const NANOSECONDS: f64 = 1e9;

/// A table the web view charts, and the columns of it that it may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// The local clock, `tracking`.
    Tracking,
    /// Sources' samples, `sources`.
    SourceSample,
    /// Sources' regressions, `sourcestats`.
    SourceStats,
    /// NTP sources' packets, `ntpdata`.
    NtpSample,
    /// Sources' selection, `selectdata`.
    Selection,
    /// The local clock, from `tracking.log`: to the second and four
    /// significant digits, but from before the daemon ran.
    LogTracking,
    /// NTP sources' packets, from `measurements.log`.
    LogMeasurement,
    /// Sources' regressions, from `statistics.log`, frequencies as
    /// fractions rather than ppm.
    LogStatistics,
    /// Refclocks' samples, from `refclocks.log`, the only record of them.
    RefclockSample,
    /// Temperature compensation, from `tempcomp.log`, likewise.
    TempcompSample,
}

/// Which source's rows a per-source table is asked for: by the
/// daemon's ID, or by the address or reference a log names it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A row of `source`.
    Id(i64),
    /// An address, or a refclock's reference such as `PPS`.
    Name(String),
}

impl Key {
    fn value(key: Option<&Self>) -> Value {
        match key {
            None => Value::Null,
            Some(Self::Id(id)) => Value::Integer(*id),
            Some(Self::Name(name)) => Value::Text(name.clone()),
        }
    }
}

impl Table {
    /// Every chartable table.
    pub const ALL: [Self; 10] = [
        Self::Tracking,
        Self::SourceSample,
        Self::SourceStats,
        Self::NtpSample,
        Self::Selection,
        Self::LogTracking,
        Self::LogMeasurement,
        Self::LogStatistics,
        Self::RefclockSample,
        Self::TempcompSample,
    ];

    /// Its name, as the schema and the web view's requests have it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tracking => "tracking",
            Self::SourceSample => "source_sample",
            Self::SourceStats => "source_stats",
            Self::NtpSample => "ntp_sample",
            Self::Selection => "selection",
            Self::LogTracking => "log_tracking",
            Self::LogMeasurement => "log_measurement",
            Self::LogStatistics => "log_statistics",
            Self::RefclockSample => "refclock_sample",
            Self::TempcompSample => "tempcomp_sample",
        }
    }

    /// The table named `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|table| table.name() == name)
    }

    /// The column each row is placed in time by: when the update or
    /// the sample happened, where the table knows, else when it was
    /// read.
    const fn time(self) -> &'static str {
        match self {
            Self::Tracking => "reference_time",
            Self::SourceSample => "sample_time",
            _ => "at",
        }
    }

    /// The column naming whose a row is, for a per-source table.
    pub const fn key(self) -> Option<&'static str> {
        match self {
            Self::Tracking | Self::LogTracking | Self::TempcompSample => None,
            Self::SourceSample | Self::SourceStats | Self::NtpSample | Self::Selection => {
                Some("source_id")
            }
            Self::LogMeasurement | Self::LogStatistics => Some("source"),
            Self::RefclockSample => Some("reference"),
        }
    }

    /// Whether its rows are a source's.
    pub const fn per_source(self) -> bool {
        self.key().is_some()
    }

    /// Whether its rows are keyed by the daemon's source IDs, rather
    /// than the names a log gives.
    pub const fn by_id(self) -> bool {
        matches!(
            self,
            Self::SourceSample | Self::SourceStats | Self::NtpSample | Self::Selection
        )
    }

    /// The numeric columns that may be charted.  Checked against this
    /// before reaching the SQL, which is interpolated, so a request
    /// cannot name an arbitrary expression.
    pub const fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Tracking => &[
                "current_correction",
                "last_offset",
                "rms_offset",
                "frequency",
                "residual_frequency",
                "skew",
                "root_delay",
                "root_dispersion",
                "last_update_interval",
                "stratum",
            ],
            Self::SourceSample => &[
                "latest_measurement",
                "original_latest_measurement",
                "latest_measurement_error",
                "poll",
                "stratum",
                "reachability",
            ],
            Self::SourceStats => &[
                "estimated_offset",
                "estimated_offset_error",
                "standard_deviation",
                "residual_frequency",
                "skew",
                "samples",
                "runs",
                "span",
            ],
            Self::NtpSample => &[
                "offset",
                "peer_delay",
                "peer_dispersion",
                "response_time",
                "root_delay",
                "root_dispersion",
                "poll",
                "stratum",
            ],
            Self::Selection => &["score", "low_limit", "high_limit"],
            Self::LogTracking => &[
                "frequency",
                "skew",
                "offset",
                "offset_sd",
                "remaining_correction",
                "root_delay",
                "root_dispersion",
                "max_error",
                "stratum",
                "combined_sources",
            ],
            Self::LogMeasurement => &[
                "offset",
                "peer_delay",
                "peer_dispersion",
                "root_delay",
                "root_dispersion",
                "score",
                "local_poll",
                "remote_poll",
                "stratum",
            ],
            Self::LogStatistics => &[
                "estimated_offset",
                "offset_sd",
                "standard_deviation",
                "residual_frequency",
                "skew",
                "stress",
                "samples",
                "runs",
                "asymmetry",
            ],
            Self::RefclockSample => &[
                "cooked_offset",
                "raw_offset",
                "dispersion",
                "pulse",
                "driver_polls",
            ],
            Self::TempcompSample => &["temperature", "compensation"],
        }
    }

    /// `name` as one of [`Self::columns`].
    pub fn column(self, name: &str) -> Option<Column> {
        self.columns()
            .iter()
            .copied()
            .find(|c| *c == name)
            .map(|name| Column { table: self, name })
    }
}

/// A chartable column of a table, made only by [`Table::column`]: the
/// only text interpolated into SQL here, so it is always one of
/// [`Table::columns`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    table: Table,
    name: &'static str,
}

impl Column {
    /// Its name.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Its table.
    pub const fn table(self) -> Table {
        self.table
    }
}

/// Bucketed readings of several columns of one table over one range.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// The mean time of each bucket's rows, unix seconds, shared by
    /// every plot: what makes stacked charts line up.
    pub at: Vec<f64>,
    /// One per column asked for, in order.
    pub plots: Vec<Plot>,
}

/// One column's buckets, against [`Series::at`].  `min` and `max` are
/// kept because a mean alone hides an excursion inside a bucket.  All
/// three are `None` in the bucket that marks a gap.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    /// The column's table.
    pub table: &'static str,
    /// The column.
    pub column: &'static str,
    /// Each bucket's mean.
    pub mean: Vec<Option<f64>>,
    /// Each bucket's lowest.
    pub min: Vec<Option<f64>>,
    /// Each bucket's highest.
    pub max: Vec<Option<f64>>,
}

/// One column of every source, bucketed on one grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    /// The bucket centers, unix seconds, shared by every line.
    pub at: Vec<f64>,
    /// One per source with rows in the range.
    pub lines: Vec<Line>,
}

/// One source's buckets, against [`Overlay::at`]; `None` where it has
/// none.  A source's samples fall in few of the grid's buckets when the
/// others are sampled more often, so `None` is not a gap: [`Line::gaps`]
/// says where the line breaks.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The source's ID.
    pub source_id: i64,
    /// Each bucket's mean.
    pub mean: Vec<Option<f64>>,
    /// Indexes into [`Overlay::at`] where the line breaks.
    pub gaps: Vec<usize>,
}

/// A time range and how finely to bucket it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Unix seconds, inclusive.
    pub from: f64,
    /// Unix seconds, inclusive.
    pub to: f64,
    /// At most this many buckets.
    pub points: usize,
}

impl Window {
    fn stored(self) -> (i64, i64, i64) {
        // Rounded to whole nanoseconds, well inside an i64 until 2262.
        let ns = |s: f64| (s * NANOSECONDS).round() as i64;
        let points = i64::try_from(self.points.clamp(MIN_POINTS, MAX_POINTS)).unwrap_or(1);
        (ns(self.from), ns(self.to).max(ns(self.from)), points)
    }
}

/// A row's numeric columns, by name.
pub type Values = Vec<(&'static str, f64)>;

/// The `status` row: the daemon's last poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// When the daemon last polled, unix nanoseconds.
    pub polled_at: i64,
    /// `up`, `down` or `unsupported`.
    pub state: String,
    /// Why, when not up.
    pub detail: Option<String>,
    /// chronyd's PID, when running.
    pub chronyd_pid: Option<i64>,
}

/// The latest `tracking` row, by name, as the strip shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracking {
    /// When the clock was last updated, unix nanoseconds.
    pub reference_time: i64,
    /// The reference ID.
    pub reference_id: i64,
    /// The reference's address.
    pub reference_address: String,
    /// The local stratum.
    pub stratum: i64,
    /// The leap status.
    pub leap: String,
    /// The other columns, by name, in [`Table::Tracking`]'s order.
    pub values: Values,
}

/// One source, with the latest of each report about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// Its ID.
    pub id: i64,
    /// Its address: IP, or a refclock's reference ID in hex.
    pub address: String,
    /// `client`, `peer` or `refclock`.
    pub mode: String,
    /// Its configured name.
    pub name: Option<String>,
    /// Unix nanoseconds.
    pub first_seen: i64,
    /// Unix nanoseconds.
    pub last_seen: i64,
    /// Its latest sample: state, and the columns by name.
    pub sample: Option<(String, Values)>,
    /// Its latest selection: state character, and the columns by name.
    pub selection: Option<(String, Values)>,
    /// Its latest regression, by name.
    pub stats: Option<Values>,
    /// Its latest NTP packet, by name.
    pub ntp: Option<Values>,
}

/// A source a log names, and what the table holds of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    /// Its address, or a refclock's reference.
    pub name: String,
    /// Its rows.
    pub rows: i64,
    /// Its first and last rows' times, unix seconds.
    pub first: f64,
    /// See `first`.
    pub last: f64,
}

/// One event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Unix nanoseconds.
    pub at: i64,
    /// Its kind's name (`EventKind::name`).
    pub kind: String,
    /// The source it is about, if one.
    pub source_id: Option<i64>,
    /// What else is known.
    pub detail: Option<String>,
}

/// The database, open for reading.
#[derive(Debug)]
pub struct Reader {
    connection: Connection,
}

impl Reader {
    /// Open the database at `path`, which must exist.
    pub fn open(path: &Path) -> Result<Self> {
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let writing = Path::new(&wal).exists();
        let uri = format!(
            "file:{}?mode=ro{}",
            uri_path(path),
            if writing { "" } else { "&immutable=1" }
        );
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|source| Error::Open {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self { connection })
    }

    /// The daemon's last poll, if it has polled.
    pub fn status(&self) -> Result<Option<Status>> {
        Ok(self
            .connection
            .query_row(
                "SELECT polled_at, state, detail, chronyd_pid FROM status WHERE id = 1",
                [],
                |row| {
                    Ok(Status {
                        polled_at: row.get(0)?,
                        state: row.get(1)?,
                        detail: row.get(2)?,
                        chronyd_pid: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// The newest clock update.
    pub fn latest_tracking(&self) -> Result<Option<Tracking>> {
        let columns = Table::Tracking.columns();
        let sql = format!(
            "SELECT reference_time, reference_id, reference_address, stratum, leap, {}
             FROM tracking ORDER BY reference_time DESC LIMIT 1",
            columns.join(", ")
        );
        Ok(self
            .connection
            .query_row(&sql, [], |row| {
                let values = columns
                    .iter()
                    .enumerate()
                    .map(|(n, c)| Ok((*c, row.get::<_, f64>(5 + n)?)))
                    .collect::<rusqlite::Result<_>>()?;
                Ok(Tracking {
                    reference_time: row.get(0)?,
                    reference_id: row.get(1)?,
                    reference_address: row.get(2)?,
                    stratum: row.get(3)?,
                    leap: row.get(4)?,
                    values,
                })
            })
            .optional()?)
    }

    /// The first and last times `table` holds, unix seconds.
    pub fn extent(&self, table: Table) -> Result<Option<(f64, f64)>> {
        let time = table.time();
        let sql = format!("SELECT MIN({time}), MAX({time}) FROM {}", table.name());
        let (first, last): (Option<i64>, Option<i64>) =
            self.connection
                .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(first
            .zip(last)
            .map(|(first, last)| (seconds(first), seconds(last))))
    }

    /// Every source a per-source log table names, by name, with its
    /// rows: the refclocks of `refclock_sample`, say.
    pub fn keys(&self, table: Table) -> Result<Vec<Named>> {
        let Some(key) = table.key().filter(|_| !table.by_id()) else {
            return Ok(Vec::new());
        };
        let time = table.time();
        let sql = format!(
            "SELECT {key}, COUNT(*), MIN({time}), MAX({time}) FROM {} GROUP BY {key} ORDER BY {key}",
            table.name()
        );
        let mut statement = self.connection.prepare(&sql)?;
        Ok(statement
            .query_map([], |row| {
                Ok(Named {
                    name: row.get(0)?,
                    rows: row.get(1)?,
                    first: seconds(row.get(2)?),
                    last: seconds(row.get(3)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every source, the most recently seen first, each with the
    /// latest of each report.
    pub fn sources(&self) -> Result<Vec<Source>> {
        let mut statement = self.connection.prepare(
            "SELECT id, address, mode, name, first_seen, last_seen FROM source
             ORDER BY last_seen DESC, id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(Source {
                    id: row.get(0)?,
                    address: row.get(1)?,
                    mode: row.get(2)?,
                    name: row.get(3)?,
                    first_seen: row.get(4)?,
                    last_seen: row.get(5)?,
                    sample: None,
                    selection: None,
                    stats: None,
                    ntp: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|mut source| {
                source.sample = self.latest_labeled(Table::SourceSample, "state", source.id)?;
                source.selection = self.latest_labeled(Table::Selection, "state", source.id)?;
                source.stats = self.latest(Table::SourceStats, source.id)?;
                source.ntp = self.latest(Table::NtpSample, source.id)?;
                Ok(source)
            })
            .collect()
    }

    /// The newest row of `table` for `source_id`, its columns by name.
    fn latest(&self, table: Table, source_id: i64) -> Result<Option<Values>> {
        Ok(self
            .latest_labeled(table, "''", source_id)?
            .map(|(_, values)| values))
    }

    /// As [`Self::latest`], with the text column `label` as well.
    fn latest_labeled(
        &self,
        table: Table,
        label: &str,
        source_id: i64,
    ) -> Result<Option<(String, Values)>> {
        let columns = table.columns();
        let sql = format!(
            "SELECT {label}, {} FROM {} WHERE source_id = ?1 ORDER BY {} DESC LIMIT 1",
            columns.join(", "),
            table.name(),
            table.time()
        );
        Ok(self
            .connection
            .query_row(&sql, params![source_id], |row| {
                let values = columns
                    .iter()
                    .enumerate()
                    .map(|(n, c)| Ok((*c, row.get::<_, f64>(1 + n)?)))
                    .collect::<rusqlite::Result<_>>()?;
                Ok((row.get(0)?, values))
            })
            .optional()?)
    }

    /// Bucketed series of `columns`, all of `table`, over `window`, for
    /// one source where the table is per source and `source` says.
    ///
    /// One query for every column, so their buckets are one grid: two
    /// queries a moment apart would bucket from different ends.
    pub fn series(
        &self,
        table: Table,
        source: Option<&Key>,
        columns: &[Column],
        window: Window,
    ) -> Result<Series> {
        let columns: Vec<Column> = columns
            .iter()
            .copied()
            .filter(|c| c.table == table)
            .collect();
        let buckets = self.buckets(table, source, &columns, window)?;
        Ok(assemble(&columns, buckets, window))
    }

    /// `columns` of `table`, and in the buckets where it has no rows
    /// `fill_columns` of `fill`, on one grid over `window`: the
    /// protocol's tracking, say, and where the daemon was not running,
    /// tracking.log's.  The plots are `columns`' and then
    /// `fill_columns`', each `None` where the other table's are drawn.
    pub fn filled(
        &self,
        table: Table,
        columns: &[Column],
        fill: Table,
        fill_columns: &[Column],
        window: Window,
    ) -> Result<Series> {
        let columns: Vec<Column> = columns
            .iter()
            .copied()
            .filter(|c| c.table == table)
            .collect();
        let fill_columns: Vec<Column> = fill_columns
            .iter()
            .copied()
            .filter(|c| c.table == fill)
            .collect();
        let primary = self.buckets(table, None, &columns, window)?;
        let taken: HashSet<i64> = primary.iter().map(|b| b.index).collect();
        let mut merged: Vec<Bucket> = primary
            .into_iter()
            .map(|mut b| {
                b.values.extend(vec![[None; 3]; fill_columns.len()]);
                b
            })
            .collect();
        merged.extend(
            self.buckets(fill, None, &fill_columns, window)?
                .into_iter()
                .filter(|b| !taken.contains(&b.index))
                .map(|b| Bucket {
                    values: vec![[None; 3]; columns.len()]
                        .into_iter()
                        .chain(b.values)
                        .collect(),
                    ..b
                }),
        );
        merged.sort_by_key(|b| b.index);
        let all: Vec<Column> = columns.into_iter().chain(fill_columns).collect();
        Ok(assemble(&all, merged, window))
    }

    /// The buckets of `columns` of `table` over `window`, in order.
    fn buckets(
        &self,
        table: Table,
        source: Option<&Key>,
        columns: &[Column],
        window: Window,
    ) -> Result<Vec<Bucket>> {
        let (from, to, points) = window.stored();
        let time = table.time();
        let aggregates: String = columns
            .iter()
            .map(|c| format!(", AVG({0}), MIN({0}), MAX({0})", c.name))
            .collect();
        let filter = source_filter(table, 4);
        let sql = format!(
            // Divided by the span plus one, so a row exactly at `to`
            // falls in the last bucket rather than one past it.
            "SELECT CAST(({time} - ?1) * ?3 / (?2 - ?1 + 1) AS INTEGER) AS bucket,
                    AVG({time}){aggregates}
             FROM {}
             WHERE {time} >= ?1 AND {time} <= ?2 {filter}
             GROUP BY bucket ORDER BY bucket",
            table.name()
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params![from, to, points, Key::value(source)])?;
        let mut buckets = Vec::new();
        while let Some(row) = rows.next()? {
            let values = (0..columns.len())
                .map(|n| {
                    let base = 2 + n * 3;
                    Ok([row.get(base)?, row.get(base + 1)?, row.get(base + 2)?])
                })
                .collect::<rusqlite::Result<Vec<[Option<f64>; 3]>>>()?;
            buckets.push(Bucket {
                index: row.get(0)?,
                at: row.get(1)?,
                values,
            });
        }
        Ok(buckets)
    }

    /// One column of every source of a table keyed by the daemon's
    /// source IDs over `window`, on one grid of buckets, a line per
    /// source.
    pub fn overlay(&self, column: Column, window: Window) -> Result<Overlay> {
        let table = column.table;
        let column = column.name;
        let (from, to, points) = window.stored();
        let time = table.time();
        let sql = format!(
            "SELECT source_id, CAST(({time} - ?1) * ?3 / (?2 - ?1 + 1) AS INTEGER) AS bucket,
                    AVG({column})
             FROM {}
             WHERE {time} >= ?1 AND {time} <= ?2
             GROUP BY source_id, bucket ORDER BY source_id, bucket",
            table.name()
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement
            .query_map(params![from, to, points], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Every bucket any source has, and per source its buckets with a
        // break before each gap, then laid on that grid.
        let mut grid: Vec<i64> = rows.iter().map(|r| r.1).collect();
        let mut sources: Vec<i64> = rows.iter().map(|r| r.0).collect();
        sources.dedup();
        let mut lines = Vec::new();
        for source in &sources {
            let own: HashMap<i64, Option<f64>> = rows
                .iter()
                .filter(|r| r.0 == *source)
                .map(|r| (r.1, r.2))
                .collect();
            let mut buckets: Vec<i64> = own.keys().copied().collect();
            buckets.sort_unstable();
            // A break is the bucket before the one after the gap.
            let breaks: Vec<i64> = buckets
                .iter()
                .zip(gaps(&buckets))
                .filter(|(_, broken)| *broken)
                .map(|(bucket, _)| bucket - 1)
                .collect();
            grid.extend(&breaks);
            lines.push((*source, own, breaks));
        }
        grid.sort_unstable();
        grid.dedup();
        let index: HashMap<i64, usize> = grid.iter().enumerate().map(|(n, b)| (*b, n)).collect();
        let width = (to - from + 1) as f64 / points as f64;
        let at = grid
            .iter()
            .map(|bucket| seconds_f(from as f64 + (*bucket as f64 + 0.5) * width))
            .collect();
        let lines = lines
            .into_iter()
            .map(|(source_id, own, breaks)| Line {
                source_id,
                mean: grid
                    .iter()
                    .map(|bucket| own.get(bucket).copied().flatten())
                    .collect(),
                gaps: breaks
                    .iter()
                    .filter_map(|b| index.get(b).copied())
                    .collect(),
            })
            .collect();
        Ok(Overlay { at, lines })
    }

    /// Every value of `column` from `from` to `to`, unix seconds, for
    /// one source where the table is per source and `source_id` says,
    /// sorted, and whether there were more than `limit`, in which case
    /// they are the newest `limit`.  For a distribution, which buckets
    /// would bias: a bucket's mean is not a reading.
    pub fn values(
        &self,
        column: Column,
        source: Option<&Key>,
        from: f64,
        to: f64,
        limit: usize,
    ) -> Result<(Vec<f64>, bool)> {
        let table = column.table;
        let (from, to, _) = Window {
            from,
            to,
            points: MIN_POINTS,
        }
        .stored();
        let time = table.time();
        let filter = source_filter(table, 3);
        let sql = format!(
            // A refclock's filtered sample has no raw offset: NULL, and
            // not a reading.
            "SELECT {column} FROM {table} WHERE {time} >= ?1 AND {time} <= ?2 {filter}
               AND {column} IS NOT NULL
             ORDER BY {time} DESC LIMIT ?4",
            column = column.name,
            table = table.name()
        );
        let mut statement = self.connection.prepare(&sql)?;
        let asked = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let mut values = statement
            .query_map(params![from, to, Key::value(source), asked], |row| {
                row.get::<_, f64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = values.len() > limit;
        values.truncate(limit);
        values.sort_by(f64::total_cmp);
        Ok((values, more))
    }

    /// Events from `from` to `to`, unix seconds, oldest first, at most
    /// `limit` of the newest.
    pub fn events(&self, from: f64, to: f64, limit: usize) -> Result<Vec<Event>> {
        let window = Window {
            from,
            to,
            points: MIN_POINTS,
        };
        let (from, to, _) = window.stored();
        let mut statement = self.connection.prepare(
            "SELECT at, kind, source_id, detail FROM (
                 SELECT rowid, at, kind, source_id, detail FROM event
                 WHERE at >= ?1 AND at <= ?2 ORDER BY at DESC, rowid DESC LIMIT ?3
             ) ORDER BY at, rowid",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        Ok(statement
            .query_map(params![from, to, limit], |row| {
                Ok(Event {
                    at: row.get(0)?,
                    kind: row.get(1)?,
                    source_id: row.get(2)?,
                    detail: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// One bucket of a series: its index on the window's grid, its rows'
/// mean time in unix nanoseconds, and each column's mean, min and max.
struct Bucket {
    index: i64,
    at: f64,
    values: Vec<[Option<f64>; 3]>,
}

/// `buckets`, of `columns` over `window`, as a series, with a break
/// before each gap.
fn assemble(columns: &[Column], buckets: Vec<Bucket>, window: Window) -> Series {
    let (from, to, points) = window.stored();
    let width = (to - from + 1) as f64 / points as f64;
    let breaks = gaps(&buckets.iter().map(|b| b.index).collect::<Vec<_>>());
    let mut series = Series {
        at: Vec::new(),
        plots: columns
            .iter()
            .map(|column| Plot {
                table: column.table.name(),
                column: column.name,
                mean: Vec::new(),
                min: Vec::new(),
                max: Vec::new(),
            })
            .collect(),
    };
    for (bucket, broken) in buckets.into_iter().zip(breaks) {
        if broken {
            series.at.push(seconds_f(bucket.at - width));
            for plot in &mut series.plots {
                plot.mean.push(None);
                plot.min.push(None);
                plot.max.push(None);
            }
        }
        series.at.push(seconds_f(bucket.at));
        for (plot, [mean, min, max]) in series.plots.iter_mut().zip(bucket.values) {
            plot.mean.push(mean);
            plot.min.push(min);
            plot.max.push(max);
        }
    }
    series
}

/// The clause choosing one source's rows, by parameter `n`, or every
/// row when it is NULL.  Bound whatever the table, since SQLite refuses
/// a parameter a statement does not use, so a table without sources
/// gets a clause that is always true.
fn source_filter(table: Table, n: usize) -> String {
    match table.key() {
        Some(key) => format!("AND (?{n} IS NULL OR {key} = ?{n})"),
        None => format!("AND (?{n} IS NULL OR ?{n} IS NOT NULL)"),
    }
}

/// `path` as the path of a `file:` URI: the characters a URI gives a
/// meaning to percent-encoded, the rest as they are.
fn uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            '%' | '?' | '#' => format!("%{:02X}", u32::from(c)),
            c => c.to_string(),
        })
        .collect()
}

/// For each bucket of a sorted list, whether a gap precedes it: a hole
/// of more than [`GAP_BUCKETS`] buckets and more than [`GAP_SPACINGS`]
/// times the list's typical spacing.
fn gaps(buckets: &[i64]) -> Vec<bool> {
    let mut steps: Vec<i64> = buckets.windows(2).map(|w| w[1] - w[0]).collect();
    steps.sort_unstable();
    let typical = steps.get(steps.len() / 2).copied().unwrap_or(1);
    let limit = GAP_BUCKETS.max(typical * GAP_SPACINGS);
    buckets
        .iter()
        .enumerate()
        .map(|(n, bucket)| n > 0 && bucket - buckets[n - 1] > limit)
        .collect()
}

/// Unix nanoseconds as seconds.
fn seconds(stored: i64) -> f64 {
    stored as f64 / NANOSECONDS
}

fn seconds_f(stored: f64) -> f64 {
    stored / NANOSECONDS
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::path::PathBuf;
    use std::time::Duration;

    use chronymon::event::EventKind;
    use chronymon::units::Ppm;
    use chronymon::units::Seconds;
    use chronymon_control::report::Address;
    use chronymon_control::report::Leap;
    use chronymon_control::report::ReferenceId;
    use chronymon_control::report::SourceData;
    use chronymon_control::report::SourceMode;
    use chronymon_control::report::SourceState;
    use jiff::Timestamp;

    use super::*;
    use crate::chrony_log::Line;
    use crate::chrony_log::RefclockLine;
    use crate::chrony_log::TrackingLine;
    use crate::writer::ChronydState;
    use crate::writer::Log;

    const START: i64 = 1_800_000_000;
    const NS: i64 = 1_000_000_000;

    /// A database in a fresh temporary file, removed after.
    struct Database {
        directory: PathBuf,
        log: Log,
    }

    impl Database {
        fn new(name: &str) -> Self {
            let directory = std::env::temp_dir()
                .join(format!("chronymon-reader-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a directory");
            let log = Log::open(&directory.join("test.sqlite")).expect("a database");
            Self { directory, log }
        }

        fn reader(&self) -> Reader {
            Reader::open(&self.directory.join("test.sqlite")).expect("a reader")
        }
    }

    impl Drop for Database {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::new(START + second, 0).expect("in range")
    }

    fn tracking(second: i64, frequency: f64) -> chronymon_control::report::Tracking {
        chronymon_control::report::Tracking {
            reference_id: ReferenceId(1),
            address: Address::Ip(IpAddr::from([192, 0, 2, 1])),
            stratum: 2,
            leap: Leap::Normal,
            reference_time: at(second),
            current_correction: Seconds::new(0.0),
            last_offset: Seconds::new(1e-6),
            rms_offset: Seconds::new(1e-6),
            frequency: Ppm::new(frequency),
            residual_frequency: Ppm::new(0.0),
            skew: Ppm::new(0.01),
            root_delay: Seconds::new(0.01),
            root_dispersion: Seconds::new(0.001),
            last_update_interval: Seconds::new(1.0),
        }
    }

    fn sample(address: [u8; 4], offset: f64) -> SourceData {
        SourceData {
            address: Address::Ip(IpAddr::from(address)),
            poll: 6,
            stratum: 1,
            state: SourceState::Selected,
            mode: SourceMode::Client,
            reachability: 0o377,
            since_sample: Some(Duration::ZERO),
            original_latest_measurement: Seconds::new(offset),
            latest_measurement: Seconds::new(offset),
            latest_measurement_error: Seconds::new(1e-5),
        }
    }

    const SERVER: [u8; 4] = [192, 0, 2, 1];
    const OTHER: [u8; 4] = [192, 0, 2, 2];

    fn column(table: Table, name: &str) -> Column {
        table.column(name).expect("a column")
    }

    fn window(from: i64, to: i64, points: usize) -> Window {
        Window {
            from: (START + from) as f64,
            to: (START + to) as f64,
            points,
        }
    }

    #[test]
    fn reads_with_and_without_the_daemon() {
        let directory =
            std::env::temp_dir().join(format!("chronymon-reader-closed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let log = Log::open(&path).expect("a database");
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        // The daemon running: its WAL is there.
        assert!(Path::new(&wal).exists());
        let reader = Reader::open(&path).expect("a reader");
        assert!(reader.status().expect("a read").is_none());
        drop(reader);
        // The daemon stopped: SQLite takes the WAL away on the last
        // close, and the reader must not make one.
        drop(log);
        assert!(!Path::new(&wal).exists());
        let reader = Reader::open(&path).expect("a reader");
        assert!(reader.status().expect("a read").is_none());
        assert!(!Path::new(&wal).exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_path_is_escaped_for_its_uri() {
        assert_eq!(
            uri_path(Path::new("/var/lib/a?b#c%d")),
            "/var/lib/a%3Fb%23c%25d"
        );
    }

    #[test]
    fn a_missing_database_is_not_created() {
        let path =
            std::env::temp_dir().join(format!("chronymon-missing-{}.sqlite", std::process::id()));
        assert!(Reader::open(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn the_log_fills_where_the_protocol_has_no_rows() {
        let mut db = Database::new("filled");
        db.log
            .write(|batch| {
                // The protocol for the first 50 s, the log from 40 s on:
                // they overlap, and only the protocol is drawn there.
                for second in 0..50 {
                    batch.tracking(at(second), &tracking(second, 1.0))?;
                }
                for second in 40..100 {
                    batch.log_line(&Line::Tracking(TrackingLine {
                        at: at(second),
                        reference: "PPS".to_owned(),
                        stratum: 1,
                        frequency: 2.0,
                        skew: 0.01,
                        offset: 1e-6,
                        leap: 'N',
                        combined_sources: 1,
                        offset_sd: 1e-7,
                        remaining_correction: 0.0,
                        root_delay: 1e-9,
                        root_dispersion: 1e-5,
                        max_error: 1e-5,
                    }))?;
                }
                Ok(())
            })
            .expect("written");
        let series = db
            .reader()
            .filled(
                Table::Tracking,
                &[column(Table::Tracking, "frequency")],
                Table::LogTracking,
                &[column(Table::LogTracking, "frequency")],
                window(0, 99, 100),
            )
            .expect("a series");
        let [reported, logged] = [&series.plots[0], &series.plots[1]];
        assert_eq!((reported.table, logged.table), ("tracking", "log_tracking"));
        assert_eq!(series.at.len(), 100);
        assert!(reported.mean[..50].iter().all(|v| *v == Some(1.0)));
        assert!(reported.mean[50..].iter().all(Option::is_none));
        assert!(logged.mean[..50].iter().all(Option::is_none));
        assert!(logged.mean[50..].iter().all(|v| *v == Some(2.0)));
    }

    #[test]
    fn reads_a_log_table_by_its_names() {
        let mut db = Database::new("logs");
        db.log
            .write(|batch| {
                for second in 0..20 {
                    for (reference, raw) in [("PPS", Some(-15.0)), ("NMEA", None)] {
                        batch.log_line(&Line::Refclock(RefclockLine {
                            at: at(second),
                            reference: reference.to_owned(),
                            driver_polls: Some(1),
                            leap: 'N',
                            pulse: Some(true),
                            raw_offset: raw,
                            cooked_offset: second as f64 * 1e-9,
                            dispersion: 1e-6,
                        }))?;
                    }
                }
                Ok(())
            })
            .expect("written");
        let reader = db.reader();
        let names: Vec<String> = reader
            .keys(Table::RefclockSample)
            .expect("keys")
            .into_iter()
            .map(|n| n.name)
            .collect();
        assert_eq!(names, ["NMEA", "PPS"]);
        assert!(reader.keys(Table::SourceSample).expect("keys").is_empty());
        let pps = Key::Name("PPS".to_owned());
        let cooked = column(Table::RefclockSample, "cooked_offset");
        let series = reader
            .series(
                Table::RefclockSample,
                Some(&pps),
                &[cooked],
                window(0, 19, 20),
            )
            .expect("a series");
        assert_eq!(series.at.len(), 20);
        // NMEA's raw offsets are NULL, filtered samples; none is a reading.
        let raw = column(Table::RefclockSample, "raw_offset");
        let nmea = Key::Name("NMEA".to_owned());
        let (values, _) = reader
            .values(raw, Some(&nmea), START as f64, (START + 19) as f64, 100)
            .expect("values");
        assert!(values.is_empty());
        let (values, _) = reader
            .values(raw, Some(&pps), START as f64, (START + 19) as f64, 100)
            .expect("values");
        assert_eq!(values.len(), 20);
    }

    #[test]
    fn only_listed_columns() {
        assert!(Table::Tracking.column("frequency").is_some());
        assert!(
            Table::Tracking
                .column("frequency; DROP TABLE event")
                .is_none()
        );
        assert!(Table::Tracking.column("offset").is_none());
        for table in Table::ALL {
            assert_eq!(Table::from_name(table.name()), Some(table));
        }
    }

    #[test]
    fn buckets_and_breaks_at_gaps() {
        let mut db = Database::new("series");
        db.log
            .write(|batch| {
                // An update a second for 100 s, then 100 s of nothing,
                // then 100 more.
                for second in (0..100).chain(200..300) {
                    batch.tracking(at(second), &tracking(second, second as f64))?;
                }
                Ok(())
            })
            .expect("written");
        let reader = db.reader();
        let frequency = column(Table::Tracking, "frequency");
        let series = reader
            .series(Table::Tracking, None, &[frequency], window(0, 299, 30))
            .expect("a series");
        let plot = &series.plots[0];
        assert_eq!(plot.column, "frequency");
        assert_eq!(series.at.len(), plot.mean.len());
        // Ten buckets either side, and one break between.
        assert_eq!(series.at.len(), 21);
        assert_eq!(plot.mean.iter().filter(|v| v.is_none()).count(), 1);
        assert_eq!(plot.min[0], Some(0.0));
        assert_eq!(plot.max[0], Some(9.0));
        assert_eq!(plot.mean[0], Some(4.5));
        let (values, more) = reader
            .values(frequency, None, START as f64, (START + 99) as f64, 50)
            .expect("values");
        assert!(more);
        assert_eq!(values.len(), 50);
        assert_eq!((values[0], values[49]), (50.0, 99.0));
        assert_eq!(
            reader.extent(Table::Tracking).expect("an extent"),
            Some((START as f64, (START + 299) as f64))
        );
        let latest = reader.latest_tracking().expect("a read").expect("a row");
        assert_eq!(latest.reference_time, (START + 299) * NS);
        assert!(latest.values.contains(&("frequency", 299.0)));
    }

    #[test]
    fn sparse_samples_are_not_gaps() {
        let mut db = Database::new("sparse");
        db.log
            .write(|batch| {
                let id = batch.source(
                    at(0),
                    sample(SERVER, 0.0).address,
                    ReferenceId(1),
                    SourceMode::Client,
                    Some("ntp.example"),
                )?;
                // A sample every 64 s for an hour.
                for n in 0..57 {
                    batch.source_sample(at(n * 64), id, &sample(SERVER, n as f64 * 1e-6))?;
                }
                Ok(())
            })
            .expect("written");
        let reader = db.reader();
        let offset = column(Table::SourceSample, "latest_measurement");
        let series = reader
            .series(
                Table::SourceSample,
                Some(&Key::Id(1)),
                &[offset],
                window(0, 3600, 1500),
            )
            .expect("a series");
        assert_eq!(series.at.len(), 57);
        assert!(series.plots[0].mean.iter().all(Option::is_some));
        let other = reader
            .series(
                Table::SourceSample,
                Some(&Key::Id(2)),
                &[offset],
                window(0, 3600, 1500),
            )
            .expect("a series");
        assert!(other.at.is_empty());
    }

    #[test]
    fn overlays_sources_on_one_grid() {
        let mut db = Database::new("overlay");
        db.log
            .write(|batch| {
                let a = batch.source(
                    at(0),
                    Address::Ip(IpAddr::from(SERVER)),
                    ReferenceId(1),
                    SourceMode::Client,
                    None,
                )?;
                let b = batch.source(
                    at(0),
                    Address::Ip(IpAddr::from(OTHER)),
                    ReferenceId(2),
                    SourceMode::Client,
                    None,
                )?;
                for n in 0..10 {
                    batch.source_sample(at(n * 10), a, &sample(SERVER, 1.0))?;
                    batch.source_sample(at(n * 10 + 5), b, &sample(OTHER, 2.0))?;
                }
                // One more of `a` after a long silence: a gap in its line
                // alone.
                batch.source_sample(at(190), a, &sample(SERVER, 1.0))?;
                Ok(())
            })
            .expect("written");
        let overlay = db
            .reader()
            .overlay(
                column(Table::SourceSample, "latest_measurement"),
                window(0, 199, 200),
            )
            .expect("an overlay");
        assert_eq!(overlay.lines.len(), 2);
        // Each source's ten, `a`'s last, and the bucket marking its gap.
        assert_eq!(overlay.at.len(), 22);
        for line in &overlay.lines {
            assert_eq!(line.mean.len(), overlay.at.len());
        }
        let [a, b] = [&overlay.lines[0], &overlay.lines[1]];
        assert_eq!(a.mean.iter().filter(|v| v.is_some()).count(), 11);
        assert_eq!(b.mean.iter().filter(|v| v.is_some()).count(), 10);
        assert_eq!(a.gaps.len(), 1);
        assert!(a.mean[a.gaps[0]].is_none());
        assert!(b.gaps.is_empty());
    }

    #[test]
    fn reads_sources_status_and_events() {
        let mut db = Database::new("sources");
        db.log
            .write(|batch| {
                batch.status(at(5), ChronydState::Up, None, Some(42))?;
                let id = batch.source(
                    at(0),
                    sample(SERVER, 0.0).address,
                    ReferenceId(1),
                    SourceMode::Client,
                    Some("ntp.example"),
                )?;
                batch.source_sample(at(0), id, &sample(SERVER, 1e-6))?;
                batch.source_sample(at(64), id, &sample(SERVER, 2e-6))?;
                for second in 0..5 {
                    batch.event(at(second), EventKind::Step, None, Some("detail"))?;
                }
                Ok(())
            })
            .expect("written");
        let reader = db.reader();
        let status = reader.status().expect("a read").expect("a status");
        assert_eq!(status.state, "up");
        assert_eq!(status.chronyd_pid, Some(42));
        let sources = reader.sources().expect("sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name.as_deref(), Some("ntp.example"));
        let (state, values) = sources[0].sample.clone().expect("a sample");
        assert_eq!(state, "selected");
        assert!(values.contains(&("latest_measurement", 2e-6)));
        assert!(sources[0].stats.is_none());
        let events = reader
            .events(START as f64, (START + 10) as f64, 3)
            .expect("events");
        assert_eq!(
            events.iter().map(|e| e.at).collect::<Vec<_>>(),
            [2, 3, 4].map(|s| (START + s) * NS)
        );
    }
}
