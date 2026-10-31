// Chart helpers shared by the pages that draw time series: what each
// column is called and in what units, and how a y axis is fitted to
// it.  Loaded after common.js.
"use strict";

// By `table.column`: the name, the factor that puts the column in units
// worth reading, and an optional qualifier said beside the name in the
// chart's title only: the menu and the cursor readout use the bare
// name.  Offsets are stored in seconds and are microseconds: plotted as
// seconds, every axis tick rounds to 0.
const LABELS = {
  "tracking.last_offset": ["Last offset, µs", 1e6],
  "tracking.current_correction": ["Current correction, µs", 1e6],
  "tracking.rms_offset": ["RMS offset, µs", 1e6],
  "tracking.frequency": ["Frequency, ppm", 1, "positive is fast"],
  "tracking.residual_frequency": ["Residual frequency, ppm", 1],
  "tracking.skew": ["Skew, ppm", 1],
  "tracking.root_delay": ["Root delay, ms", 1e3],
  "tracking.root_dispersion": ["Root dispersion, ms", 1e3],
  "tracking.last_update_interval": ["Update interval, s", 1],
  "tracking.stratum": ["Stratum", 1],
  "source_sample.latest_measurement": ["Offset, µs", 1e6],
  "source_sample.original_latest_measurement": ["Offset as measured, µs", 1e6],
  "source_sample.latest_measurement_error": ["Offset error bound, µs", 1e6],
  "source_sample.poll": ["Poll, log₂ s", 1],
  "source_sample.stratum": ["Stratum", 1],
  "source_sample.reachability": ["Reachability register", 1],
  "source_stats.estimated_offset": ["Estimated offset, µs", 1e6],
  "source_stats.estimated_offset_error": ["Estimated offset error, µs", 1e6],
  "source_stats.standard_deviation": ["Standard deviation, µs", 1e6],
  "source_stats.residual_frequency": ["Frequency difference, ppm", 1],
  "source_stats.skew": ["Skew, ppm", 1],
  "source_stats.samples": ["Samples", 1],
  "source_stats.runs": ["Runs", 1],
  "source_stats.span": ["Span, s", 1],
  "ntp_sample.offset": ["Offset, µs", 1e6],
  "ntp_sample.peer_delay": ["Delay, ms", 1e3],
  "ntp_sample.peer_dispersion": ["Dispersion, µs", 1e6],
  "ntp_sample.response_time": ["Response time, µs", 1e6],
  "ntp_sample.root_delay": ["Root delay, ms", 1e3],
  "ntp_sample.root_dispersion": ["Root dispersion, ms", 1e3],
  "ntp_sample.poll": ["Poll, log₂ s", 1],
  "ntp_sample.stratum": ["Stratum", 1],
  "selection.score": ["Selection score", 1, "the selected source's is 1"],
  "selection.low_limit": ["Offset interval low, µs", 1e6],
  "selection.high_limit": ["Offset interval high, µs", 1e6],
  // chronyd's logs.  tracking.log's frequency and skew are in ppm, with
  // the same sign as the protocol's (both from LCL_ReadAbsoluteFrequency
  // in reference.c); statistics.log's are fractions.
  "log_tracking.frequency": ["Frequency, ppm", 1],
  "log_tracking.skew": ["Skew, ppm", 1],
  "log_tracking.offset": ["Offset, µs", 1e6],
  "log_tracking.offset_sd": ["Offset standard deviation, µs", 1e6],
  "log_tracking.remaining_correction": ["Remaining correction, µs", 1e6],
  "log_tracking.root_delay": ["Root delay, ms", 1e3],
  "log_tracking.root_dispersion": ["Root dispersion, ms", 1e3],
  "log_tracking.max_error": ["Maximum error, ms", 1e3],
  "log_statistics.residual_frequency": ["Frequency difference, ppm", 1e6],
  "log_statistics.skew": ["Skew, ppm", 1e6],
  "log_statistics.estimated_offset": ["Estimated offset, µs", 1e6],
  "log_statistics.standard_deviation": ["Standard deviation, µs", 1e6],
  "log_measurement.offset": ["Offset, µs", 1e6],
  "log_measurement.peer_delay": ["Delay, ms", 1e3],
  "refclock_sample.cooked_offset": ["Offset, µs", 1e6],
  "refclock_sample.raw_offset": ["Raw offset, ms", 1e3],
  "refclock_sample.dispersion": ["Dispersion, µs", 1e6],
  // tempcomp reads its sensor file as it is, so the unit is the
  // sensor's: a thermal zone gives millidegrees Celsius.
  "tempcomp_sample.temperature": ["Temperature, as the sensor reads it", 1],
  "tempcomp_sample.compensation": ["Compensation, ppm", 1],
};

// Hover text by `table.column`: what each is, from chronyc's
// documentation (chronyc.adoc), which a name and a unit cannot carry.
const HINTS = {
  "tracking.last_offset": "The offset measured on the last clock update (chronyc tracking, Last offset).",
  "tracking.rms_offset": "A long-term average of the offset (chronyc tracking, RMS offset).",
  "tracking.frequency":
    "The rate at which the system clock would be wrong without chronyd's correction (chronyc tracking).",
  "tracking.residual_frequency":
    "The selected source's frequency less the current one; nonzero while chronyd is still converging.",
  "tracking.skew": "The estimated error bound on the frequency.",
  "tracking.root_delay": "Total network path delay to the stratum-1 at the root.",
  "tracking.root_dispersion": "Total dispersion accumulated to the stratum-1 at the root.",
  "source_stats.residual_frequency":
    "The source's frequency relative to the local clock, from chronyd's regression (chronyc sourcestats, Freq).",
  "source_stats.skew": "The error bound on that frequency (chronyc sourcestats, Freq Skew).",
  "source_stats.estimated_offset": "The source's offset as the regression estimates it (chronyc sourcestats).",
  "source_sample.latest_measurement":
    "The offset of the source's sample, positive when the local clock is ahead (chronyc sources, Last sample).",
  "ntp_sample.offset": "The offset of the source's last valid packet (chronyc ntpdata).",
  "ntp_sample.peer_delay": "The round-trip delay of the source's last valid packet.",
  "ntp_sample.peer_dispersion": "The dispersion of the source's last valid packet.",
  "refclock_sample.cooked_offset":
    "The sample's offset after the refclock's offset and delay corrections (refclocks.log, Cooked).",
  "refclock_sample.raw_offset":
    "The offset the driver measured, before corrections; empty in a filtered sample (refclocks.log, Raw).",
  "refclock_sample.dispersion": "The sample's dispersion (refclocks.log, Disp.).",
  "tempcomp_sample.temperature":
    "The value read from tempcomp's sensor file, in its own unit (tempcomp.log, Temp.).",
  "tempcomp_sample.compensation":
    "The frequency correction tempcomp applied for that temperature (tempcomp.log, Comp.).",
};

// Columns whose axis is linear near zero and logarithmic beyond: an
// offset of a microsecond and a spike of a millisecond are both read on
// one chart.  uPlot's `asinh` scale (distr 4) does it.
const ASINH_COLUMNS = new Set([
  "tracking.last_offset",
  "tracking.current_correction",
  "source_sample.latest_measurement",
  "source_sample.original_latest_measurement",
  "source_stats.estimated_offset",
  "ntp_sample.offset",
  "refclock_sample.cooked_offset",
]);

const key = (table, column) => `${table}.${column}`;
const columnName = (k) => (LABELS[k] ?? [k.split(".").at(-1), 1])[0];
const label = (k) => {
  const qualifier = LABELS[k]?.[2];
  return qualifier ? `${columnName(k)} (${qualifier})` : columnName(k);
};
const factor = (k) => (LABELS[k] ?? [k, 1])[1];
const scaled = (values, k) =>
  factor(k) === 1 ? values : values.map((v) => (v === null || v === undefined ? v : v * factor(k)));

// The hover text for the columns a chart draws, each once.
const hintText = (...keys) => [...new Set(keys)].map((k) => HINTS[k]).filter(Boolean).join(" ");

// Give a chart's title the hover text for its columns, underlined so it
// is found.
const hint = (chart, ...keys) => hintAs(chart, hintText(...keys));

function hintAs(chart, text) {
  const heading = chart.root.querySelector(".u-title");
  if (!text || !heading) return;
  heading.title = text;
  heading.classList.add("hinted");
}

// What a chart's title says of the readings it leaves out of what it
// `shows`: how many lie below it and how many above.
const beyondWords = (below, above, shows) => `${below} below and ${above} above ${shows}`;

// Points to ask for: enough that a long window is not thinned to
// half-hour bins, bounded by what the server will give.
function pointsFor(seconds) {
  if (!seconds) return 5000;
  return Math.min(5000, Math.max(1500, Math.round(seconds / 30)));
}

// A y scale that begins and ends on a labeled gridline.
//
// uPlot scales to the data and then places round ticks inside it, so
// the top and bottom lines fall short by whatever the data happens to
// need -- by a different amount on every plot, which across a stack
// reads as ragged.  Snapping the scale itself to the tick step fixes
// it, at the cost of some empty space above and below the trace.
//
// Steps are 1, 2, 5 or 10 times a power of ten, so the labels are
// numbers a person would choose.
const TICK_COUNT_WEIGHT = 0.3;

// `room`, as `mostTicks` gives it, caps the gaps between labels, unless
// no round step fits under it.
function niceScale(min, max, target, room) {
  if (!isFinite(min) || !isFinite(max)) return null;
  // A series that never moved still needs an axis with room around it.
  // "Never moved" cannot be exact equality: averaging the same number
  // in floating point does not give it back exactly.
  const noise = Math.max(Math.abs(min), Math.abs(max)) * 1e-9;
  if (max - min <= noise) {
    const pad = Math.max(Math.abs(min) * 0.05, 1);
    min -= pad;
    max += pad;
  }
  const span = max - min;
  const aim = Math.min(target, room);
  const mag = Math.pow(10, Math.floor(Math.log10(span / aim)));
  // Under a cap, a step a decade up as well: a span just under a power
  // of ten otherwise has no round step that few ticks cover.
  return roundScale(min, max, span, mag, aim, Math.min(room, MOST_TICKS), [1, 2, 5, 10, 20, 50]) ??
    roundScale(min, max, span, mag, aim, MOST_TICKS, [1, 2, 5, 10]);
}

// Two lines is not an axis; nine is a grid.
const MOST_TICKS = 8;

// The best step of `multiples` of `mag` over `min` to `max`, with at
// most `most` gaps, as `niceScale` scores them; null for none.
function roundScale(min, max, span, mag, target, most, multiples) {
  let best = null;
  for (const m of multiples) {
    const step = m * mag;
    const lo = Math.floor(min / step) * step;
    const hi = Math.ceil(max / step) * step;
    const n = Math.round((hi - lo) / step);
    if (n < 2 || n > most) continue;
    // Both matter and the balance is empirical: weighting the tick
    // count heavily gives round axes that use less than half the plot.
    const score = Math.abs(n - target) * TICK_COUNT_WEIGHT + (hi - lo - span) / span;
    if (!best || score < best.score) best = { lo, hi, step, n, score };
  }
  return best;
}

// How many gaps between labels suit a plot of this height.  About
// forty pixels each: closer and the labels crowd, further and a short
// plot ends up with two.
function tickTarget(height) {
  return Math.min(6, Math.max(3, Math.round(height / 40)));
}

// The least room a y label needs, in CSS pixels, with the gap to the
// next: closer and they overprint.
const LABEL_ROOM = 24;

// The most gaps between labels a stack chart of this height has room
// for, on the plot inside its ends.
const mostTicks = (height) => Math.max(2, Math.floor((height - 2 * STACK_END) / LABEL_ROOM));

// How tall each chart of a stack is, given how many there are, less
// the time axis under the bottom one.  One gets the room it always had;
// a stack divides a similar budget, with a floor below which a plot
// stops being readable.
function plotHeight(count) {
  return count === 1 ? 420 : Math.max(150, Math.round(720 / count));
}

// One tick label, at the precision its step implies.  Fixed places
// rather than toLocaleString's default, or a step of 0.05 renders as
// 35.9 and 36 and the axis looks unevenly spaced when it is not.
function tickLabel(v, places) {
  return v.toLocaleString(undefined, {
    minimumFractionDigits: places,
    maximumFractionDigits: places,
  });
}

// A y axis for `values` of column `k` on a plot `height` pixels tall:
// the range its scale is snapped to, and the axis's ticks, labels and
// width to match, so the lines and the range cannot disagree.  Both
// empty when there is nothing to fit.
function yAxis(k, values, height) {
  if (ASINH_COLUMNS.has(k)) {
    const asinh = asinhAxis(values, height);
    if (asinh) return asinh;
  }
  return linearAxis(values, height);
}

// A linear y axis, in the shape `yAxis` returns.
function linearAxis(values, height) {
  const nice = !values.length
    ? null
    : niceScale(Math.min(...values), Math.max(...values), tickTarget(height), mostTicks(height));
  const places = nice ? Math.max(0, -Math.floor(Math.log10(nice.step))) : 0;
  const ticks = [];
  if (nice) {
    for (let i = 0; i <= nice.n; i++) ticks.push(nice.lo + i * nice.step);
  }
  // Wide enough for the longest label this axis will actually draw.
  // Measured from the ticks rather than from the data, since a tick
  // can be wider than every value under it, and the default gutter
  // silently clips instead of overflowing.
  const widest = Math.max(
    ...(ticks.length ? ticks : [0]).map((v) => tickLabel(v, places).length),
    4,
  );
  return {
    axis: {
      size: 30 + widest * 8,
      ...(nice
        ? {
            splits: () => ticks,
            values: (u, vals) => vals.map((v) => tickLabel(v, places)),
          }
        : {}),
    },
    range: nice ? [nice.lo, nice.hi] : null,
  };
}

// The linear part of an asinh axis spans a 1, 2 or 5 times a power of
// ten at or above the median magnitude: the usual wander is read on a
// linear scale and only the excursions are compressed.
function linearThreshold(values) {
  const magnitudes = values.map(Math.abs).filter((v) => v > 0).sort((a, b) => a - b);
  if (!magnitudes.length) return 1;
  const median = magnitudes[Math.floor(magnitudes.length / 2)];
  const e = Math.floor(Math.log10(median));
  const m = median / 10 ** e;
  return [1, 2, 5, 10].find((s) => s >= m) * 10 ** e;
}

// How far past its linear threshold the data must reach for an asinh
// axis to be worth it: within ten times it, a linear axis reads better.
const ASINH_REACH = 10;

// An axis for `values` on uPlot's asinh scale, in the shape `yAxis`
// returns, or null when the data stays near its threshold.  Its ticks
// are zero, the threshold, and decades of it beyond, on each side the
// data reaches: uPlot's own asinh splits crowd the linear part with
// grid lines.
function asinhAxis(values, height) {
  if (!values.length) return null;
  const threshold = linearThreshold(values);
  const lo = Math.min(...values), hi = Math.max(...values);
  if (Math.max(Math.abs(lo), Math.abs(hi)) <= ASINH_REACH * threshold) return null;
  // The decades out to the first at or beyond `reach`.
  const out = (reach) => {
    const ticks = [];
    for (let t = threshold; ; t *= 10) {
      ticks.push(t);
      if (t >= reach) break;
    }
    return ticks;
  };
  const above = hi > 0 ? out(hi) : [];
  const below = lo < 0 ? out(-lo) : [];
  // As many as the plot has room for: every second or third decade when
  // a step of seconds shares an axis with nanoseconds, counted from the
  // outermost, which each side keeps since the axis ends there.  Thinned,
  // the threshold itself goes: compressed, it sits too near zero to
  // label.
  const every = Math.ceil((above.length + below.length) / Math.max(1, mostTicks(height) - 1));
  const thin = (side) =>
    side.filter((_, i) => (side.length - 1 - i) % every === 0 && (every === 1 || i > 0 || side.length === 1));
  const ends = [below.length ? -below.at(-1) : 0, above.length ? above.at(-1) : 0];
  const ticks = [
    ...clearOfZero(thin(below), threshold, ends, height).map((t) => -t).reverse(),
    0,
    ...clearOfZero(thin(above), threshold, ends, height),
  ];
  const format = (v) => (v == null ? null : v.toLocaleString(undefined, { maximumSignificantDigits: 3 }));
  const widest = Math.max(4, ...ticks.map((v) => format(v).length));
  return {
    axis: {
      size: 30 + widest * 8,
      splits: () => ticks,
      // uPlot keeps only powers of ten on a log or asinh axis; these
      // ticks are decades of the threshold, which need not be one.
      filter: (u, splits) => splits,
      values: (u, splits) => splits.map(format),
    },
    range: ends,
    scale: { distr: 4, asinh: threshold },
  };
}

// Of one side's ticks of an asinh axis, outward from zero, those whose
// labels clear zero's and one another's on a plot `height` tall from
// `ends[0]` to `ends[1]`, kept from the outermost in.  Thinned by count
// alone, the threshold can still sit a pixel or two from zero when the
// axis reaches seconds beside a threshold of nanoseconds.
function clearOfZero(side, threshold, [lo, hi], height) {
  const at = (v) => Math.asinh(v / threshold);
  const px = (height - 2 * STACK_END) / (at(hi) - at(lo));
  const kept = [];
  for (const t of [...side].reverse()) {
    if (at(t) * px >= LABEL_ROOM && (!kept.length || (at(kept.at(-1)) - at(t)) * px >= LABEL_ROOM)) kept.push(t);
  }
  return kept.reverse();
}

// ------------------------------------------------------------- clipping
//
// A y axis fitted to the bulk of what a chart draws, on a display no one
// zooms: a source's first samples after chronyd restarts, or a step of
// seconds after a boot, beside microseconds of wander leave the wander a
// hairline, asinh axis or not.  What lies beyond is drawn at the axis's
// edge, marked, and counted in the title.

// The percentiles the bulk lies between: those the Distribution page's
// histogram shows (HISTOGRAM_FROM and HISTOGRAM_TO in api.rs).
const BULK_FROM = 0.5;
const BULK_TO = 99.5;
// How far past the bulk a value is still drawn to scale, in spans of
// the bulk: a normal spread's 0.5% and 99.5% are 2.6 sigma out, so half
// its span again either side reaches 5 sigma, past a day of buckets'
// extremes.  Only what lies beyond is an outlier.
const BULK_MARGIN = 0.5;
// The marker at the axis's edge where a line goes past it, a triangle
// pointing off the plot: its height and half its base, in CSS pixels.
const OFF_SCALE_MARK = 5;

// The `p`th percentile of sorted `values`, by linear interpolation
// between the nearest ranks, as `percentile` in api.rs.
function percentile(sorted, p) {
  const rank = (p / 100) * (sorted.length - 1);
  const below = Math.floor(rank);
  const above = Math.min(below + 1, sorted.length - 1);
  return sorted[below] + (sorted[above] - sorted[below]) * (rank - below);
}

// The values a clipped axis is drawn to scale over, `[lo, hi]`: the
// bulk of `values` and its margin.  Null when every value is within
// them, and the axis need not be clipped.
function bulkOf(values) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const from = percentile(sorted, BULK_FROM), to = percentile(sorted, BULK_TO);
  const margin = (to - from) * BULK_MARGIN;
  const bulk = [from - margin, to + margin];
  return sorted[0] < bulk[0] || sorted.at(-1) > bulk[1] ? bulk : null;
}

// What of `drawing` lies off its y axis `[lo, hi]`, its lines not
// `hidden`: how many of a line's buckets, its envelope's included, go
// below and above, and a mark `{ at, side, color }` for each, `at` the
// bucket's index and `side` -1 below, 1 above.  Null for none.
function offScale(drawing, hidden, [lo, hi]) {
  const marks = [];
  drawing.lines.forEach((line, n) => {
    if (hidden(line)) return;
    const arrays = [line.values, ...(n === 0 && drawing.envelope ? [drawing.envelope.min, drawing.envelope.max] : [])];
    line.values.forEach((_, i) => {
      const at = arrays.map((a) => a[i]).filter((v) => v != null);
      if (at.some((v) => v < lo)) marks.push({ at: i, side: -1, color: line.color });
      if (at.some((v) => v > hi)) marks.push({ at: i, side: 1, color: line.color });
    });
  });
  if (!marks.length) return null;
  return {
    below: marks.filter((m) => m.side < 0).length,
    above: marks.filter((m) => m.side > 0).length,
    marks,
  };
}

// A y axis for `drawing` as `yAxis` fits one to `values`, clipped to
// their bulk when `clip` and anything lies beyond it, with `off`, as
// `offScale` gives it, for what is drawn at its edge.
function clippedAxis(drawing, values, height, hidden, clip) {
  const bulk = clip ? bulkOf(values) : null;
  if (!bulk) return yAxis(drawing.column, values, height);
  const fit = yAxis(drawing.column, values.filter((v) => v >= bulk[0] && v <= bulk[1]), height);
  const off = fit.range ? offScale(drawing, hidden, fit.range) : null;
  return off ? { ...fit, off } : fit;
}

// `values` held inside a clipped axis's range, drawn at its edge.
const pinned = (values, fit) =>
  fit.off ? values.map((v) => (v == null ? v : Math.min(Math.max(v, fit.range[0]), fit.range[1]))) : values;

// What a chart's title says of the buckets drawn at its y axis's edge,
// after what it names: nothing when there are none.
function offScaleTitle(chart, fit) {
  const heading = chart.root.querySelector(".u-title");
  if (!heading) return;
  let note = heading.querySelector(".off-scale");
  if (!fit.off) {
    note?.remove();
    return;
  }
  if (!note) {
    note = document.createElement("span");
    note.className = "muted off-scale";
    heading.appendChild(note);
  }
  update(note, "textContent", `; ${beyondWords(fit.off.below, fit.off.above, "the axis")}`);
}

// A marker at the y axis's edge for each bucket drawn there, in its
// line's color.
function drawOffScale(u) {
  const off = u.chronymonAxis?.fit.off;
  if (!off) return;
  const { ctx } = u;
  const size = OFF_SCALE_MARK * devicePixelRatio;
  ctx.save();
  for (const { at, side, color } of off.marks) {
    const x = u.valToPos(u.data[0][at], "x", true);
    const edge = side < 0 ? u.bbox.top + u.bbox.height : u.bbox.top;
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(x, edge);
    ctx.lineTo(x - size, edge + side * size);
    ctx.lineTo(x + size, edge + side * size);
    ctx.closePath();
    ctx.fill();
  }
  ctx.restore();
}

// Charts in a stack share a time axis only if their plotting areas line
// up: each gets the widest y axis of the stack, and the same right
// padding, wide enough for the bottom chart's last time label, which
// uPlot would otherwise make room for on that chart alone.
const STACK_RIGHT = 32;

// The colors every chart's axes are drawn in: the labels, then the grid
// and ticks, recessive so the readings are seen first.
const AXIS_INK = "#8b929c";
const AXIS_RULE = "#2b3038";
// An axis as every chart draws one.
const AXIS = { stroke: AXIS_INK, grid: { stroke: AXIS_RULE }, ticks: { stroke: AXIS_RULE } };
// uPlot's own time axis height, for its own font.
const TIME_SIZE = 50;

// The room above and below a stack chart's plot, for half its top and
// bottom y labels: uPlot's own default, a third of its time axis.
// Under the bottom chart the time axis takes its place.
const STACK_END = Math.round(TIME_SIZE / 3);

// The height of chart `n` of a stack of `count`, each given `height`,
// its axes `scale` times their own size.  The bottom chart's time axis
// is drawn inside its height, so it is given the axis's room on top:
// otherwise its plot is that much shorter than the others'.
const stackChartHeight = (height, n, count, scale = 1) =>
  height + (n === count - 1 ? (TIME_SIZE - STACK_END) * scale : 0);

// The options every chart in a stack of time charts shares:
//   width, height  its size, as `stackChartHeight` gives the height
//   title          its title
//   series         its series after the time
//   fitted, ySize  its y axis as `yAxis` fitted it, and the stack's
//                  shared y axis width
//   last           whether it is the bottom chart, the one that
//                  carries the time labels
//   yStroke        the y axis labels' color
//   legend, hooks  uPlot's, the hooks added to the drag that sets
//                  the range
//   font, scale    optional: the axes' font, as uPlot takes it, and how
//                  many times their own size its room is
//   span()         the time axis's `{ from, to }`, the read's: each
//                  chart fitted to its own times would put one time at
//                  different places down the stack, and drop a stretch
//                  missing at either end
function stackOptions(stack) {
  const scale = stack.scale ?? 1;
  const font = stack.font ? { font: stack.font } : {};
  return {
    width: stack.width,
    height: stack.height,
    padding: [STACK_END * scale, STACK_RIGHT * scale, stack.last ? 0 : STACK_END * scale, null],
    title: stack.title,
    series: [{}, ...stack.series],
    axes: [
      { ...AXIS, ...font, size: TIME_SIZE * scale, show: stack.last },
      {
        ...AXIS,
        ...font,
        ...(stack.yStroke ? { stroke: stack.yStroke } : {}),
        ...stack.fitted.axis,
        size: stack.ySize,
      },
    ],
    scales: {
      x: { time: true, range: () => [stack.span().from, stack.span().to] },
      ...(stack.fitted.range || stack.fitted.scale
        ? { y: { ...stack.fitted.scale, ...(stack.fitted.range ? { range: stack.fitted.range } : {}) } }
        : {}),
    },
    legend: stack.legend ?? { show: true, live: true },
    cursor: { drag: { x: true, y: false }, sync: { key: stack.sync, setSeries: false } },
    hooks: { ...stack.hooks, ...TIME_HOOKS },
  };
}

// Give every chart in a stack the width its container has settled at.
// Each was made at the width the container had then, and adding charts
// can bring on a scrollbar that narrows it partway down the stack.
function settleWidths(charts, container) {
  const width = container.clientWidth;
  for (const c of charts) {
    if (width && c.width !== width) c.setSize({ width, height: c.height });
  }
}

// Hold a chart container at the height it has now, while its charts are
// drawn again, until `release`.  Called before they are taken down:
// emptied, the page would be too short for the place it was scrolled
// to, and the browser would put it back at the top.
function hold(container) {
  container.style.minHeight = `${container.offsetHeight}px`;
}

// Let a container held by `hold` take its own height again.
function release(container) {
  container.style.minHeight = "";
}

// The y axis width a stack of fitted axes shares.
const stackWidth = (fits) => Math.max(...fits.map((f) => f.axis.size ?? 50));

// Tie a time chart to the range: a drag fixes the range to the stretch
// it covered, a double click zooms out, as in Grafana.  The drag is
// read from setSelect, not setScale: setScale also fires when uPlot
// fits the scale to new data, on every draw.
const TIME_HOOKS = {
  setSelect: [
    (u) => {
      if (u.select.width <= 0) return;
      fixRange(u.posToVal(u.select.left, "x"), u.posToVal(u.select.left + u.select.width, "x"));
    },
  ],
};
function ranged(chart) {
  chart.over.addEventListener("dblclick", () => zoomOut());
  return chart;
}

// ---------------------------------------------------------------- stack
//
// A stack of time charts, a chart per column sharing one time axis.

// The trace colors, named once.  Both the plot and the things that say
// which line is which read them from here, because a key that disagrees
// with the lines is worse than no key.
const TRACE = "#38bdf8";
const MATE = "#fbbf24";
// One color per source on the overlay charts, none of them an event's.
const SOURCE_COLORS = ["#38bdf8", "#fbbf24", "#a78bfa", "#2dd4bf", "#fb923c", "#e2e8f0", "#84cc16", "#f472b6"];
// The envelope behind a line: each bucket's lowest and highest.
const ENVELOPE = "#0e7490";

// `/api/history` for `columns` of `table` over the range, for `source`
// when given.
// `fill`, when given, is `{ table, columns }`: chronyd's log, drawn
// where the daemon was not recording.
function historyQuery(table, columns, bounds, source, fill) {
  const q = new URLSearchParams({ table, columns: columns.join(","), points: pointsFor(bounds.seconds) });
  setBounds(q, bounds);
  if (source != null) q.set("source", source);
  if (fill) {
    q.set("fill", fill.table);
    q.set("fill_columns", fill.columns.join(","));
  }
  return "/api/history?" + q;
}

// `/api/overlay` for `[table, column]` of every source over `bounds`.
function overlayQuery([table, column], bounds) {
  const q = new URLSearchParams({ table, column, points: pointsFor(bounds.seconds) });
  setBounds(q, bounds);
  return "/api/overlay?" + q;
}

// Bounds both ends of which the server chooses: the latest hour.
const OPEN_BOUNDS = { from: null, to: null, seconds: null };

// Reads over the range, `read(bounds)` each, for charts that share a
// time axis: each over the same bounds.  For `all`, first over the
// server's latest hour, which is cheap and says each table's extent,
// then over the extent of them all: how far back the database goes is
// only knowable once it has been asked.
async function readTogether(reads) {
  if (range.last !== "all") {
    const bounds = rangeBounds();
    return Promise.all(reads.map((read) => read(bounds)));
  }
  const probe = await Promise.all(reads.map((read) => read(OPEN_BOUNDS)));
  const first = Math.min(...probe.map((got) => got.first ?? Infinity));
  const last = Math.max(...probe.map((got) => got.last ?? -Infinity));
  if (!(last > first)) return probe;
  const all = { from: first, to: last, seconds: last - first };
  return Promise.all(reads.map((read) => read(all)));
}

// `/api/history`, as `historyQuery` asks it, over the range.  `bounds`,
// when given, are used as they are, as a second read on the grid of a
// first one must be.
async function readHistory(signal, table, columns, source, fill, bounds) {
  const read = (b) => ask(historyQuery(table, columns, b, source, fill), signal, HISTORY_TIMEOUT);
  return bounds ? read(bounds) : (await readTogether([read]))[0];
}

// A read's own bounds, for a second read on its grid.
const boundsOf = (got) => ({ from: got.from, to: got.to, seconds: got.to - got.from });

// The time axis of charts drawn from `reads`: the bounds the server read
// them over.
const spanOf = (reads) => ({
  from: Math.min(...reads.map((got) => got.from)),
  to: Math.max(...reads.map((got) => got.to)),
});

// How a line from chronyd's log is drawn beside the protocol's: dimmer
// and dashed, since it is to the second and four significant digits.
const LOGGED = "#7dd3fc80";
const LOGGED_DASH = [5, 4];
const LOGGED_NAME = "from chronyd's log";

// The drawings of a filled read of `table`: a chart per column of
// `table`, with the log's column `fills[column]` beside it as a second,
// dashed line where it has anything to draw.
function filledCharts(table, got, fills) {
  return got.plots
    .filter((p) => p.table === table)
    .map((plot) => {
      const k = key(table, plot.column);
      const logged = got.plots.find((p) => p.table !== table && p.column === fills[plot.column]);
      const lines = [{ column: k, values: scaled(spanned(plot.mean, plot.gaps), k), color: TRACE, name: "reported" }];
      if (logged && logged.mean.some((v) => v != null)) {
        lines.push({
          column: k,
          values: scaled(spanned(logged.mean, logged.gaps), k),
          color: LOGGED,
          dash: LOGGED_DASH,
          name: LOGGED_NAME,
        });
      }
      return {
        column: k,
        title: label(k),
        envelope: envelopeOf(plot, k),
        lines: lines.length > 1 ? lines : [{ ...lines[0], name: undefined }],
      };
    });
}

// The drawings of a stack from the server's plots of `table`: a chart
// per column, each
//   column    its key, which its y axis is fitted to and its hover
//             text is for
//   envelope  { min, max }, drawn behind the line, or null
//   lines     [{ column, values, color, name }] on one y axis
// and, given by a page, optionally
//   heading   its title as HTML, in place of one naming its lines
function chartsOf(table, plots) {
  return plots.map((plot) => {
    const k = key(table, plot.column);
    return {
      column: k,
      envelope: envelopeOf(plot, k),
      lines: [{ column: k, values: scaled(spanned(plot.mean, plot.gaps), k), color: TRACE }],
    };
  });
}

// A line's values on a grid it shares: a bucket it has no row in is
// `undefined`, which uPlot draws across, and only its `gaps` `null`,
// which uPlot breaks at.  A source polled every 64 s beside a refclock
// polled every 16 s has nothing in most of an overlay's buckets; the
// clock, written only when it updates, nothing in most of a long
// range's while chronyd's log fills around it.
function spanned(values, gaps) {
  const out = values.map((v) => (v === null ? undefined : v));
  for (const gap of gaps) out[gap] = null;
  return out;
}

// A plot's envelope, its each bucket's lowest and highest, spanned as
// its line is.
const envelopeOf = (plot, k) => ({
  min: scaled(spanned(plot.min, plot.gaps), k),
  max: scaled(spanned(plot.max, plot.gaps), k),
});

// A drawing of `overlay`, one column of every source, a line each in
// the source's color.  `names` maps a source ID to its name.
function overlayChart(table, column, overlay, names) {
  const k = key(table, column);
  return {
    column: k,
    envelope: null,
    x: overlay.at,
    title: label(k),
    lines: overlay.lines.map((line) => ({
      column: k,
      name: names(line.source_id),
      values: scaled(spanned(line.mean, line.gaps), k),
      color: sourceColor(line.source_id),
    })),
  };
}

// A source's color, the same on every chart and page.
const sourceColor = (id) => SOURCE_COLORS[(Number(id) - 1) % SOURCE_COLORS.length];

// A multi-line chart's title, each line's own words in its color, so
// the title says which line is which with the legend off.  On a stack
// whose lines can be hidden, each name is a button that hides or shows
// it, as `lineButton` draws it.
function groupTitle(drawing, toggles) {
  const lines = drawing.lines;
  return (
    esc(drawing.title ?? label(lines[0].column)) +
    '<span class="muted">: </span>' +
    lines
      .map((l) => toggles && l.name
        ? lineButton(l)
        : `<span style="color:${l.color}">${esc(l.name ?? columnName(l.column))}</span>`)
      .join('<span class="muted">, </span>')
  );
}

// ------------------------------------------------------------- hiding
//
// A stack's lines, a source each, hidden by name on every chart of it,
// where the stack allows: one source wandering milliseconds, as an NMEA
// receiver does, otherwise sets the axes of every chart it is on and
// flattens the rest.  Some names a page hides unless asked, as Sources
// does a refclock a PPS is locked to.  The names hidden ride in the
// address, `hide=`, and those hidden unless asked that are shown,
// `show=`.

// The address keys, and the names each lists.
const HIDE_KEY = "hide";
const SHOW_KEY = "show";
const namesIn = (k) => new Set((new URLSearchParams(location.search).get(k) ?? "").split(",").filter(Boolean));
const hiddenNames = namesIn(HIDE_KEY);
const shownNames = namesIn(SHOW_KEY);
// The names hidden unless shown, as the page last said.
let hiddenUnlessShown = new Set();

// Hide `names` unless the address shows them.
function hideUnlessShown(names) {
  hiddenUnlessShown = new Set(names);
}

// Whether the line named `name` is hidden.
const nameHidden = (name) => (hiddenUnlessShown.has(name) ? !shownNames.has(name) : hiddenNames.has(name));

// Whether `line` is hidden, on a stack whose lines can be.
const hiddenOn = (toggles) => (line) => Boolean(toggles && line.name && nameHidden(line.name));

// The words a line's name button says it does.
const toggleWords = (name, hidden) => `${hidden ? "show" : "hide"} ${name} on every chart`;

// A line's name, in its color, as a button pressed while it is shown.
const lineButton = (l) => {
  const hidden = nameHidden(l.name);
  return `<button type="button" class="line-name" data-line="${esc(l.name)}" style="color:${l.color}"` +
    ` aria-pressed="${!hidden}" title="${esc(toggleWords(l.name, hidden))}">${esc(l.name)}</button>`;
};

// Bring `chart`'s lines and its title's buttons up to the names hidden.
function showLines(chart, hidden) {
  for (const l of chart.chronymonLines) {
    const show = !hidden(l);
    if (chart.series[l.series].show !== show) chart.setSeries(l.series, { show });
  }
  for (const button of chart.root.querySelectorAll(".u-title button[data-line]")) {
    const name = button.dataset.line;
    button.setAttribute("aria-pressed", String(!nameHidden(name)));
    button.title = toggleWords(name, nameHidden(name));
  }
}

// Hide the line named `name` if it is shown, else show it, and say so
// in the address.
function toggleName(name) {
  const names = hiddenUnlessShown.has(name) ? shownNames : hiddenNames;
  if (!names.delete(name)) names.add(name);
  const listed = (set) => (set.size ? [...set].join(",") : null);
  remember({ [HIDE_KEY]: listed(hiddenNames), [SHOW_KEY]: listed(shownNames) });
}

// Let the names in `view`'s titles hide and show their lines, once: the
// stack is drawn again from its last read, its y axes fitted to what is
// left, and the button pressed keeps the focus.
function toggleLines(view) {
  if (view.toggling) return;
  view.toggling = true;
  $(view.container).addEventListener("click", (e) => {
    const button = e.target.closest("button[data-line]");
    if (!button || !view.read) return;
    const name = button.dataset.line;
    toggleName(name);
    const at = view.charts.findIndex((c) => c.root.contains(button));
    drawStack(view, view.read, view.why);
    view.charts[at]?.root.querySelector(`button[data-line="${CSS.escape(name)}"]`)?.focus();
  });
}

// Every value a drawing plots, its envelope's as well as its lines'.
const drawingArrays = (d) => [
  ...(d.envelope ? [d.envelope.min, d.envelope.max] : []),
  ...d.lines.map((l) => l.values),
];

// The values a drawing's y axis is fitted to: those of the lines not
// `hidden`.
const fittedArrays = (d, hidden) => [
  ...(d.envelope ? [d.envelope.min, d.envelope.max] : []),
  ...d.lines.filter((l) => !hidden(l)).map((l) => l.values),
];

// Whether a drawing has anything to plot.  One with no data has no y
// axis, which would shift its plot out of line with the rest of a stack.
const drawsAnything = (d) => drawingArrays(d).flat().some((v) => v != null && isFinite(v));

// Each chart's y axis, fitted to what it draws.  The scale has to cover
// the envelope, not just the lines: the min and max are drawn, and an
// axis that ends below them would clip the very excursions they exist
// to show.  A line hidden is not fitted to.  With `clip`, as
// `clippedAxis` clips it.
const stackFits = (read, height, hidden, clip) =>
  read.drawn.map((chart) =>
    clippedAxis(chart, fittedArrays(chart, hidden).flat().filter((v) => v != null && isFinite(v)), height, hidden,
      clip));

// What a chart of the stack is drawn as, less its data and its y axis's
// range, ticks and width: its lines, and the kind of y axis.  Two reads
// that give a chart the same shape can share it.
const stackShape = (drawing, fitted) =>
  JSON.stringify([
    drawing.lines.map((l) => [l.column, l.name, l.color]),
    drawing.heading ?? null,
    Boolean(drawing.envelope),
    drawing.shaded !== false,
    Boolean(fitted.range),
    Boolean(fitted.axis.splits),
    Boolean(fitted.axis.filter),
    fitted.scale ?? null,
  ]);

// A fitted y axis, as `yAxis` gives it, that reads its range, ticks and
// labels from `held.fit` each time the chart draws: `restack` refits a
// chart in place by changing `held`, without drawing it afresh.
function heldAxis(held) {
  const { fit } = held;
  return {
    axis: {
      ...(fit.axis.splits ? { splits: (...args) => held.fit.axis.splits(...args) } : {}),
      ...(fit.axis.values ? { values: (...args) => held.fit.axis.values(...args) } : {}),
      ...(fit.axis.filter ? { filter: (...args) => held.fit.axis.filter(...args) } : {}),
    },
    range: fit.range && (() => held.fit.range),
    scale: fit.scale,
  };
}

// Draw the stack `read` -- `{ x, span, drawn }`, `span` its time axis as
// `spanOf` gives it -- into `container`, and answer its charts.  A
// drawing with an `x` of its own is plotted against that rather than
// the stack's.  `style` is
//   height(count)  each chart's height, given how many there are
//   sync           the cursor's sync key
//   hooks          uPlot's, as `stackOptions` takes them
//   font, scale    optional, as `stackOptions` takes them
//   toggles        optional: whether the names in the titles hide and
//                  show their lines
//   clip           optional: whether each y axis is clipped to the bulk
//                  of what it draws, as `clippedAxis` clips it
//   bands          optional: the bands behind each chart's lines, as
//                  `drawBands` takes them, a function, read as drawn;
//                  none behind a drawing's whose `shaded` is false
function stackCharts(container, read, style) {
  const scale = style.scale ?? 1;
  const height = style.height(read.drawn.length);
  const hidden = hiddenOn(style.toggles);
  const fits = stackFits(read, height / scale, hidden, style.clip);
  const ySize = stackWidth(fits) * scale;
  const charts = read.drawn.map((drawing, n) => {
    const several = drawing.lines.length > 1;
    const host = document.createElement("div");
    container.appendChild(host);
    const held = { fit: fits[n], ySize, span: read.span };
    const opts = stackOptions({
      width: container.clientWidth || 800,
      height: stackChartHeight(height, n, read.drawn.length, scale),
      title: drawing.title ?? label(drawing.column),
      series: [
        ...(drawing.envelope
          ? [
              { label: "min", stroke: ENVELOPE, width: 1, points: { show: false } },
              { label: "max", stroke: ENVELOPE, width: 1, points: { show: false } },
            ]
          : []),
        ...drawing.lines.map((l) => ({
          label: l.name ?? columnName(l.column),
          stroke: l.color,
          width: 1.5,
          ...(l.dash ? { dash: l.dash } : {}),
          points: { show: false },
          spanGaps: false,
          show: !hidden(l),
        })),
      ],
      fitted: heldAxis(held),
      ySize: () => held.ySize,
      span: () => held.span,
      // Only the bottom chart carries the time labels.  Repeating them
      // under every plot costs the height that makes the stack worth
      // having, and says the axes might differ when the whole point is
      // that they cannot.
      last: n === read.drawn.length - 1,
      yStroke: several ? AXIS_INK : TRACE,
      // No per-chart legend.  With the cursor synced there is one
      // instant being pointed at, not one per plot, so the values
      // belong in one row, which the page builds.
      legend: { show: false },
      sync: style.sync,
      hooks: style.hooks,
      font: style.font,
      scale,
    });
    const chart = ranged(new uPlot(opts, stackData(read, drawing, fits[n]), host));
    chart.chronymonShape = stackShape(drawing, fits[n]);
    chart.chronymonAxis = held;
    // Set before uPlot first draws, which it does in a microtask.
    chart.chronymonBands = drawing.shaded === false || !style.bands ? () => [] : style.bands;
    // Which lines this chart draws, and from which series, so a shared
    // readout can label them without reaching back into the request
    // that built it.
    chart.chronymonLines = drawing.lines.map((l, i) => ({
      column: l.column,
      name: l.name,
      series: 1 + (drawing.envelope ? 2 : 0) + i,
      color: several ? l.color : null,
    }));
    hint(chart, ...drawing.lines.map((l) => l.column));
    // uPlot's title is plain text, and with the legend off a chart of
    // several lines otherwise names them without saying which is which.
    if (several || drawing.heading || style.toggles) {
      const heading = chart.root.querySelector(".u-title");
      if (heading) heading.innerHTML = drawing.heading ?? groupTitle(drawing, style.toggles);
    }
    offScaleTitle(chart, fits[n]);
    return chart;
  });
  settleWidths(charts, container);
  return charts;
}

// What a chart of the stack plots on its y axis `fit`: its time axis,
// then its series, held inside a clipped axis.
const stackData = (read, drawing, fit) =>
  [drawing.x ?? read.x, ...drawingArrays(drawing).map((values) => pinned(values, fit))];

// The stack `charts`, drawn in `container`, brought up to `read`: each
// chart given its new data, y axis and size in place while the stack
// keeps its shape -- the same charts and lines -- so a page read again
// on a timer neither flashes nor moves, and drawn afresh when it does
// not.  Answers the charts, the same ones or new.
function restack(container, charts, read, style) {
  const scale = style.scale ?? 1;
  const height = style.height(read.drawn.length);
  const hidden = hiddenOn(style.toggles);
  const fits = stackFits(read, height / scale, hidden, style.clip);
  const ySize = stackWidth(fits) * scale;
  const kept = charts.length === read.drawn.length &&
    charts.every((c, n) => c.chronymonShape === stackShape(read.drawn[n], fits[n]));
  if (kept) {
    charts.forEach((c, n) => {
      const held = c.chronymonAxis;
      const widened = held.ySize !== ySize;
      held.fit = fits[n];
      held.ySize = ySize;
      held.span = read.span;
      c.setData(stackData(read, read.drawn[n], fits[n]));
      offScaleTitle(c, fits[n]);
      showLines(c, hidden);
      const size = {
        width: container.clientWidth || c.width,
        height: stackChartHeight(height, n, charts.length, scale),
      };
      if (size.width !== c.width || size.height !== c.height) c.setSize(size);
      // A new width alone changes no scale, which is what uPlot measures
      // its axes again on.
      else if (widened) c.redraw(false, true);
    });
    return charts;
  }
  hold(container);
  for (const c of charts) c.destroy();
  container.innerHTML = "";
  const drawn = stackCharts(container, read, style);
  release(container);
  return drawn;
}

// One value per line at the instant the cursor is on, plus the time
// once, for a stack sharing one cursor: uPlot's legend is per chart.
// The same cells are drawn whether or not anything is under the
// pointer, with dashes for the values when nothing is, so the row does
// not move what is below it.  A line hidden has none.  `mark` is the
// text of an event near the cursor, or null.
function readoutHtml(charts, mark) {
  const idx = charts.map((c) => c.cursor.idx).find((i) => i != null);
  const show = (v) =>
    v == null ? "--" : v.toLocaleString(undefined, { maximumSignificantDigits: 6 });
  const under = charts.find((c) => c.cursor.idx != null) ?? charts[0];
  const at = idx == null ? null : new Date(under.data[0][idx] * 1000);
  // A named line is named with its chart's measure too, or a stack of
  // overlays reads as the same source names over again.
  const name = (l) => {
    const measure = columnName(l.column).split(",")[0];
    return l.name ? `${measure}, ${l.name}` : columnName(l.column);
  };
  const cells = charts.flatMap((c) =>
    c.chronymonLines.filter((l) => c.series[l.series].show).map((l) => {

      const v = c.cursor.idx == null ? null : c.data[l.series][c.cursor.idx];
      return `<span><span class="k"${l.color ? ` style="color:${l.color}"` : ""}>` +
        `${esc(name(l))}</span> <span class="v">${esc(show(v))}</span></span>`;
    }));
  return `<span class="v">${at ? esc(at.toLocaleTimeString()) : "--:--:--"}</span>` +
    cells.join("") +
    `<span class="note">${mark ? esc(mark) : "&nbsp;"}</span>`;
}

// ---------------------------------------------------------------- marks

// How near the cursor must be to an event's line, in CSS pixels, for its
// text to be shown.
const MARK_REACH = 6;

// Each event's line, dashed across the plot area at its time, in its
// own color.  `marks` is a list of `{ at, text, color }`, `at` in unix
// seconds.
function drawMarks(u, marks) {
  const [lo, hi] = [u.scales.x.min, u.scales.x.max];
  const { ctx } = u;
  ctx.save();
  ctx.lineWidth = devicePixelRatio;
  ctx.setLineDash([4 * devicePixelRatio, 4 * devicePixelRatio]);
  for (const { at, color } of marks) {
    if (at < lo || at > hi) continue;
    const x = Math.round(u.valToPos(at, "x", true));
    ctx.strokeStyle = color;
    ctx.beginPath();
    ctx.moveTo(x, u.bbox.top);
    ctx.lineTo(x, u.bbox.top + u.bbox.height);
    ctx.stroke();
  }
  ctx.restore();
}

// The text of the events in `marks` under the cursor, if any are within
// reach, and of the `bands` it is in.
function markAt(u, marks, bands) {
  if (u.cursor.left == null || u.cursor.left < 0) return null;
  const near = marks.filter(({ at }) => Math.abs(u.valToPos(at, "x") - u.cursor.left) <= MARK_REACH);
  const t = u.posToVal(u.cursor.left, "x");
  const within = bands.filter(({ from, to }) => from <= t && t <= to);
  const texts = [...near, ...within].map((m) => m.text);
  return texts.length ? texts.join("; ") : null;
}

// ---------------------------------------------------------------- bands

// A span chronyd was not synchronized to a source in, unsynchronized
// or on its local reference, shaded behind the lines in the red the
// strip says unsynchronized in, faintly, as the time axis's own state
// rather than an event at an instant.
const NOT_SYNCHRONIZED_BAND = "#f871711f";

// What a band says of chronyd, by the read's list of its spans.
const NOT_SYNCHRONIZED_TEXTS = {
  unsynchronized: `chronyd ${LEAP_NAMES.unsynchronized}`,
  local: "chronyd on its local reference",
};

// The spans a read of `tracking` says chronyd was not synchronized to
// a source in, as bands `{ from, to, text }`, unix seconds.
const notSynchronizedBands = (got) =>
  Object.entries(NOT_SYNCHRONIZED_TEXTS)
    .flatMap(([key, text]) => (got[key] ?? []).map(([from, to]) => ({ from, to, text })))
    .sort((a, b) => a.from - b.from);

// A PPS device's pulse missing or the device unreadable, and its pulse
// flapping, shaded as not synchronized is, flapping in the amber of a
// warning.
const PPS_MISSING_BAND = NOT_SYNCHRONIZED_BAND;
const PPS_FLAPPING_BAND = "#fbbf241f";

// The spans the PPS events journal, by the event starting one: the event
// ending it, what the band says, and its color.  The daemon journals no
// edge inside a flapping span, so the span is shaded whole.
const PPS_SPANS = {
  pps_lost: ["pps_back", "PPS missing", PPS_MISSING_BAND],
  pps_flapping: ["pps_steady", "PPS flapping", PPS_FLAPPING_BAND],
  pps_down: ["pps_up", "PPS device unreadable", PPS_MISSING_BAND],
};

// The device a PPS event is of: its detail's first word, `pps2` of
// `pps2: 3 edges missed`.
const ppsDevice = (e) => (e.detail ?? "").split(":")[0];

// The spans `events`, oldest first, journal of PPS devices, as bands
// `{ from, to, text, color }` over `from` to `to`: one ending in the
// range but begun before it starts at `from`; one not ended, at `to`.
// A device going unreadable ends its other spans, as the daemon stops
// counting them.
function ppsBands(events, from, to) {
  const starts = Object.fromEntries(Object.entries(PPS_SPANS).map(([start, [end]]) => [end, start]));
  const open = new Map();
  const bands = [];
  const close = (device, start, at, detail) => {
    const key = `${device}\n${start}`;
    const [, words, color] = PPS_SPANS[start];
    bands.push({ from: open.get(key) ?? from, to: at, text: `${words}: ${detail}`, color });
    open.delete(key);
  };
  for (const e of events) {
    const device = ppsDevice(e);
    if (e.kind === "pps_down") {
      for (const start of ["pps_lost", "pps_flapping"]) {
        if (open.has(`${device}\n${start}`)) close(device, start, e.at, device);
      }
    }
    if (PPS_SPANS[e.kind]) {
      const key = `${device}\n${e.kind}`;
      if (!open.has(key)) open.set(key, e.at);
    } else if (starts[e.kind]) {
      close(device, starts[e.kind], e.at, e.detail ?? device);
    }
  }
  for (const key of [...open.keys()]) {
    const [device, start] = key.split("\n");
    close(device, start, to, `${device}, still`);
  }
  return bands.sort((a, b) => a.from - b.from);
}

// Each band across the plot area, from its start to its end, in its
// color, or as not synchronized.
function drawBands(u, bands) {
  const [lo, hi] = [u.scales.x.min, u.scales.x.max];
  const { ctx } = u;
  ctx.save();
  for (const { from, to, color } of bands) {
    if (to < lo || from > hi) continue;
    ctx.fillStyle = color ?? NOT_SYNCHRONIZED_BAND;
    const left = u.valToPos(Math.max(from, lo), "x", true);
    const right = u.valToPos(Math.min(to, hi), "x", true);
    ctx.fillRect(left, u.bbox.top, Math.max(right - left, devicePixelRatio), u.bbox.height);
  }
  ctx.restore();
}

// How far the event's text sits from the pointer, and how near the
// window's edge it may come, in CSS pixels.
const TIP_OFFSET = 12;
const TIP_MARGIN = 4;

// An event's text beside the pointer on `chart`, in the page's
// `#note-tip`; hidden when there is none.  It goes below and to the
// right of the pointer, or to the left and above where that would
// leave the window, so an event near the right edge can still be read.
function markTip(chart, text) {
  const tip = $("note-tip");
  if (!tip) return;
  if (!chart || text == null) {
    tip.style.display = "none";
    return;
  }
  const box = chart.over.getBoundingClientRect();
  const x = box.left + chart.cursor.left;
  const y = box.top + chart.cursor.top;
  tip.textContent = text;
  tip.style.display = "block";
  const { width, height } = tip.getBoundingClientRect();
  const right = x + TIP_OFFSET;
  const below = y + TIP_OFFSET;
  const left = right + width > innerWidth - TIP_MARGIN ? x - TIP_OFFSET - width : right;
  const top = below + height > innerHeight - TIP_MARGIN ? y - TIP_OFFSET - height : below;
  tip.style.left = `${Math.max(TIP_MARGIN, left)}px`;
  tip.style.top = `${Math.max(TIP_MARGIN, top)}px`;
}

// A stack of time charts with event marks and one shared readout,
// which a page keeps in `view` across reads:
//   container, readout   its elements; a stack nobody points at has
//                        no readout
//   sync                 the cursor's sync key
//   charts, marks        the charts drawn and the events marked
//   height, font, scale  optional, as `stackCharts` takes them
//   toggles, clip        optional, as `stackCharts` takes them
//   read, why, toggling  kept by drawStack: the last drawn, to draw it
//                        again, and whether its titles toggle lines
// `read` is `{ x, span, drawn, bands }`, `bands` optional, as
// `notSynchronizedBands` gives them, behind every chart but a drawing's
// whose `shaded` is false; `why` what to say when there is nothing.
function drawStack(view, read, why) {
  read.drawn = read.drawn.filter(drawsAnything);
  if (!read.drawn.length) {
    clearStack(view, why || "no readings in this range");
    return;
  }
  view.read = read;
  view.why = why;
  if (view.toggles) toggleLines(view);
  update($(view.note), "textContent", "");
  const readout = () => {
    if (!view.readout) return;
    const mark = view.charts.map((c) => markAt(c, view.marks, c.chronymonBands())).find((m) => m != null);
    showHtml($(view.readout), readoutHtml(view.charts, mark));
    const under = view.charts.find((c) => c.over.matches(":hover"));
    markTip(under, under ? markAt(under, view.marks, under.chronymonBands()) : null);
  };
  view.charts = restack($(view.container), view.charts, read, {
    height: view.height ?? plotHeight,
    sync: view.sync.key,
    hooks: {
      setCursor: [readout],
      drawClear: [(u) => drawBands(u, u.chronymonBands?.() ?? [])],
      draw: [(u) => drawMarks(u, view.marks), drawOffScale],
    },
    font: view.font,
    scale: view.scale,
    toggles: view.toggles,
    clip: view.clip,
    // The read drawn now, not the one a chart kept was made for.
    bands: () => view.read?.bands ?? [],
  });
  readout();
}

// Take a stack's charts down, saying why in their place.
function clearStack(view, why) {
  for (const c of view.charts) c.destroy();
  view.charts = [];
  view.read = null;
  $(view.container).innerHTML = "";
  if (view.readout) showHtml($(view.readout), "");
  markTip(null, null);
  update($(view.note), "textContent", why);
}

// The chart library is fetched from a CDN, so say so plainly rather
// than leaving an empty rectangle if it did not arrive.
const haveUplot = typeof uPlot !== "undefined";
const NO_UPLOT =
  "The chart library did not load. It comes from a CDN, so this page needs internet access; the strip above does not.";

// ------------------------------------------------------------- scatter
//
// One measure against another, a dot per bucket both have, with the
// least-squares line: smartclock's correlation page's, cut down.

// Fewest pairs a statistic is given for.
const MIN_PAIRS = 3;

// Pearson's r, or null for too few pairs or a series that never moves.
function pearson(pairs) {
  if (pairs.length < MIN_PAIRS) return null;
  const n = pairs.length;
  const mx = pairs.reduce((s, p) => s + p.x, 0) / n;
  const my = pairs.reduce((s, p) => s + p.y, 0) / n;
  let sxy = 0, sxx = 0, syy = 0;
  for (const { x, y } of pairs) {
    sxy += (x - mx) * (y - my);
    sxx += (x - mx) ** 2;
    syy += (y - my) ** 2;
  }
  return sxx > 0 && syy > 0 ? sxy / Math.sqrt(sxx * syy) : null;
}

// The least-squares line y = slope * x + intercept, or null.
function leastSquares(pairs) {
  if (pairs.length < MIN_PAIRS) return null;
  const n = pairs.length;
  const mx = pairs.reduce((s, p) => s + p.x, 0) / n;
  const my = pairs.reduce((s, p) => s + p.y, 0) / n;
  let sxy = 0, sxx = 0;
  for (const { x, y } of pairs) {
    sxy += (x - mx) * (y - my);
    sxx += (x - mx) ** 2;
  }
  if (!(sxx > 0)) return null;
  const slope = sxy / sxx;
  return { slope, intercept: my - slope * mx };
}

// Each value's bucket on the grid of `got`, a read of `points` buckets:
// the server reports a bucket's mean time, which differs between
// tables, so two reads pair up only by bucket.
const bucketOf = (got, points) => (t) => Math.floor(((t - got.from) * points) / (got.to - got.from + 1e-9));

// Pairs `{ at, x, y }` of the buckets two reads on one grid both have.
function pairsOn(xs, ys, points) {
  const xBucket = bucketOf(xs.read, points), yBucket = bucketOf(ys.read, points);
  const byBucket = new Map();
  xs.read.at.forEach((t, i) => {
    if (xs.values[i] != null) byBucket.set(xBucket(t), { at: t, x: xs.values[i] });
  });
  const pairs = [];
  ys.read.at.forEach((t, i) => {
    const x = byBucket.get(yBucket(t));
    if (x && ys.values[i] != null) pairs.push({ ...x, y: ys.values[i] });
  });
  return pairs;
}

// A scatter series drawn as a dot per point, for uPlot's `paths`: its
// mode 2 joins a series' points with a line unless given its own.
const DOT_RADIUS = 2;
function dots(u, seriesIdx) {
  const [dx, dy] = u.data[seriesIdx];
  const dot = new Path2D();
  const r = DOT_RADIUS * devicePixelRatio;
  for (let i = 0; i < dx.length; i++) {
    const cx = u.valToPos(dx[i], "x", true), cy = u.valToPos(dy[i], "y", true);
    dot.moveTo(cx + r, cy);
    dot.arc(cx, cy, r, 0, 2 * Math.PI);
  }
  return { stroke: dot, fill: dot };
}

// The scatter's colors: the dots, and the fitted line over them.
const DOT_COLOR = "#38bdf8";
const FIT_COLOR = "#fbbf24";

// A scatter of `pairs` in `host`, `x` and `y` each `{ name }`, with its
// least-squares line; answers the chart.
function scatter(host, pairs, x, y, width) {
  const dx = pairs.map((p) => p.x), dy = pairs.map((p) => p.y);
  const fit = leastSquares(pairs);
  const lo = Math.min(...dx), hi = Math.max(...dx);
  const line = fit ? [[lo, hi], [fit.slope * lo + fit.intercept, fit.slope * hi + fit.intercept]] : [[], []];
  return new uPlot({
    mode: 2,
    width,
    height: Math.round(Math.min(560, Math.max(300, width / 2.5))),
    scales: { x: { time: false }, y: {} },
    axes: [
      { ...AXIS, label: x.name, labelSize: 28 },
      { ...AXIS, label: y.name, labelSize: 28, size: 80 },
    ],
    series: [
      {},
      { label: "buckets", stroke: DOT_COLOR + "99", fill: DOT_COLOR + "99", paths: dots, points: { show: false } },
      { label: "fit", stroke: FIT_COLOR, width: 1.5, points: { show: false } },
    ],
    legend: { show: false },
    cursor: { drag: { x: false, y: false }, points: { show: false } },
  }, [null, [dx, dy], line], host);
}
