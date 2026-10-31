// What every page shares: the status strip, the range control, and the
// lifecycle that decides when a page reads and what it does when the
// daemon or chronyd goes away.
//
// The strip is on every page because chronyd's state is the context for
// whatever else a page shows: a chart read without knowing chronyd is
// down, or that the feed has stopped, is read wrongly.  The lifecycle is
// shared so that the pages cannot drift apart in how they behave at the
// edges.  Loaded as a classic script before each page's own, so these
// are ordinary globals, and each page ends by calling `content` once.
"use strict";

const $ = (id) => document.getElementById(id);
const pad = (n) => String(n).padStart(2, "0");
// Everything interpolated into innerHTML goes through this.  Source
// names come from chronyd's configuration and could hold anything.
const esc = (v) =>
  String(v).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fmt = (v, digits = 3) =>
  v === null || v === undefined ? "--" : Number(v).toFixed(digits);

// The HTML each element was last given by `showHtml`.  A page that
// paints every second writes only what changed: a write replaces an
// element's children, which a slow display's browser repaints, and
// can be seen to, even when they are the same.
const shownHtml = new WeakMap();

// Give `el` this HTML, unless it was the last it was given.  An
// element painted this way is written only through this and `update`.
function showHtml(el, html) {
  if (shownHtml.get(el) === html) return;
  shownHtml.set(el, html);
  el.innerHTML = html;
}

// Set one of `el`'s properties -- its text, class, title -- unless it
// already has that value.
function update(el, property, value) {
  if (el[property] === value) return;
  // Its children may be changed, and with them what `showHtml` gave it.
  shownHtml.delete(el);
  el[property] = value;
}

// How long the strip waits for the server.
const QUICK_TIMEOUT = 5000;
// How long a read of the history waits: a range over months is a scan
// of a large table, which on a slow host takes several seconds.
const HISTORY_TIMEOUT = 60000;
// How often the strip polls: the daemon writes its status every second.
const STRIP_EVERY = 1000;
// How old the daemon's last poll may be, in seconds, before the feed is
// called stale.  It polls every second by default; a few missed polls
// are a busy host, more are a daemon that has stopped.
const STALE_AFTER = 10;

// ------------------------------------------------------------- fetching

// Total: never rejects, never hangs.  A rejected fetch anywhere in the
// boot sequence would abort it before the polling timer was installed,
// leaving the page stuck on "connecting..." until someone reloaded it.
// For the strip, which reports failure in place.
async function getJson(url) {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(QUICK_TIMEOUT) });
    return await response.json();
  } catch (e) {
    return { error: String(e) };
  }
}

// An answer that arrived for a range nobody is looking at any more.
// The lifecycle drops it without a word.
class Superseded extends Error {}

// Read JSON for a page's content, or throw.  `signal` is the one the
// lifecycle hands the page, aborted when the page moves on; anything
// arriving after that throws Superseded rather than being drawn.  A
// failure the server could describe comes back as 200 with `error` in
// it, a request it refused as 400 with the same, and throws with that
// message.
async function ask(url, signal, timeout = QUICK_TIMEOUT) {
  let body;
  try {
    const r = await fetch(url, {
      signal: AbortSignal.any([signal, AbortSignal.timeout(timeout)]),
    });
    const said = r.ok ? null : await r.json().then((b) => b?.error, () => null);
    if (!r.ok) throw new Error(said ?? `the server answered ${r.status}`);
    body = await r.json();
  } catch (e) {
    if (signal.aborted) throw new Superseded();
    if (e.name === "TimeoutError") throw new Error(`no answer within ${timeout / 1000} s`);
    throw e;
  }
  if (signal.aborted) throw new Superseded();
  if (body?.error) throw new Error(body.error);
  return body;
}

// ------------------------------------------------------------- address

// Put `bounds`' ends, unix seconds, on the query `q`, whole seconds
// outward; an end left open, null, is the server's to choose.  Zero is
// an end: `all` starts at it.
function setBounds(q, bounds) {
  if (bounds.from != null) q.set("from", Math.floor(bounds.from));
  if (bounds.to != null) q.set("to", Math.ceil(bounds.to));
}

// Merge `params` into the address without touching the rest of it: a
// null value removes the key.  Every control that is worth keeping on
// a reload or a shared link -- the range, the columns, the source --
// goes through here, so none of them wipes another's setting.  `push`
// makes the change a step the browser's Back button undoes; only a
// change of range is one.
function remember(params, push = false) {
  if (addressFixed) return;
  const q = new URLSearchParams(location.search);
  for (const [k, v] of Object.entries(params)) {
    if (v === null || v === undefined) q.delete(k);
    else q.set(k, String(v));
  }
  const s = q.toString();
  const url = location.pathname + (s ? `?${s}` : "");
  if (push && url !== location.pathname + location.search) history.pushState(null, "", url);
  else history.replaceState(null, "", url);
  remembered = location.search;
}
// Whether the page leaves its address as it was loaded.  The kiosk
// does: a signage player may take a changed address as a cue to reload.
let addressFixed = false;

// The address as this page last wrote it.  Back restores an older
// address whole, and only its range is wanted from it.
let remembered = location.search;

// What the links between pages carry from this one's address: the
// range, which means the same on every page.
const CARRIED = ["last", "from", "to"];

// Point the links between pages at the same range, so moving from one
// to another keeps it rather than falling back to the default hour.
function carry() {
  const here = new URLSearchParams(location.search);
  const q = new URLSearchParams();
  for (const k of CARRIED) {
    if (here.has(k)) q.set(k, here.get(k));
  }
  for (const a of document.querySelectorAll("nav a")) {
    a.search = q.toString();
  }
}

// ---------------------------------------------------------- formatting

// SI prefixes for a time in seconds, largest first.
// Each unit a time is shown in: its size in seconds, its name, and its
// decimals.  Whole nanoseconds, the finest any of chronyd's figures
// means anything to.
const SECOND_UNITS = [[1, "s", 3], [1e-3, "ms", 3], [1e-6, "µs", 3], [1e-9, "ns", 0]];

// A time in seconds in the unit that suits it: chronyd's offsets run
// from nanoseconds to seconds, and one fixed unit reads as 0.000 at one
// end or 1234567 at the other.  `signed` puts a plus on a positive
// offset, so a sign is never mistaken for absent.
function seconds(v, signed = false) {
  if (v === null || v === undefined || !Number.isFinite(v)) return "--";
  const magnitude = Math.abs(v);
  const [size, name, decimals] = SECOND_UNITS.find(([s]) => magnitude >= s) ?? SECOND_UNITS.at(-1);
  return `${fixed(v / size, decimals, signed)} ${name}`;
}

// `v` to `decimals` places, a plus on it if `signed` and positive.
// Rounded first, so that what rounds to zero has no sign.
function fixed(v, decimals, signed) {
  const rounded = Number(v.toFixed(decimals)) || 0;
  return `${signed && rounded > 0 ? "+" : ""}${rounded.toFixed(decimals)}`;
}

// A frequency in ppm, to the thousandth chronyc shows: a part per
// billion.
const ppm = (v, signed = false) =>
  v === null || v === undefined ? "--" : `${fixed(v, 3, signed)} ppm`;

// An interval in seconds: whole seconds, and below one second, as
// `seconds` puts it.
const interval = (v) =>
  v === null || v === undefined ? "--" : v < 1 ? seconds(v) : `${v.toFixed(0)} s`;

// An age in seconds, as a person says it: not an engineering figure,
// so hours and days to a tenth.
function age(secs) {
  if (secs === null || secs === undefined || !Number.isFinite(secs)) return "--";
  if (secs < 120) return `${secs.toFixed(0)} s`;
  if (secs < 7200) return `${(secs / 60).toFixed(0)} min`;
  if (secs < 172800) return `${(secs / 3600).toFixed(1)} h`;
  return `${(secs / 86400).toFixed(1)} d`;
}

const local = (unixSeconds) => esc(new Date(unixSeconds * 1000).toLocaleString());

// chronyd's leap status, as the database names it.
const LEAP_NAMES = {
  normal: "normal",
  insert: "insert second",
  delete: "delete second",
  unsynchronized: "not synchronized",
};

// A source's state as chronyc's `sources` marks it, and the class
// that colors it.
const SOURCE_STATES = {
  selected: ["*", "state-selected"],
  combined: ["+", ""],
  not_combined: ["-", ""],
  nonselectable: ["?", "state-warn"],
  falseticker: ["x", "state-bad"],
  jittery: ["~", "state-warn"],
};

// A source as `/api/sources` gives it, by its name, or its address
// when it has none of its own.
const sourceName = (s) => (s.name && s.name !== s.address ? s.name : s.address);

// ---------------------------------------------------------------- strip

// Whether the last poll found the daemon writing and chronyd up.  Null
// until the first poll has answered.
let linkUp = null;
let ticking = false;
// The last status answer, for a page that wants it.
let lastStatus = null;

// The page's own title, before the host is put in it.
const PAGE_TITLE = document.title;

// `chronymon@host`, the program and whose clock it shows, in place of
// the title's leading `chronymon`.
function titled(host) {
  const title = host ? PAGE_TITLE.replace(/^chronymon/, `chronymon@${host}`) : PAGE_TITLE;
  if (document.title !== title) document.title = title;
}

async function tick() {
  if (ticking) return;
  ticking = true;
  try {
    const s = await getJson("/api/status");
    lastStatus = s;
    titled(s.host);
    if (s.error) {
      paintLost("no data", s.error);
      linkIs(false);
      return;
    }
    paint(s);
    linkIs(feedState(s)[0] === "live");
  } finally {
    ticking = false;
  }
}

// The feed's state as a pill's class and text: whether the daemon is
// writing, and what it last found of chronyd.
function feedState(s) {
  if (!s.status) return ["down", "NO POLL YET"];
  if (s.status.age > STALE_AFTER) return ["stale", "STALE"];
  if (s.status.state === "up") return ["live", "LIVE"];
  if (s.status.state === "unsupported") return ["down", "UNSUPPORTED"];
  return ["down", "CHRONYD DOWN"];
}

// The strip's readings, each
//   k, v  its name and its value as text
//   hint  hover text, or undefined
function readings(s) {
  const t = s.tracking;
  if (!t) return [];
  const v = t.values;
  const reference = s.reference_name
    ? `${s.reference_name}`
    : t.reference_address;
  return [
    { k: "reference", v: reference, hint: `${t.reference_address}, reference ID ${t.reference_id}` },
    { k: "stratum", v: String(t.stratum) },
    { k: "last offset", v: seconds(v.last_offset, true), hint: "The offset measured on the last clock update." },
    { k: "RMS offset", v: seconds(v.rms_offset), hint: "A long-term average of the offset." },
    { k: "frequency", v: ppm(v.frequency, true), hint: "How fast the system clock would run uncorrected; positive is fast." },
    { k: "residual freq", v: ppm(v.residual_frequency, true) },
    { k: "skew", v: ppm(v.skew), hint: "The error bound on the frequency." },
    { k: "root delay", v: seconds(v.root_delay) },
    { k: "root dispersion", v: seconds(v.root_dispersion) },
    { k: "update interval", v: interval(v.last_update_interval) },
    { k: "leap", v: LEAP_NAMES[t.leap] ?? t.leap },
    { k: "last update", v: t.reference_time === null ? "none" : `${age(s.now - t.reference_time)} ago` },
  ];
}

// One reading as a tile: its value over its name.
const statTile = (r) =>
  `<span class="stat"${r.hint ? ` title="${esc(r.hint)}"` : ""}>` +
  `<span class="v">${esc(r.v)}</span><span class="k">${esc(r.k)}</span></span>`;

// Paint the strip.
function render(s) {
  const state = feedState(s);
  const why = s.status && s.status.state !== "up" && s.status.detail
    ? ` <span class="muted">${esc(s.status.detail)}</span>`
    : "";
  // Polling is fine, but what chronyd was configured with may be stale.
  const config = s.status?.state === "up" && s.chrony_config_error
    ? ` <span class="muted" title="${esc(s.chrony_config_error)}">chrony.conf not read</span>`
    : "";
  showHtml($("status"),
    `<span class="pill ${state[0]}">${state[1]}</span>` +
    (s.status ? `<span class="muted" id="age">polled ${age(s.status.age)} ago</span>` : "") +
    why +
    config +
    readings(s).map(statTile).join(""));
}

// Paint the strip, or what the page shows in its place.
const paint = (s) => (page?.render ?? render)(s);
const paintLost = (what, why) => (page?.lost ?? lost)(what, why);

// Nothing is known.
function lost(what, why) {
  showHtml($("status"),
    `<span class="pill down">${esc(what)}</span>` +
    (why ? ` <span class="muted">${esc(why)}</span>` : ""));
}

// ------------------------------------------------------------- events

// Each event kind's color on the charts and its words in the journal:
// red for what loses data or the clock, amber for what interrupts it,
// green for what restores it.
const EVENTS = {
  daemon_start: ["#4ade80", "chronymond started"],
  daemon_stop: ["#fbbf24", "chronymond stopped"],
  daemon_crash: ["#f87171", "chronymond had crashed"],
  boot: ["#4ade80", "host booted"],
  shutdown: ["#fbbf24", "host shut down"],
  host_crash: ["#f87171", "host crashed or lost power"],
  chronyd_down: ["#f87171", "chronyd down"],
  chronyd_up: ["#4ade80", "chronyd up"],
  chronyd_restart: ["#fbbf24", "chronyd restarted"],
  chronyd_crash: ["#f87171", "chronyd crashed"],
  step: ["#f87171", "clock stepped"],
  large_offset: ["#fbbf24", "large offset"],
  reference_change: ["#fbbf24", "reference changed"],
  leap_change: ["#fbbf24", "leap status changed"],
  source_added: ["#a78bfa", "source added"],
  source_removed: ["#a78bfa", "source removed"],
  unsupported: ["#f87171", "unsupported reply"],
  chrony_config_error: ["#fbbf24", "chrony.conf not read"],
  chrony_config_read: ["#4ade80", "chrony.conf read again"],
  pps_lost: ["#f87171", "PPS edge missed"],
  pps_back: ["#4ade80", "PPS back"],
  pps_flapping: ["#f87171", "PPS flapping"],
  pps_steady: ["#4ade80", "PPS steady"],
  pps_down: ["#f87171", "PPS device unreadable"],
  pps_up: ["#4ade80", "PPS device readable"],
  gpsd_down: ["#f87171", "gpsd down"],
  gpsd_up: ["#4ade80", "gpsd up"],
  fix_lost: ["#f87171", "fix lost"],
  fix_regained: ["#4ade80", "fix regained"],
};

const eventColor = (kind) => (EVENTS[kind] ?? ["#8b929c"])[0];
const eventWords = (kind) => (EVENTS[kind] ?? [, kind])[1];

// An event as one line of text, for a chart's tip.
const eventText = (e) =>
  [eventWords(e.kind), e.source, e.detail].filter(Boolean).join(": ");

// The events over the range, as the charts mark them: `{ at, text,
// color }`, and the events themselves.
async function eventsFor(signal, from, to) {
  const q = new URLSearchParams({ limit: EVENT_ROWS });
  setBounds(q, { from, to });
  const events = await ask("/api/events?" + q, signal);
  return {
    events,
    marks: events.map((e) => ({ at: e.at, text: eventText(e), color: eventColor(e.kind) })),
  };
}

// The most events a page reads for its range.
const EVENT_ROWS = 500;

// The events as a table, newest first.  Each time centers the charts on
// it with `onAt`, or, given `href`, links to the live page there.
function eventsTable(events, href) {
  if (!events.length) return `<span class="muted">nothing recorded in this range</span>`;
  const rows = [...events].reverse().map((e) => {
    const when = href
      ? `<a href="${esc(href(e.at))}">${local(e.at)}</a>`
      : `<span class="note-link" data-at="${e.at}" title="Show the charts around it">${local(e.at)}</span>`;
    return `<tr><td>${when}</td>` +
      `<td><span style="color:${eventColor(e.kind)}">${esc(eventWords(e.kind))}</span></td>` +
      `<td>${esc(e.source ?? "")}</td><td class="muted">${esc(e.detail ?? "")}</td></tr>`;
  });
  return `<table><thead><tr><th>when</th><th>what</th><th>source</th><th>detail</th></tr></thead>` +
    `<tbody>${rows.join("")}</tbody></table>`;
}

// ------------------------------------------------------------ lifecycle
//
// Each page's content is described once, to `content`, and read by
// this, so that every page does the same thing at the same moments:
//
//   - at load, and then on a timer, as its refresh picker says (below);
//   - on a change the page makes itself, like a new range: read, with
//     any read in flight abandoned;
//   - when the feed goes or comes back: read again, since what the
//     database says still stands and may have gained a gap.
//
// A read is never started behind another: one asked for while one is in
// flight runs when that one ends.  Anything abandoned is dropped when it
// arrives, however late.

// The page's description:
//   clear(why, bad)  blank what the page shows and say why, in its own
//                    place; `bad` for an error
//   load(signal)     read the content with `ask` and draw it; throws on
//                    failure
//   floor            the shortest period its refresh picker offers, in
//                    seconds
//   render(s), lost(what, why)
//                    optional: paint the status, or its absence, in
//                    place of the strip, as `render` and `lost` do
let page = null;
let running = null;
let again = false;

function refresh() {
  if (running) {
    again = true;
    return;
  }
  const control = new AbortController();
  running = control;
  readStarted();
  page
    .load(control.signal)
    .catch((e) => {
      if (!(e instanceof Superseded)) page.clear(e.message, true);
    })
    .finally(() => {
      if (running !== control) return;
      running = null;
      if (again) {
        again = false;
        refresh();
      }
    });
}

// Abandon whatever is in flight and read again.
function renew() {
  running?.abort();
  running = null;
  again = false;
  refresh();
}

// How long after the last change the reader made -- a resize, a drag,
// the columns, the range -- a page reads again, in milliseconds.  A
// resize fires dozens a second, and a drag fires on every synced chart.
const COALESCE = 120;
let settling = null;

// Read again once changes stop coming, abandoning any read in flight.
function soon() {
  clearTimeout(settling);
  settling = setTimeout(() => renew(), COALESCE);
}

function linkIs(up) {
  if (up === linkUp) return;
  const was = linkUp;
  linkUp = up;
  // The first answer is not a change.
  if (was === null) return;
  refresh();
}

// Register the page's content and start it.  Every page calls this
// once, last.
function content(spec) {
  page = spec;
  // The refresh picker's choices depend on the page's floor.
  refreshChoice = floored(refreshChoice);
  if (rangeEl) rememberRefresh();
  drawRangeControl();
  carry();
  colophon();
  tick();
  setInterval(tick, STRIP_EVERY);
  refresh();
  schedule();
}

// ------------------------------------------------------------ colophon

// Whose this is, under what license, and where its source and issues
// are, from the server's own package metadata.  Left empty if the server cannot say.
async function colophon() {
  const about = await getJson("/api/about");
  if (about.error || !$("colophon")) return;
  const link = (href, text) => `<a href="${esc(href)}">${esc(text)}</a>`;
  $("colophon").innerHTML = [
    `© ${esc(about.copyright)}`,
    esc(about.license),
    `chronymon-web ${esc(about.version)}`,
    esc(about.database),
    link(about.repository, "github"),
    link(`${about.repository}/issues`, "issues"),
  ].join(" · ");
}

// ---------------------------------------------------------- time range
//
// The range control works as Grafana's dashboards do:
//
//   - a range is "the last N units" up to now, moving with the clock,
//     or a fixed pair; a drag on a chart zooms to a fixed pair;
//   - the back and forward buttons move it by half its length; zoom
//     out doubles it about its center, as a double click on a chart
//     does; the keys are `t Left`, `t Right`, `t -`, `t +` (halve it)
//     and `t a` (fix it where it is), and Ctrl+Z zooms out too;
//   - each change of range is a step the browser's Back button undoes;
//   - a refresh picker, Auto unless chosen otherwise, says how often
//     the range is read again.
//
// Two departures.  A range moved or zoomed out to end within half its
// length of now becomes the moving range of its length, where Grafana's
// dashboards slide on into a future with no readings.  And a fixed
// range is read again only until a read has started after its end;
// past that its readings cannot change.
//
// The range rides in the address (`last=SECONDS`, `last=all`, or
// `from=..&to=..` in unix seconds) so a reload or a shared link shows
// the same window.  Grafana's forms are read too -- `from=now-6h&to=now`,
// epoch milliseconds, ISO times -- and rewritten as these.

const RANGE_UNITS = [["min", 60], ["h", 3600], ["d", 86400]];
const RANGE_PRESETS = [
  ["1h", 3600], ["6h", 21600], ["24h", 86400],
  ["7d", 604800], ["30d", 2592000],
];
// The address keys that hold the range.
const RANGE_KEYS = ["last", "from", "to"];
// The shortest window a zoom makes, in seconds.
const SHORTEST_WINDOW = 60;
// What a zoom out multiplies the window by, and a zoom in divides it by.
const ZOOM = 2;
// The fraction of the window a step moves it by, and how near now a
// window must end to become the moving one.
const STEP = 0.5;

// `last`: seconds, or "all"; `from`/`to`: unix seconds when absolute.
let range = { last: 3600, from: null, to: null };
// The page's default length, in seconds.
let rangeFallback = 3600;

// Grafana's relative instants: "now", or "now-" a count and a unit.
// Its calendar units (months, years) and rounding (`now/d`) are not
// read.
const RELATIVE = /^now(?:-(\d+)([smhdw]))?$/;
const RELATIVE_UNITS = { s: 1, m: 60, h: 3600, d: 86400, w: 604800 };
// A bare number above this is epoch milliseconds, as Grafana writes,
// and below it unix seconds, as this page does: 1e11 seconds is the
// year 5138, 1e11 milliseconds 1973.
const MILLISECONDS_ABOVE = 1e11;

// An instant from the address: `{ ago }` in seconds before now, or
// `{ at }` in unix seconds; null if it is neither.
function instant(text) {
  if (text === null) return null;
  const relative = RELATIVE.exec(text);
  if (relative) {
    return { ago: relative[1] ? Number(relative[1]) * RELATIVE_UNITS[relative[2]] : 0 };
  }
  if (/^\d+(\.\d+)?$/.test(text)) {
    const n = Number(text);
    return { at: n > MILLISECONDS_ABOVE ? n / 1000 : n };
  }
  const parsed = Date.parse(text);
  return Number.isFinite(parsed) ? { at: parsed / 1000 } : null;
}

// Read the range from the address, and write it back in this page's
// own form.
function readRange(fallback) {
  rangeFallback = fallback ?? rangeFallback;
  range = rangeFrom(new URLSearchParams(location.search));
  rememberRange();
}

function rangeFrom(v) {
  const from = instant(v.get("from")), to = instant(v.get("to"));
  if (from && to) {
    const now = Date.now() / 1000;
    // Up to now is a moving range, however its start is written.
    const length = from.ago ?? now - from.at;
    if (to.ago === 0 && length > 0) return { last: Math.round(length), from: null, to: null };
    const start = from.at ?? now - from.ago, end = to.at ?? now - to.ago;
    if (end > start) return { last: null, from: start, to: end };
  }
  const last = v.get("last");
  if (last === "all" || last === "null") return { last: "all", from: null, to: null };
  if (last && Number(last) > 0) return { last: Number(last), from: null, to: null };
  return { last: rangeFallback, from: null, to: null };
}

// The bounds to ask the server for, in unix seconds; null is open.
function rangeBounds() {
  if (range.last === "all") return { from: 0, to: null, seconds: null };
  if (range.last !== null) {
    const to = Date.now() / 1000;
    return { from: to - range.last, to, seconds: range.last };
  }
  return { from: range.from, to: range.to, seconds: range.to - range.from };
}

// Whether the range moves with the clock.
const moving = () => range.last !== null;
// Whether reading the range again can show anything new: it moves, or
// a fixed one, such as an event's, ends after the last read started.
const growing = () => moving() || range.to > readAt / 1000;

function rememberRange(push = false) {
  const fixed = !moving();
  remember({
    last: fixed ? null : range.last,
    from: fixed ? Math.round(range.from) : null,
    to: fixed ? Math.round(range.to) : null,
  }, push);
  carry();
}

// A length in seconds as the biggest whole unit that divides it.
function splitLength(secs) {
  for (const [, size] of [...RANGE_UNITS].reverse()) {
    if (secs >= size && Number.isInteger(secs / size)) return [secs / size, size];
  }
  return [Math.max(1, Math.round(secs / 60)), 60];
}

function shortTime(t) {
  const d = new Date(t * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// The control's element and what the page does on a change, kept so
// that a change from anywhere -- the control, a drag, a key, Back --
// is drawn and read the same way.
let rangeEl = null;
let rangeChanged = null;

// Build the control into `el`; `changed` is called after every change.
function rangeControl(el, changed) {
  rangeEl = el;
  rangeChanged = changed;
  refreshChoice = refreshFrom(new URLSearchParams(location.search));
  drawRangeControl();
}

// Make `next` the range: into the address as a step Back undoes, onto
// the control, and read.  Charts synced by cursor each report the same
// drag, so a range that is already the one shown is not a change.
function setRange(next) {
  if (next.last === range.last && next.from === range.from && next.to === range.to) return;
  range = next;
  rememberRange(true);
  rangeShown();
}

// Draw the control for the range now set, and read it.
function rangeShown() {
  drawRangeControl();
  schedule();
  rangeChanged?.();
}

// A fixed window, from a drag, an event or a step.
function fixRange(from, to) {
  if (to > from) setRange({ last: null, from, to });
}

// The moving window of `length` seconds.
function moveWith(length) {
  setRange({ last: Math.max(SHORTEST_WINDOW, Math.round(length)), from: null, to: null });
}

// Back to a window moving with the clock, of the length shown.
function liveAgain() {
  moveWith(rangeBounds().seconds || rangeFallback);
}

// A window that would end within a step of now, or later, is the moving
// window of its length; an earlier one stays fixed.
function settle(from, to) {
  const length = to - from;
  if (to >= Date.now() / 1000 - length * STEP) moveWith(length);
  else fixRange(from, to);
}

// Move by a step, earlier (-1) or later (1).
function step(direction) {
  if (range.last === "all") return;
  const { from, to, seconds } = rangeBounds();
  const by = direction * seconds * STEP;
  if (direction > 0) settle(from + by, to + by);
  else fixRange(from + by, to + by);
}

// Scale the window about its center: ZOOM zooms out, 1 / ZOOM in.
function zoom(factor) {
  if (range.last === "all") return;
  const { from, to, seconds } = rangeBounds();
  const half = Math.max(SHORTEST_WINDOW, seconds * factor) / 2;
  const center = (from + to) / 2;
  if (factor > 1) settle(center - half, center + half);
  else fixRange(center - half, center + half);
}

const zoomOut = () => zoom(ZOOM);
const zoomIn = () => zoom(1 / ZOOM);

// Fix a moving window where it is now.
function fixWhereItIs() {
  if (!moving() || range.last === "all") return;
  const { from, to } = rangeBounds();
  fixRange(from, to);
}

// The window shown, as times, for a span that is always present, so the
// right-aligned buttons after it stay where they are.
function shownSpan() {
  if (range.last === "all") return "all";
  const { from, to } = rangeBounds();
  return `${shortTime(from)} – ${shortTime(to)}`;
}

function drawRangeControl() {
  const el = rangeEl;
  if (!el) return;
  const relative = moving();
  const all = range.last === "all";
  // A fixed range shows its own length, so typing over it starts from
  // what is on screen; typing makes it the moving range of that length.
  const [n, size] = all ? [1, 3600]
    : relative ? splitLength(range.last)
    : splitLength(Math.round(range.to - range.from));
  const disabled = (off) => (off ? " disabled" : "");
  el.innerHTML =
    RANGE_PRESETS.map(([label, secs]) =>
      `<button data-last="${secs}" aria-pressed="${range.last === secs}">${label}</button>`).join(" ") +
    ` <button data-last="all" aria-pressed="${all}">all</button>` +
    ` <label>last <input id="range-n" type="number" min="1" step="1" value="${n}" ` +
    `style="width:5em"> ` +
    `<select id="range-unit">` +
    RANGE_UNITS.map(([name, s]) =>
      `<option value="${s}"${s === size ? " selected" : ""}>${name}</option>`).join("") +
    `</select></label>` +
    ` <span id="range-shown" class="muted">${esc(shownSpan())}</span>` +
    ` <span class="range-group">` +
    `<button id="range-back" class="glyph" title="earlier by half the window (t ←)"${disabled(all)}>&lsaquo;</button>` +
    `<button id="range-out" class="glyph" title="zoom out (t -, Ctrl+Z, double click)"${disabled(all)}>−</button>` +
    `<button id="range-forward" class="glyph" title="later by half the window (t →)"${disabled(all || relative)}>&rsaquo;</button>` +
    `<button id="range-now" title="the same length, up to now"${disabled(relative)}>now</button>` +
    `</span> <span class="range-group">` +
    `<button id="range-reload" class="glyph" title="read again now">⟳</button>` +
    `<select id="range-refresh" title="${esc(refreshTitle())}"${disabled(!growing())}>` +
    refreshOptions().map(([value, label]) =>
      `<option value="${value}"${value === refreshChoice ? " selected" : ""}>${label}</option>`).join("") +
    `</select>` +
    `</span>`;
  for (const b of el.querySelectorAll("button[data-last]")) {
    b.onclick = () =>
      setRange({ last: b.dataset.last === "all" ? "all" : Number(b.dataset.last), from: null, to: null });
  }
  const setLast = () => {
    const count = Number($("range-n").value), unitSize = Number($("range-unit").value);
    if (count > 0) setRange({ last: count * unitSize, from: null, to: null });
  };
  $("range-n").onchange = setLast;
  $("range-unit").onchange = setLast;
  $("range-back").onclick = () => step(-1);
  $("range-out").onclick = () => zoomOut();
  $("range-forward").onclick = () => step(1);
  $("range-now").onclick = liveAgain;
  $("range-reload").onclick = () => renew();
  $("range-refresh").onchange = () => {
    refreshChoice = $("range-refresh").value;
    rememberRefresh();
    drawRangeControl();
    schedule();
  };
}

// Grafana's time keys: `t`, then the second key within this many ms.
const CHORD = 1000;
// The second key, by its `key`, and what it does.
const TIME_KEYS = {
  ArrowLeft: () => step(-1),
  ArrowRight: () => step(1),
  "-": zoomOut,
  "+": zoomIn,
  // The + key unshifted, as Grafana takes it.
  "=": zoomIn,
  a: fixWhereItIs,
};
// Keys that only modify the next, and so neither start nor end a chord.
const MODIFIERS = new Set(["Shift", "Control", "Alt", "Meta"]);
// When `t` was pressed, as the event's timeStamp; -Infinity when no
// chord is open.
let chordAt = -Infinity;
addEventListener("keydown", (e) => {
  if (!rangeEl || e.defaultPrevented || MODIFIERS.has(e.key)) return;
  // Keys typed into a control are the control's: arrows move a select
  // and `t` is text.
  if (e.target.closest?.("input, select, textarea, [contenteditable]")) return;
  if (e.ctrlKey && !e.altKey && !e.metaKey && e.key === "z") {
    e.preventDefault();
    zoomOut();
    return;
  }
  if (e.ctrlKey || e.altKey || e.metaKey) return;
  const action = TIME_KEYS[e.key];
  if (action && e.timeStamp - chordAt <= CHORD) {
    e.preventDefault();
    chordAt = -Infinity;
    action();
    return;
  }
  chordAt = e.key === "t" ? e.timeStamp : -Infinity;
});

// Back and Forward: the range from the address returned to, and
// everything else -- the columns, the source -- as it is now, since
// only a change of range is a step.
addEventListener("popstate", () => {
  if (!rangeEl) return;
  const returned = new URLSearchParams(location.search);
  const q = new URLSearchParams(remembered);
  for (const k of RANGE_KEYS) {
    if (returned.has(k)) q.set(k, returned.get(k));
    else q.delete(k);
  }
  range = rangeFrom(q);
  const s = q.toString();
  history.replaceState(null, "", location.pathname + (s ? `?${s}` : ""));
  remembered = location.search;
  rememberRange();
  rangeShown();
});

// ------------------------------------------------------------- refresh
//
// How often the range is read again.  Auto, as Grafana's does, reads
// about once per pixel's worth of time across the window, here rounded
// up to the next of Grafana's intervals; never more often than the
// page's floor, which is what one read of it costs.  Paused while the
// tab is hidden, and put off while a read is still running or the
// pointer is on a chart or text is selected, since each read redraws
// the page and would take the cursor readout, a drag in progress or the
// selection with it.

const REFRESH_INTERVALS = [
  ["5s", 5], ["10s", 10], ["30s", 30], ["1m", 60], ["5m", 300], ["15m", 900],
  ["30m", 1800], ["1h", 3600], ["2h", 7200], ["1d", 86400],
];
const REFRESH_OFF = "off";
const REFRESH_AUTO = "auto";
const REFRESH_DEFAULT = REFRESH_AUTO;
// The window Auto assumes for `all`, whose length the page does not
// know: the longest preset.
const ALL_SPAN = RANGE_PRESETS[RANGE_PRESETS.length - 1][1];
// How long a refresh put off waits to try again, in ms.
const PUT_OFF_WAIT = 1000;

// "off", "auto", or one of REFRESH_INTERVALS' names.
let refreshChoice = REFRESH_DEFAULT;
// The pending timer for the next read, if any.
let refreshTimer = null;
// When the last read started, in ms since the epoch.
let readAt = Date.now();

// The page's floor in seconds.
const refreshFloor = () => page?.floor ?? REFRESH_INTERVALS[0][1];

// An interval's length in seconds, by its name; undefined for Off and Auto.
const refreshSeconds = (name) => REFRESH_INTERVALS.find(([n]) => n === name)?.[1];

function refreshFrom(v) {
  const asked = v.get("refresh");
  const known = [REFRESH_OFF, REFRESH_AUTO, ...REFRESH_INTERVALS.map(([name]) => name)];
  return known.includes(asked) ? asked : REFRESH_DEFAULT;
}

function rememberRefresh() {
  remember({ refresh: refreshChoice === REFRESH_DEFAULT ? null : refreshChoice });
}

// A choice faster than the page's floor, from an address made on
// another page, as the fastest it does offer: the picker cannot show
// what it does not offer.
function floored(choice) {
  const floor = refreshFloor();
  const secs = refreshSeconds(choice);
  if (secs === undefined || secs >= floor) return choice;
  return REFRESH_INTERVALS.find(([, s]) => s >= floor)[0];
}

// The choices the picker offers: none faster than the page's floor.
function refreshOptions() {
  const floor = refreshFloor();
  return [
    [REFRESH_OFF, "Off"],
    [REFRESH_AUTO, "Auto"],
    ...REFRESH_INTERVALS.filter(([, secs]) => secs >= floor).map(([name]) => [name, name]),
  ];
}

// The period in seconds the range is read again at, or null for never.
function refreshEvery() {
  const floor = refreshFloor();
  if (!page || !growing() || refreshChoice === REFRESH_OFF) return null;
  if (refreshChoice !== REFRESH_AUTO) return Math.max(floor, refreshSeconds(refreshChoice));
  const span = rangeBounds().seconds ?? ALL_SPAN;
  const wanted = Math.max(floor, span / Math.max(1, innerWidth));
  return (REFRESH_INTERVALS.find(([, secs]) => secs >= wanted) ?? REFRESH_INTERVALS.at(-1))[1];
}

function refreshTitle() {
  if (!growing()) return "a range now past is not read again";
  const every = refreshEvery();
  return every === null ? "not read again" : `read again every ${every} s`;
}

// Set the timer for the next read, replacing any set before.
function schedule() {
  clearTimeout(refreshTimer);
  const every = refreshEvery();
  if (every === null) return;
  refreshTimer = setTimeout(due, every * 1000);
}

function due() {
  if (document.hidden) return;
  if (running || document.querySelector(".u-over:hover") || String(getSelection() ?? "")) {
    refreshTimer = setTimeout(due, PUT_OFF_WAIT);
    return;
  }
  refresh();
}

// A read has started, whatever started it: the next is timed from it,
// and the control's times and picker follow.
function readStarted() {
  readAt = Date.now();
  schedule();
  const shownEl = $("range-shown");
  if (shownEl) update(shownEl, "textContent", shownSpan());
  const picker = $("range-refresh");
  if (picker) {
    update(picker, "disabled", !growing());
    update(picker, "title", refreshTitle());
  }
}

// A hidden tab is not read; on its return it is, once a read is due.
document.addEventListener("visibilitychange", () => {
  const every = refreshEvery();
  if (document.hidden || every === null) return;
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(due, Math.max(0, readAt + every * 1000 - Date.now()));
});
