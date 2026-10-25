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

chronyd answers its control and monitoring protocol ("cmdmon") on a
Unix socket, `/run/chrony/chronyd.sock`, and on UDP 127.0.0.1:323.
Over UDP, unprivileged, it answers only some reports (`sources` did,
`ntpdata` gave `501 Not authorised` on chrony 4.8); the Unix socket
answers all of them and needs the `_chrony` user or group.

The daemon reads two ways, through one ingest path:

1. **cmdmon (primary).**  Polled once a second, each report kept only
   when it has changed:

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

2. **Log files (fallback and extras).**  Tailed and backfilled from
   rotated files, `.gz` included, deduplicated by (log, time, source):
   - `refclocks.log` and `tempcomp.log`, which hold per-sample data
     cmdmon does not report;
   - `tracking`, `statistics` and `measurements` logs, to fill the
     history from before the daemon ran or while it was down;
   - hosts where the socket cannot be reached.

   Columns are found by name from each file's header line, never by
   position.

Events are derived at ingest: chronyd restart (socket gone and back,
or its start time moving), a step (tracking's correction jumping past
a threshold, or a `makestep` line in the journal), reference change,
source added or removed, leap status change, and the daemon's own
start and stop.

## Architecture

The smartclock layout, so the two read the same:

| Crate | Kind | Role |
| - | - | - |
| `chronymon` | lib | the data model: reports, samples, events, units, newtypes |
| `chronymon-cmdmon` | lib | the cmdmon client: request and reply encoding, socket and UDP transports, typed errors |
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
port, as smartclock-web is.  Live updates as smartclock
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

0. **Scaffold.**  Workspace, pinned toolchain, Makefile targets, CI
   (including the commit-message check over the history), license
   files, `README.md`.  `AGENTS.md` and the commit hook are done.
1. **cmdmon spike.**  Against chronyd 4.8 as `_chrony`: decide between
   `chrony-candm` (0.1.3) and our own client (it must be synchronous,
   cover every report above and keep up with chrony), and capture
   replies of every report as test fixtures.  Supported: chrony 4.x as
   in Debian 12 and later, 4.3 through 4.8, with fixtures for each.
2. **Daemon, cmdmon ingest.**  Schema 1 and `migrate()`, collectors,
   change detection, events, the live socket.
3. **Web.**  Live, Sources, Journal; then Distribution.
4. **Log ingest.**  Backfill, rotated and `.gz` files, header-based
   columns; then the Refclocks and Temperature pages.
5. **Packaging.**  Debian package, systemd units, the daemon in the
   `_chrony` group.
6. **Later, maybe.**  smartclock-sensord's ambient readings, a
   Prometheus exporter.

## Adversarial review

- **cmdmon is internal.**  chrony bumps its protocol version and
  changes reply layouts between releases, and `chrony-candm` is at
  0.1.  Mitigation: fixtures per chrony version in tests, the log
  tailer as fallback, and a clear "unsupported protocol" state in the
  strip rather than wrong numbers.
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
- chrony 4.3 through 4.8.
- Debian package and systemd units.
- License MIT OR Apache-2.0; the copied smartclock files are
  relicensed by their author.

## Open questions

- `chrony-candm` or our own cmdmon client (phase 1)?
- The step threshold that makes a correction an event.
