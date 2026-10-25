//! The JSON the pages read, from the database.
//!
//! Times are unix seconds, as numbers, which is what a chart's time axis
//! takes.  Requests name tables and columns by the reader's own names
//! and are checked against its lists before reaching it.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use chronymon_log::reader::Column;
use chronymon_log::reader::Key;
use chronymon_log::reader::Reader;
use chronymon_log::reader::Table;
use chronymon_log::reader::Values;
use chronymon_log::reader::Window;
use jiff::Timestamp;
use serde_json::Value;
use serde_json::json;

/// The window shown when a request names none, in seconds: the last
/// hour of what the table holds.
const DEFAULT_WINDOW: f64 = 3600.0;
/// Buckets when a request names no number.
const DEFAULT_POINTS: usize = 1500;
/// The most events one request is given, and the most when it names no
/// number.
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

/// The database at `path`, open to read, or why not: a database that is
/// not there yet is the daemon not having run, which the pages say.
fn open(path: &Path) -> Result<Reader> {
    if !path.exists() {
        bail!(
            "no database at {} yet: chronymond has not run, or writes elsewhere",
            path.display()
        );
    }
    Reader::open(path).with_context(|| format!("opening {}", path.display()))
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

fn number(query: &str, key: &str) -> Option<f64> {
    value(query, key)
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// The table a request names, `tracking` by default.
fn table(query: &str) -> Result<Table> {
    let name = value(query, "table").unwrap_or_else(|| Table::Tracking.name().to_owned());
    Table::from_name(&name).ok_or_else(|| anyhow!("{name} is not a table this serves"))
}

fn column(table: Table, name: &str) -> Result<Column> {
    table.column(name).ok_or_else(|| {
        anyhow!(
            "{name} is not a column of {} this serves; it has {}",
            table.name(),
            table.columns().join(", ")
        )
    })
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
}

/// The window a request names, or the last hour `table` holds, and the
/// table's extent, which the page needs for `all`.
fn window(reader: &Reader, tables: &[Table], query: &str) -> Result<(Window, Value)> {
    // The tables' extents together: a filled series spans both.
    let extent = tables
        .iter()
        .try_fold(None, |held: Option<(f64, f64)>, table| {
            Ok::<_, anyhow::Error>(match (held, reader.extent(*table)?) {
                (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
                (one, other) => one.or(other),
            })
        })?;
    let to = number(query, "to").unwrap_or_else(|| extent.map_or_else(now, |(_, last)| last));
    let from = number(query, "from").unwrap_or(to - DEFAULT_WINDOW);
    if from > to {
        bail!("the range ends before it starts");
    }
    let points = value(query, "points")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_POINTS);
    let (first, last) = extent.map_or((None, None), |(first, last)| (Some(first), Some(last)));
    Ok((
        Window { from, to, points },
        json!({ "from": from, "to": to, "first": first, "last": last }),
    ))
}

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
    let reader = open(path)?;
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
    let reference_name = match &tracking {
        Some(t) => reader
            .sources()?
            .into_iter()
            .find(|s| s.address == t.reference_address)
            .and_then(|s| s.name),
        None => None,
    };
    let tracking = tracking.map(|t| {
        json!({
            "reference_time": seconds(t.reference_time),
            "reference_id": format!("{:08X}", t.reference_id),
            "reference_address": t.reference_address,
            "stratum": t.stratum,
            "leap": t.leap,
            "values": object(&t.values),
        })
    });
    Ok(json!({
        "now": now,
        "status": status,
        "tracking": tracking,
        "reference_name": reference_name,
    }))
}

/// The table and columns a request fills from, `fill=` and
/// `fill_columns=`, if it names one.
fn fill(query: &str) -> Result<Option<(Table, Vec<Column>)>> {
    let Some(name) = value(query, "fill") else {
        return Ok(None);
    };
    let fill =
        Table::from_name(&name).ok_or_else(|| anyhow!("{name} is not a table this serves"))?;
    let columns = value(query, "fill_columns")
        .ok_or_else(|| anyhow!("which columns of {name}?"))?
        .split(',')
        .filter(|c| !c.is_empty())
        .map(|c| column(fill, c))
        .collect::<Result<Vec<_>>>()?;
    Ok(Some((fill, columns)))
}

/// `/api/keys?table=`: the sources a log table names, by name, with
/// their rows and the span of them.
pub(crate) fn keys(path: &Path, query: &str) -> Result<Value> {
    let reader = open(path)?;
    let table = table(query)?;
    Ok(Value::Array(
        reader
            .keys(table)?
            .into_iter()
            .map(|n| json!({ "name": n.name, "rows": n.rows, "first": n.first, "last": n.last }))
            .collect(),
    ))
}

/// `/api/history?table=&columns=a,b&from=&to=&points=&source=`: one
/// table's columns, bucketed on one grid, each with the indexes its
/// line breaks at; with `fill=&fill_columns=`, a log's where the daemon
/// was not recording.
pub(crate) fn history(path: &Path, query: &str) -> Result<Value> {
    let reader = open(path)?;
    let table = table(query)?;
    let columns = value(query, "columns")
        .ok_or_else(|| anyhow!("which columns?"))?
        .split(',')
        .filter(|c| !c.is_empty())
        .map(|c| column(table, c))
        .collect::<Result<Vec<_>>>()?;
    if columns.is_empty() {
        bail!("no columns asked for");
    }
    let fill = fill(query)?;
    let tables: Vec<Table> = [Some(table), fill.as_ref().map(|f| f.0)]
        .into_iter()
        .flatten()
        .collect();
    let (window, bounds) = window(&reader, &tables, query)?;
    let series = match fill {
        Some((fill, fill_columns)) => {
            reader.filled(table, &columns, fill, &fill_columns, window)?
        }
        None => reader.series(table, source(query, table)?.as_ref(), &columns, window)?,
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
    Ok(with(json!({ "at": series.at, "plots": plots }), bounds))
}

/// `/api/overlay?table=&column=&from=&to=&points=`: one column of every
/// source, a line each, on one grid.
pub(crate) fn overlay(path: &Path, query: &str) -> Result<Value> {
    let reader = open(path)?;
    let table = table(query)?;
    if !table.by_id() {
        bail!("{} is not a table of the daemon's sources", table.name());
    }
    let name = value(query, "column").ok_or_else(|| anyhow!("which column?"))?;
    let column = column(table, &name)?;
    let (window, bounds) = window(&reader, &[table], query)?;
    let overlay = reader.overlay(column, window)?;
    let lines: Vec<Value> = overlay
        .lines
        .iter()
        .map(|l| json!({ "source_id": l.source_id, "mean": l.mean, "gaps": l.gaps }))
        .collect();
    Ok(with(json!({ "at": overlay.at, "lines": lines }), bounds))
}

/// `/api/sources`: every source with the latest of each report, those
/// chronyd has removed last.
pub(crate) fn sources(path: &Path) -> Result<Value> {
    let reader = open(path)?;
    let sources: Vec<Value> = reader
        .sources()?
        .into_iter()
        .map(|s| {
            json!({
                "id": s.id,
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
}

/// `/api/events?from=&to=&limit=`: events oldest first, each with its
/// source's name.
pub(crate) fn events(path: &Path, query: &str) -> Result<Value> {
    let reader = open(path)?;
    let names = names(&reader)?;
    let to = number(query, "to").unwrap_or_else(now);
    let from = number(query, "from").unwrap_or(0.0);
    let limit = value(query, "limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_EVENTS)
        .min(MAX_EVENTS);
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
    let reader = open(path)?;
    let table = table(query)?;
    let name = value(query, "column").ok_or_else(|| anyhow!("which column?"))?;
    let column = column(table, &name)?;
    let (window, bounds) = window(&reader, &[table], query)?;
    let (values, truncated) = reader.values(
        column,
        source(query, table)?.as_ref(),
        window.from,
        window.to,
        MAX_VALUES,
    )?;
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
}

#[cfg(test)]
mod tests {
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
}
