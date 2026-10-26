//! The JSON the pages read, from the database.
//!
//! Times are unix seconds, as numbers, which is what a chart's time axis
//! takes.  Requests name tables and columns by the reader's own names
//! and are checked against its lists, and against what one answer may
//! cost, before the database is opened: a request that fails either is
//! [`Refused`].

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use chronymon_log::reader::Column;
use chronymon_log::reader::Key;
use chronymon_log::reader::MAX_POINTS;
use chronymon_log::reader::MIN_POINTS;
use chronymon_log::reader::Reader;
use chronymon_log::reader::Table;
use chronymon_log::reader::Values;
use chronymon_log::reader::Window;
use jiff::Timestamp;
use serde_json::Value;
use serde_json::json;

/// The strip's name for chronyd's reference while it serves its local
/// reference, whose address is unspecified.
const LOCAL_REFERENCE: &str = "local reference";

/// The window shown when a request names none, in seconds: the last
/// hour of what the table holds.
const DEFAULT_WINDOW: f64 = 3600.0;
/// Buckets when a request names no number.
const DEFAULT_POINTS: usize = 1500;
/// The most buckets of all its columns together one history answers:
/// the Live page's every column and its log's, at the most points, with
/// room to spare.  Each is three numbers, so a request for a column many
/// times over could otherwise cost hundreds of megabytes.
const MAX_CELLS: usize = 16 * MAX_POINTS;
/// The most events one request may ask for, and how many when it names
/// no number.
const MAX_EVENTS: usize = 5000;
const DEFAULT_EVENTS: usize = 500;
/// The most readings a distribution is drawn from: some weeks of a
/// once-a-second clock, which a sort handles in well under a second.
const MAX_VALUES: usize = 2_000_000;
/// The percentiles a distribution reports.
const PERCENTILES: [f64; 9] = [0.1, 1.0, 5.0, 25.0, 50.0, 75.0, 95.0, 99.0, 99.9];
/// The histogram's bins, and the percentiles its ends are at: the rest
/// is counted as below or above, so one outlier does not squeeze the
/// whole distribution into a bin.
const BINS: usize = 60;
const HISTOGRAM_FROM: f64 = 0.5;
const HISTOGRAM_TO: f64 = 99.5;

/// Nanoseconds in a second.
const NANOSECONDS: f64 = 1e9;

/// The kernel's name for this host, which is chronyd's: the web view
/// runs beside the daemon it reads (one host, PLAN.md).
const HOSTNAME: &str = "/proc/sys/kernel/hostname";

/// This host's name, for the pages to say whose clock they show; None
/// where the kernel will not say.
fn host() -> Option<String> {
    let name = fs::read_to_string(HOSTNAME).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A request this does not answer: a name it does not serve, a value
/// that does not parse, or more than one answer may cost.  Answered 400,
/// where a database that cannot answer is the server's failure.
#[derive(Debug)]
pub(crate) struct Refused(String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// A request refused, for `why`.
fn refused(why: String) -> anyhow::Error {
    Refused(why).into()
}

/// `f` of the database at `path`, open to read (`Reader::read`), or
/// why not: a database that is not there yet is the daemon not having
/// run, which the pages say.
fn read<T>(path: &Path, f: impl Fn(&Reader) -> Result<T>) -> Result<T> {
    if !path.exists() {
        bail!(
            "no database at {} yet: chronymond has not run, or writes elsewhere",
            path.display()
        );
    }
    Reader::read(path, f)
}

/// Unix nanoseconds, as stored, as seconds.
fn seconds(stored: i64) -> f64 {
    stored as f64 / NANOSECONDS
}

fn now() -> f64 {
    let ms = Timestamp::now().as_millisecond() as f64;
    ms / 1e3
}

/// Columns as an object, by name.
fn object(values: &Values) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(k, v)| ((*k).to_owned(), json!(v)))
            .collect(),
    )
}

/// One query parameter.
fn value(query: &str, key: &str) -> Option<String> {
    chronymon_http::value(query, key)
}

/// A number a request gives, if it gives one.
fn number(query: &str, key: &str) -> Result<Option<f64>> {
    value(query, key)
        .map(|v| {
            v.parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| refused(format!("{key}={v} is not a number")))
        })
        .transpose()
}

/// A count a request gives, if it gives one, at most `most`.
fn count(query: &str, key: &str, most: usize) -> Result<Option<usize>> {
    value(query, key)
        .map(|v| match v.parse::<usize>() {
            Ok(n) if n <= most => Ok(n),
            Ok(_) => Err(refused(format!("{key}={v} is more than {most}"))),
            Err(_) => Err(refused(format!("{key}={v} is not a count"))),
        })
        .transpose()
}

/// The table named `name`.
fn named(name: &str) -> Result<Table> {
    Table::from_name(name).ok_or_else(|| refused(format!("{name} is not a table this serves")))
}

/// The table a request names, `tracking` by default.
fn table(query: &str) -> Result<Table> {
    value(query, "table").map_or(Ok(Table::Tracking), |name| named(&name))
}

fn column(table: Table, name: &str) -> Result<Column> {
    table.column(name).ok_or_else(|| {
        refused(format!(
            "{name} is not a column of {} this serves; it has {}",
            table.name(),
            table.columns().join(", ")
        ))
    })
}

/// The one column of `table` a request names, `column=`.
fn one_column(query: &str, table: Table) -> Result<Column> {
    let name =
        value(query, "column").ok_or_else(|| refused("which column? no column=".to_owned()))?;
    column(table, &name)
}

/// The columns of `table` a request lists in `key`, each once.
fn columns(query: &str, key: &str, table: Table) -> Result<Vec<Column>> {
    let listed = value(query, key).ok_or_else(|| refused(format!("which columns? no {key}=")))?;
    let columns = listed
        .split(',')
        .filter(|c| !c.is_empty())
        .map(|c| column(table, c))
        .collect::<Result<Vec<_>>>()?;
    if columns.is_empty() {
        return Err(refused(format!("no columns in {key}=")));
    }
    if let Some(twice) = columns
        .iter()
        .enumerate()
        .find_map(|(n, c)| columns[..n].contains(c).then_some(c))
    {
        return Err(refused(format!("{} is in {key}= twice", twice.name())));
    }
    Ok(columns)
}

/// The buckets a request asks for, `points=`, as the reader will give
/// them.
fn points(query: &str) -> Result<usize> {
    Ok(count(query, "points", MAX_POINTS)?
        .unwrap_or(DEFAULT_POINTS)
        .max(MIN_POINTS))
}

/// The source a request names: the daemon's ID for a table keyed by
/// it, the log's name for a log's.
fn source(query: &str, table: Table) -> Result<Option<Key>> {
    value(query, "source")
        .map(|v| {
            if table.by_id() {
                v.parse::<i64>()
                    .map(Key::Id)
                    .with_context(|| format!("{v} is not a source ID"))
            } else {
                Ok(Key::Name(v))
            }
        })
        .transpose()
        .map_err(|e| refused(format!("{e:#}")))
}

/// The window a request names, or the last hour `table` holds, and the
/// table's extent, which the page needs for `all`.
fn window(reader: &Reader, tables: &[Table], asked: Asked) -> Result<(Window, Value)> {
    // The tables' extents together: a filled series spans both.
    let extent = tables
        .iter()
        .try_fold(None, |held: Option<(f64, f64)>, table| {
            Ok::<_, anyhow::Error>(match (held, reader.extent(*table)?) {
                (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
                (one, other) => one.or(other),
            })
        })?;
    let to = asked
        .to
        .unwrap_or_else(|| extent.map_or_else(now, |(_, last)| last));
    let from = asked.from.unwrap_or(to - DEFAULT_WINDOW);
    if from > to {
        return Err(refused(BACKWARD.to_owned()));
    }
    let (first, last) = extent.map_or((None, None), |(first, last)| (Some(first), Some(last)));
    Ok((
        Window {
            from,
            to,
            points: asked.points,
        },
        json!({ "from": from, "to": to, "first": first, "last": last }),
    ))
}

/// The range and buckets a request asks for, checked before the
/// database is opened; an end it leaves open is the window's to fill.
#[derive(Debug, Clone, Copy)]
struct Asked {
    /// `from=`, unix seconds.
    from: Option<f64>,
    /// `to=`, unix seconds.
    to: Option<f64>,
    /// `points=`, as [`points`] gives it.
    points: usize,
}

/// What `query` asks for, bucketed into `points`.
fn asked(query: &str, points: usize) -> Result<Asked> {
    let (from, to) = (number(query, "from")?, number(query, "to")?);
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return Err(refused(BACKWARD.to_owned()));
    }
    Ok(Asked { from, to, points })
}

/// Why a range is refused whose end is before its start.
const BACKWARD: &str = "the range ends before it starts";

/// Add `bounds`' fields to `value`, an object.
fn with(mut value: Value, bounds: Value) -> Value {
    if let (Some(fields), Value::Object(extra)) = (value.as_object_mut(), bounds) {
        fields.extend(extra);
    }
    value
}

/// Each source's name to show: its configured name, else its address.
fn names(reader: &Reader) -> Result<HashMap<i64, String>> {
    Ok(reader
        .sources()?
        .into_iter()
        .map(|s| (s.id, s.name.filter(|n| !n.is_empty()).unwrap_or(s.address)))
        .collect())
}

/// `/api/status`: the daemon's last poll and the newest clock update,
/// with the reference's name.
pub(crate) fn status(path: &Path) -> Result<Value> {
    read(path, |reader| {
        let now = now();
        let status = reader.status()?.map(|s| {
            json!({
                "polled_at": seconds(s.polled_at),
                "age": now - seconds(s.polled_at),
                "state": s.state,
                "detail": s.detail,
                "chronyd_pid": s.chronyd_pid,
            })
        });
        let tracking = reader.latest_tracking()?;
        // A refclock's address is its reference ID among the sources, but
        // tracking leaves it unspecified (chrony 4.6.1 `reference.c:959`).
        let reference_name = match &tracking {
            Some(t) if t.local() => Some(LOCAL_REFERENCE.to_owned()),
            Some(t) => {
                let reference_id = format!("{:08X}", t.reference_id);
                reader
                    .sources()?
                    .into_iter()
                    .find(|s| s.address == t.reference_address || s.address == reference_id)
                    .and_then(|s| s.name)
            }
            None => None,
        };
        let tracking = tracking.map(|t| {
            json!({
                // None while unsynchronized, which chronyd reports as 0.
                "reference_time": (t.reference_time != 0).then(|| seconds(t.reference_time)),
                "reference_id": format!("{:08X}", t.reference_id),
                "reference_address": t.reference_address,
                "stratum": t.stratum,
                "leap": t.leap,
                "values": object(&t.values),
            })
        });
        Ok(json!({
            "now": now,
            "host": host(),
            "status": status,
            "tracking": tracking,
            "reference_name": reference_name,
        }))
    })
}

/// The table and columns a request fills from, `fill=` and
/// `fill_columns=`, if it names one.
fn fill(query: &str) -> Result<Option<(Table, Vec<Column>)>> {
    value(query, "fill")
        .map(|name| {
            let fill = named(&name)?;
            Ok((fill, columns(query, "fill_columns", fill)?))
        })
        .transpose()
}

/// `/api/keys?table=`: the sources a log table names, by name, with
/// their rows and the span of them.
pub(crate) fn keys(path: &Path, query: &str) -> Result<Value> {
    let table = table(query)?;
    read(path, |reader| {
        Ok(Value::Array(
            reader
                .keys(table)?
                .into_iter()
                .map(
                    |n| json!({ "name": n.name, "rows": n.rows, "first": n.first, "last": n.last }),
                )
                .collect(),
        ))
    })
}

/// `/api/history?table=&columns=a,b&from=&to=&points=&source=`: one
/// table's columns, bucketed on one grid, each with the indexes its
/// line breaks at; with `fill=&fill_columns=`, a log's where the daemon
/// was not recording.  `unsynchronized` and `local`: `[from, to]`
/// pairs, for `tracking` when chronyd was unsynchronized and on its
/// local reference, else none.
pub(crate) fn history(path: &Path, query: &str) -> Result<Value> {
    let table = table(query)?;
    let columns = columns(query, "columns", table)?;
    let fill = fill(query)?;
    let source = source(query, table)?;
    let points = points(query)?;
    let cells = (columns.len() + fill.as_ref().map_or(0, |(_, c)| c.len())) * points;
    if cells > MAX_CELLS {
        return Err(refused(format!(
            "{cells} buckets, its columns at points={points}, is more than {MAX_CELLS}"
        )));
    }
    let asked = asked(query, points)?;
    read(path, |reader| {
        let tables: Vec<Table> = [Some(table), fill.as_ref().map(|f| f.0)]
            .into_iter()
            .flatten()
            .collect();
        let (window, bounds) = window(reader, &tables, asked)?;
        let series = match &fill {
            Some((fill, fill_columns)) => {
                reader.filled(table, &columns, *fill, fill_columns, window)?
            }
            None => reader.series(table, source.as_ref(), &columns, window)?,
        };
        let plots: Vec<Value> = series
            .plots
            .iter()
            .map(|p| {
                json!({
                    "table": p.table,
                    "column": p.column,
                    "mean": p.mean,
                    "min": p.min,
                    "max": p.max,
                    "gaps": p.gaps,
                })
            })
            .collect();
        // Where the clock's lines break for chronyd not synchronized to
        // a source, for the charts to mark.
        let not_synchronized = if table == Table::Tracking {
            Some(reader.not_synchronized(window)?)
        } else {
            None
        };
        let (unsynchronized, local) = not_synchronized
            .map(|n| (n.unsynchronized, n.local))
            .unwrap_or_default();
        Ok(with(
            json!({
                "at": series.at,
                "plots": plots,
                "unsynchronized": unsynchronized,
                "local": local,
            }),
            bounds,
        ))
    })
}

/// `/api/overlay?table=&column=&from=&to=&points=`: one column of every
/// source, a line each, on one grid.
pub(crate) fn overlay(path: &Path, query: &str) -> Result<Value> {
    let table = table(query)?;
    if !table.by_id() {
        return Err(refused(format!(
            "{} is not a table of the daemon's sources",
            table.name()
        )));
    }
    let column = one_column(query, table)?;
    let asked = asked(query, points(query)?)?;
    read(path, |reader| {
        let (window, bounds) = window(reader, &[table], asked)?;
        let overlay = reader.overlay(column, window)?;
        let lines: Vec<Value> = overlay
            .lines
            .iter()
            .map(|l| json!({ "source_id": l.source_id, "mean": l.mean, "gaps": l.gaps }))
            .collect();
        Ok(with(json!({ "at": overlay.at, "lines": lines }), bounds))
    })
}

/// `/api/sources`: every source with the latest of each report, those
/// chronyd has removed last.
pub(crate) fn sources(path: &Path) -> Result<Value> {
    read(path, |reader| {
        let listed = reader.sources()?;
        // Each refclock another is locked to, by address, and the name
        // of the one locked to it: its PPS, beside which its own offsets
        // are noise.
        let locked_by: HashMap<String, String> = reader
            .refclock_locks()?
            .into_iter()
            .map(|lock| {
                let refclock = lock.refclock.to_string();
                let name = listed
                    .iter()
                    .find(|s| s.address == refclock)
                    .and_then(|s| s.name.clone())
                    .or_else(|| lock.refclock.text())
                    .unwrap_or(refclock);
                (lock.locked_to.to_string(), name)
            })
            .collect();
        let sources: Vec<Value> = listed
            .into_iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "locked_by": locked_by.get(&s.address),
                    "address": s.address,
                    "mode": s.mode,
                    "name": s.name,
                    "first_seen": seconds(s.first_seen),
                    "last_seen": seconds(s.last_seen),
                    "removed": s.removed,
                    "state": s.report.as_ref().map(|(state, _)| state),
                    "report": s.report.as_ref().map(|(_, values)| object(values)),
                    "selection_state": s.selection.as_ref().map(|(state, _)| state),
                    "selection": s.selection.as_ref().map(|(_, values)| object(values)),
                    "stats": s.stats.as_ref().map(object),
                    "ntp": s.ntp.as_ref().map(object),
                })
            })
            .collect();
        Ok(Value::Array(sources))
    })
}

/// `/api/events?from=&to=&limit=`: events oldest first, each with its
/// source's name.
pub(crate) fn events(path: &Path, query: &str) -> Result<Value> {
    let to = number(query, "to")?.unwrap_or_else(now);
    let from = number(query, "from")?.unwrap_or(0.0);
    let limit = count(query, "limit", MAX_EVENTS)?.unwrap_or(DEFAULT_EVENTS);
    read(path, |reader| {
        let names = names(reader)?;
        let events: Vec<Value> = reader
            .events(from, to, limit)?
            .into_iter()
            .map(|e| {
                json!({
                    "at": seconds(e.at),
                    "kind": e.kind,
                    "source_id": e.source_id,
                    "source": e.source_id.and_then(|id| names.get(&id)),
                    "detail": e.detail,
                })
            })
            .collect();
        Ok(Value::Array(events))
    })
}

/// The `p`th percentile of sorted `values`, by linear interpolation
/// between the nearest ranks.
fn percentile(values: &[f64], p: f64) -> Option<f64> {
    let last = values.len().checked_sub(1)?;
    let rank = p / 100.0 * last as f64;
    let below = rank.floor() as usize;
    let above = (below + 1).min(last);
    let fraction = rank - below as f64;
    Some(values[below] + (values[above] - values[below]) * fraction)
}

/// `/api/distribution?table=&column=&from=&to=&source=`: percentiles
/// and a histogram of one column's readings over a range.
pub(crate) fn distribution(path: &Path, query: &str) -> Result<Value> {
    let table = table(query)?;
    let column = one_column(query, table)?;
    let source = source(query, table)?;
    // Not bucketed: the points only fill in the window.
    let asked = asked(query, DEFAULT_POINTS)?;
    read(path, |reader| {
        let (window, bounds) = window(reader, &[table], asked)?;
        let (values, truncated) =
            reader.values(column, source.as_ref(), window.from, window.to, MAX_VALUES)?;
        let percentiles: Vec<Value> = PERCENTILES
            .iter()
            .map(|p| json!({ "p": p, "value": percentile(&values, *p) }))
            .collect();
        let count = values.len() as f64;
        let mean = values.iter().sum::<f64>() / count;
        let deviation = (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count).sqrt();
        let histogram = match (
            percentile(&values, HISTOGRAM_FROM),
            percentile(&values, HISTOGRAM_TO),
        ) {
            (Some(lo), Some(hi)) if hi > lo => {
                let width = (hi - lo) / BINS as f64;
                let mut counts = vec![0_u64; BINS];
                let (mut below, mut above) = (0_u64, 0_u64);
                for v in &values {
                    if *v < lo {
                        below += 1;
                    } else if *v > hi {
                        above += 1;
                    } else {
                        let bin = (((v - lo) / width) as usize).min(BINS - 1);
                        counts[bin] += 1;
                    }
                }
                let edges: Vec<f64> = (0..=BINS).map(|n| lo + n as f64 * width).collect();
                json!({ "edges": edges, "counts": counts, "below": below, "above": above })
            }
            _ => Value::Null,
        };
        Ok(with(
            json!({
                "column": column.name(),
                "count": values.len(),
                "truncated": truncated,
                "mean": if values.is_empty() { None } else { Some(mean) },
                "standard_deviation": if values.is_empty() { None } else { Some(deviation) },
                "percentiles": percentiles,
                "histogram": histogram,
            }),
            bounds,
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use chronymon_control::report::Address;
    use chronymon_control::report::ReferenceId;
    use chronymon_control::report::SourceMode;
    use chronymon_log::schema::RefclockLock;
    use chronymon_log::writer::Log;

    use super::*;

    #[test]
    fn percentiles_interpolate() {
        let values = [0.0, 10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&values, 0.0), Some(0.0));
        assert_eq!(percentile(&values, 50.0), Some(20.0));
        assert_eq!(percentile(&values, 100.0), Some(40.0));
        assert_eq!(percentile(&values, 12.5), Some(5.0));
        assert_eq!(percentile(&[], 50.0), None);
    }

    /// A path with no database: a request refused never reaches it, and
    /// one that is not refused stops there.
    const NOWHERE: &str = "/nonexistent/chronymon.sqlite";

    /// Why `answer` failed, if it was refused, else None.
    fn refusal(answer: Result<Value>) -> Option<String> {
        let e = answer.expect_err("answered");
        e.downcast_ref::<Refused>().map(ToString::to_string)
    }

    fn history_refusal(query: &str) -> Option<String> {
        refusal(history(Path::new(NOWHERE), query))
    }

    /// Each source's name and `locked_by`, from `/api/sources` of a
    /// database of a PPS and an NMEA refclock and an NTP server, with
    /// `locks` recorded, or none.
    fn locked_by(name: &str, locks: Option<&[RefclockLock]>) -> Vec<(String, Value)> {
        let directory =
            std::env::temp_dir().join(format!("chronymon-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("test.sqlite");
        let mut log = Log::open(&path).expect("a database");
        log.write(|batch| {
            let at = Timestamp::UNIX_EPOCH;
            for (id, mode) in [
                (*b"PPS\0", SourceMode::Refclock),
                (*b"NMEA", SourceMode::Refclock),
                ([192, 0, 2, 1], SourceMode::Client),
            ] {
                let reference_id = ReferenceId(u32::from_be_bytes(id));
                let address = match mode {
                    SourceMode::Refclock => Address::ReferenceId(reference_id.0),
                    _ => Address::Ip(IpAddr::from(id)),
                };
                batch.source(
                    at,
                    address,
                    reference_id,
                    mode,
                    reference_id.text().as_deref(),
                )?;
            }
            locks.map_or(Ok(()), |locks| batch.refclock_locks(locks))
        })
        .expect("written");
        let answer = sources(&path).expect("sources");
        std::fs::remove_dir_all(&directory).expect("removed");
        answer
            .as_array()
            .expect("an array")
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap_or_default().to_owned(),
                    s["locked_by"].clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_refclock_locked_to_is_marked_with_the_one_locked_to_it() {
        let pps = ReferenceId(u32::from_be_bytes(*b"PPS\0"));
        let nmea = ReferenceId(u32::from_be_bytes(*b"NMEA"));
        let paired = [RefclockLock {
            refclock: pps,
            locked_to: nmea,
        }];
        let null = Value::Null;
        assert_eq!(
            locked_by("paired", Some(&paired)),
            [
                ("PPS".to_owned(), null.clone()),
                ("NMEA".to_owned(), json!("PPS")),
                (String::new(), null.clone()),
            ]
        );
        // A lone NMEA, and a database no daemon has read a configuration
        // into: none is.
        for locks in [Some(&[][..]), None] {
            assert!(
                locked_by("lone", locks).iter().all(|(_, by)| by.is_null()),
                "{locks:?}"
            );
        }
    }

    #[test]
    fn a_column_asked_for_twice_is_refused() {
        let why = history_refusal("columns=poll,stratum,poll&table=source_sample");
        assert_eq!(why.as_deref(), Some("poll is in columns= twice"));
        // A small request repeating a column hundreds of times.
        let many = vec!["skew"; 600].join(",");
        assert!(history_refusal(&format!("columns={many}")).is_some());
        let why = history_refusal("columns=frequency&fill=log_tracking&fill_columns=skew,skew");
        assert_eq!(why.as_deref(), Some("skew is in fill_columns= twice"));
    }

    #[test]
    fn too_many_points_or_cells_are_refused() {
        let why = history_refusal(&format!("columns=skew&points={}", MAX_POINTS + 1));
        assert_eq!(
            why,
            Some(format!(
                "points={} is more than {MAX_POINTS}",
                MAX_POINTS + 1
            ))
        );
        assert!(history_refusal("columns=skew&points=-1").is_some());
        assert!(history_refusal("columns=skew&points=1e9").is_some());
        // Every column of tracking and of its log, each once, at the most
        // points: more than one answer may cost.
        let tracking = Table::Tracking.columns().join(",");
        let logged = Table::LogTracking.columns().join(",");
        let all = format!("columns={tracking}&fill=log_tracking&fill_columns={logged}");
        let cells =
            (Table::Tracking.columns().len() + Table::LogTracking.columns().len()) * MAX_POINTS;
        assert!(cells > MAX_CELLS);
        assert!(history_refusal(&format!("{all}&points={MAX_POINTS}")).is_some());
        // At fewer points, the same columns are answered.
        assert_eq!(
            history_refusal(&format!("{all}&points={DEFAULT_POINTS}")),
            None
        );
    }

    #[test]
    fn what_the_pages_ask_for_reaches_the_database() {
        // Live's default columns and their log's, at the most points.
        let live = "table=tracking&columns=last_offset,frequency,skew,rms_offset,root_delay,root_dispersion\
                    &points=5000&from=0&to=100&fill=log_tracking\
                    &fill_columns=offset,frequency,skew,root_delay,root_dispersion";
        assert_eq!(history_refusal(live), None);
        assert_eq!(
            refusal(overlay(
                Path::new(NOWHERE),
                "table=ntp_sample&column=offset&points=5000"
            )),
            None
        );
        assert_eq!(
            refusal(events(Path::new(NOWHERE), "limit=500&from=0")),
            None
        );
    }

    #[test]
    fn every_other_parameter_is_checked() {
        let nowhere = Path::new(NOWHERE);
        for query in [
            "columns=skew&from=yesterday",
            "columns=skew&to=inf",
            "columns=skew&from=10&to=5",
            "columns=",
            "table=nope&columns=skew",
            "columns=nope",
            "table=ntp_sample&columns=offset&source=one",
            "columns=frequency&fill=log_tracking",
            "columns=frequency&fill=nope&fill_columns=skew",
        ] {
            assert!(history_refusal(query).is_some(), "{query}");
        }
        for answer in [
            overlay(nowhere, "table=tracking&column=skew"),
            overlay(nowhere, "table=ntp_sample"),
            overlay(
                nowhere,
                &format!("table=ntp_sample&column=offset&points={}", MAX_POINTS + 1),
            ),
            events(nowhere, &format!("limit={}", MAX_EVENTS + 1)),
            events(nowhere, "limit=many"),
            events(nowhere, "from=x"),
            keys(nowhere, "table=nope"),
            distribution(nowhere, "column=nope"),
            distribution(nowhere, "column=skew&to=now"),
        ] {
            assert!(refusal(answer).is_some());
        }
    }
}
