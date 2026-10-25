# chronymon: plan and design

A daemon that records what chronyd knows about its clock and its
sources, and a browser view that charts it live.  Status: design; no
code yet.

## Requirements

- History of its own: rows are ingested into SQLite as they appear, so
  they survive logrotate, chronyd restarts and the daemon's own.
  Ranges from an hour to everything.
- Live: charts extend as data arrives, without a reload.
- Interactive: zoom, pan and a cursor readout on every time chart, all
  charts on a page sharing one cursor and one range.
- Never show blanks as data: no data is drawn as a gap, a stale feed is
  labeled stale with its age, and a lost chronyd is labeled down.
- Gaps explained: chronyd restarts, clock steps, source changes, leap
  status changes and the daemon's own outages are events, drawn as
  markers on every time axis and listed in a journal.
- Sources by name, from chronyd (`sourcename`), with the address kept
  beside it; nothing to maintain by hand when DNS moves a server.
- Tolerant: new chrony versions, new log columns and `.gz` rotations
  do not break ingest.
- Fast on a small host: a week of tracking draws in well under a
  second on an APU-class machine.
- Same look and behavior as smartclock-web (see "Web").

## Data sources

chronyd answers its control protocol, the one chronyc speaks
(`candm.h` and `cmdmon.c` in chrony's source), on a Unix socket,
`/run/chrony/chronyd.sock`, and on UDP 127.0.0.1:323.  Over UDP it
answers only the reports `opencommands` enables (from 4.7; fixed
before), which by default excludes `ntpdata`, `sourcestats` and
`selectdata`: `ntpdata` gave `501 Not authorised` on 4.8.  The Unix
socket answers all of them; the client binds its own socket in
`/run/chrony/`, which needs the `_chrony` user or group.

chronymon speaks the Unix socket only.  UDP would give a partial
picture, and one path is less to test.  Without access to the socket
the daemon refuses to start, saying which group it needs.

The daemon reads two ways, through one ingest path:

1. **Control protocol (primary), our own client.**  Polled once a
   second, each report kept only when it has changed:

   | Report | Gives | New row when |
   | - | - | - |
   | `tracking` | reference, stratum, leap, last offset, RMS offset, frequency, residual frequency, skew, root delay and dispersion, update interval | its reference time changes |
   | `sources` | per source: mode, state, stratum, poll, reach, last sample | its last-sample age resets |
   | `sourcestats` | per source: samples, runs, span, std dev, residual frequency, skew, estimated offset and error | any value changes (the report has no timestamp) |
   | `ntpdata` | per NTP source: offset, peer delay and dispersion, jitter asymmetry, poll, test flags, counters | its valid-sample counter increments |
   | `selectdata` | per source: selection state and score | it changes |
   | `sourcename` | name for an address | once per new source |

   Polling faster than samples arrive loses nothing; polling with the
   daemon down does, which the logs backfill when chronyd keeps them.

2. **Log files (backfill and extras).**  Tailed and backfilled from
   rotated files, `.gz` included, deduplicated by (log, time, source):
   - `refclocks.log` and `tempcomp.log`, which hold per-sample data
     the control protocol does not report;
   - `tracking`, `statistics` and `measurements` logs, to fill the
     history from before the daemon ran or while it was down.

   Columns are found by name from each file's header line, never by
   position.

### Why our own client, and the logs beside it

Researched against chrony 4.2 to 4.9's source and `chrony-candm`
0.1.3's, 2026-10-24.

- **Precision.**  The protocol carries chrony's own float (7-bit
  exponent, 25-bit coefficient: about 7 significant digits at any
  magnitude) and nanosecond timestamps.  `chronyc -c` prints frequency
  and skew to 3 decimals of ppm, so in 1 ppb steps, and offsets in
  1 ns steps.  The logs print 4 significant digits (`%10.3e`),
  tracking's frequency to 1 ppb (`%10.3f`), and time to the second
  (refclocks: the microsecond).  Only the protocol keeps what a
  disciplined clock's charts need.
- **Stability.**  chrony's `opencommands` documentation calls the
  default-open reports (`tracking`, `sources`, `sourcename`,
  `activity`, `serverstats`, ...) stable and `ntpdata`, `sourcestats`,
  `selectdata` not stable.  It has moved: `ntpdata` replies with type
  16 (`RPY_NTP_DATA`) through 4.5 and type 26 (`RPY_NTP_DATA2`, 20
  bytes longer) from 4.6; `serverstats` with 24 in 4.3 and 25 from
  4.4.  The client decodes each reply type it knows by its type code,
  and reports any other as unsupported rather than guessing.  Fixtures
  from every supported chronyd, run in a container with `-x`, keep
  that honest.
- **`chrony-candm` rejected.**  It decodes no reply type past 25, so
  no `ntpdata` from 4.6 on (Debian 13's 4.6.1, 4.8); it is
  GPL-2.0-only, which Apache-2.0 cannot combine with; its async API is
  the default; it has had one release since 2024.
- **`chronyc -c` rejected.**  1 ppb resolution, positional CSV, a
  process per report per poll, and the same privilege for `ntpdata`.
- **UDP rejected.**  Unprivileged, but without `ntpdata`,
  `sourcestats` or `selectdata` by default: a view missing most of
  what it is for.
- **Logs kept.**  The only complete record (every sample, where the
  protocol gives the last), the only source for refclock and tempcomp
  samples, and the backfill for time the daemon was not running.

Events are derived at ingest: chronyd restart (socket gone and back,
or its start time moving), a step (tracking's correction jumping past
chronyd's own `makestep` threshold, read from its configuration, 1 s
if it has none), reference change,
source added or removed, leap status change, and the daemon's own
start and stop.

## Architecture

The smartclock layout, so the two read the same:

| Crate | Kind | Role |
| - | - | - |
| `chronymon` | lib | the data model: reports, samples, events, units, newtypes |
| `chronymon-control` | lib | the control protocol client: request and reply encoding, the Unix socket transport, typed errors; synchronous |
| `chronymon-log` | lib | the SQLite schema, `migrate()`, writer and readers; the chrony log parser |
| `chronymond` | bin | collectors, event detection, the one writer, a Unix socket for live state |
| `chronymon-web` | bin | the browser view: reads the database directly (WAL), asks the daemon for live state |

- One writer, any number of readers, as smartclock's log does.  The
  web view needs no daemon to show history, and says so in the strip
  when there is none.
- Synchronous: threads, no async runtime, the same small HTTP server
  smartclock uses.
- One host: the daemon records the chronyd it runs beside, and the
  web view shows that one.  More than one would be a schema change.
- Shown, not alerted: an outage turns the strip stale or down and is
  journaled; alerting is left to other tools.
- Raw rows kept forever; charts get one min/max pair per pixel column,
  computed at query time from the timestamp index, so a range of a year
  costs what a range of a day does to draw.

Rough volume: ten sources at a 64 s poll plus tracking is about 20,000
rows a day, 7 million a year.  Raw retention is affordable.

## Web

Same design as smartclock-web: the same `style.css` tokens (dark,
monospace, panels, the live/stale/down pills), the same nav row,
status strip and shared cursor readout, uPlot 1.6.32 from jsdelivr
with SRI, assets compiled into the binary.  smartclock's `style.css`,
`common.js`, `charts.js` and HTTP crate are copied in, to be factored
into something shared once both have settled.  Served directly on a
port, as smartclock-web is: 127.0.0.1:8323 by default, set in
`/etc/default/chronymon-web` (0.0.0.0 for the LAN).  Read-only, no
authentication.  Live updates as smartclock
does them: the page polls a `since` cursor every few seconds and
appends what is new.  Nothing arrives faster than chronyd's own
updates, so a push channel would buy nothing.

The y axis of offset charts is linear near zero and logarithmic beyond
(uPlot's `asinh` scale, threshold from the median magnitude), so a
microsecond wander and a millisecond spike are both readable on one
chart.  Percentile lines (1, 5, 50, 95, 99) on request.

Pages:

- **Live**: status strip (sync state, reference, stratum, offset,
  frequency, skew, RMS offset, root delay and dispersion, leap, age of
  last update); stacked charts of offset, frequency, skew, RMS offset
  and std dev, root delay and dispersion, with event markers.
- **Sources**: table of sources (name, address, state, stratum, poll,
  reach, score); charts of all sources' offset, frequency difference
  and skew; a source's own page with offset +- delay/2, delay, jitter,
  poll and selection state over time.
- **Distribution**: percentiles and histograms of offset and frequency
  over the selected range.
- **Journal**: events, each centering the charts on its time.  Events
  only; no notes written by hand.
- **Refclocks**: offset and jitter of PPS and NMEA refclocks.
- **Temperature**: tempcomp's temperature against frequency, and the
  correction it applied.

## Phases

0. **Scaffold.**  Done: workspace, pinned toolchain, Makefile, CI
   through deb-workflows (`rust-ci.yml`, the commit-message check over
   the history, a scheduled audit), license files, `README.md`,
   `AGENTS.md`, the commit hook.
1. **Control client.**  `chronymon-control`: every report in the
   table, each reply type the supported versions send, with replies
   captured from each version as fixtures: a script in `tools/` runs
   each release's packaged chronyd in a Docker container with `-x`
   and records its replies.
2. **Daemon, protocol ingest.**  Schema 1 and `migrate()`, collectors,
   change detection, events, the live socket.
3. **Web.**  Live, Sources, Journal; then Distribution.
4. **Log ingest.**  Backfill, rotated and `.gz` files, header-based
   columns; then the Refclocks and Temperature pages.
5. **Packaging.**  One Debian package, `chronymon`, amd64 and arm64,
   released through deb-workflows (`rust-build-deb.yml`): both
   binaries, their systemd units and `/etc/default` files, and a
   `chronymon` system user (sysusers.d) in the `_chrony` group.
6. **Later, maybe.**  A kiosk page, smartclock-sensord's ambient
   readings, a Prometheus exporter.

## Adversarial review

- **The reports we need most are the unstable ones.**  `ntpdata`
  changed reply type in 4.6; the next change is ours to follow.
  Mitigation: fixtures per chrony version, the log tailer as
  fallback, and an "unsupported reply" state in the strip rather than
  wrong numbers.
- **A client written from GPL-2.0-only headers.**  Layouts and codes
  only, no text copied, so the client stays ours to license.
- **Privilege.**  The socket needs `_chrony`.  On Debian,
  `LogsDirectoryMode=0750` and the socket's mode reset on each chronyd
  start, so group membership is the only setup that survives.  The
  daemon must say plainly when it is refused, not log nothing.
- **Snapshots, not streams.**  `sourcestats` has no timestamp, so its
  rows carry the poll time; `ntpdata` keeps only the last sample, so a
  sample is lost if two arrive between polls (impossible above a 1 s
  source poll).  The logs, where enabled, are the full record.
- **A daemon is more to run than a timer.**  It is also the only way
  to see an outage while it happens.  It needs a watchdog and must
  survive chronyd restarting under it.
- **Polling, not push.**  Chosen for consistency with smartclock and
  because the HTTP server is synchronous; a page held open costs one
  small request every few seconds.  Revisit only if many viewers.
- **Copying from smartclock.**  The same look today and drift
  tomorrow; the copies have to be factored out before they diverge far.
- **One host.**  Cheapest now; a second host later means a migration
  adding a host to every table, and a picker in the view.
- **What this rules out.**  Static images for a page without
  JavaScript; folding chrony into smartclock-web itself, which would
  have given correlation with the receiver for free.  Correlation can
  come back later by reading both databases.

## Decisions

- Copy smartclock's web assets and HTTP crate now; share them later.
- One host per daemon and view.
- Raw rows kept forever.
- No alerting.
- Served directly on a port.
- Separate daemon and web binaries.
- Events only in the journal.
- chrony as packaged by supported Debian and Ubuntu releases, Ubuntu
  22.04 (4.2) excepted: 4.3 (Debian 12), 4.5 (Ubuntu 24.04), 4.6.1
  (Debian 13), 4.8 (Ubuntu 26.04, 26.10), 4.9 (Debian forky).  Reply
  types: 4.9's are 4.8's; 4.3 to 4.5 send `RPY_NTP_DATA`, 4.6 on
  `RPY_NTP_DATA2`.
- Unix socket only; no socket access means the daemon does not start.
- The daemon runs as its own `chronymon` user, in the `_chrony` group,
  so it can read the socket and the logs but not chrony's state.
- The database is `/var/lib/chronymon/chronymon.sqlite`
  (`StateDirectory=chronymon`).
- Polled every second by default, set in `/etc/default/chronymond`.
- Web on 127.0.0.1:8323 by default; one package for amd64 and arm64.
- Debian package and systemd units; CI and release through
  deb-workflows.
- Our own control protocol client, with the logs beside it.
- Steps are judged by chronyd's own `makestep` threshold.
- License MIT OR Apache-2.0; the copied smartclock files are
  relicensed by their author.

## Open questions

- None yet.
