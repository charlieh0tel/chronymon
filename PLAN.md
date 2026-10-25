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
socket answers all of them.  The client binds its own socket in a
private directory beside chronyd's, as chronyc does, for chronyd to
reply to.  chronyd creates its directory and socket under a 022 umask,
so `/run/chrony` is 0750 and `chronyd.sock` 0755 (seen on 4.8): only
`_chrony` and root can send to the socket or bind beside it, and
membership of the group is not enough.

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

   chrony 4.3 to 4.9 write every one of these logs alike, header and
   line.  The header's column names cannot be split apart (`Date (UTC)
   Time`, `Peer del.`), so a log is known by its header matched whole,
   and a file whose header is not one of these is refused, with an
   `unsupported` event, rather than read by position and guesswork.

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

Events are derived at ingest: chronyd restart (the PID in
`/run/chrony/chronyd.pid` changing, or the socket refusing and then
answering again), a step (tracking's correction jumping past
chronyd's own `makestep` threshold, read from its configuration, 1 s
if it has none), reference change,
source added or removed, leap status change, and the daemon's own
start and stop.  And the host's:

- **Boot**: `/proc/sys/kernel/random/boot_id` differs from the last
  run's.
- **Shutdown or power-off**: the daemon, stopped by SIGTERM while
  systemd reports the system `stopping`, records a clean shutdown.
- **Crash or power loss**: the last run recorded no stop.  With a new
  boot ID it was the host (a kernel crash if pstore holds a new dump,
  else power loss or a hard reset); with the same one, chronymond
  itself.
- **chronyd crash**: its socket gone while `chronyd.pid` names a dead
  process; a clean exit removes the file.

## Architecture

The smartclock layout, so the two read the same:

| Crate | Kind | Role |
| - | - | - |
| `chronymon` | lib | the data model: units, event kinds |
| `chronymon-control` | lib | the control protocol client: request and reply encoding, the Unix socket transport, typed errors; synchronous |
| `chronymon-http` | lib | the small synchronous HTTP server, copied from smartclock's |
| `chronymon-log` | lib | the SQLite schema, `migrate()`, writer and readers; the chrony log parser |
| `chronymond` | bin | the poll, change detection, event detection, the one writer |
| `chronymon-web` | bin | the browser view: reads the database directly (WAL), live state from its `status` row |

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
  costs what a range of a day does to draw.  The buckets carry mean,
  min and max, in smartclock's shape, all columns of a table in one
  query so stacked charts share one grid.  A line breaks only at a
  hole wider than a few buckets and three times the series' own
  typical spacing: a source sampled every 64 s leaves dozens of empty
  buckets between samples at an hour's zoom, which is its cadence, not
  a gap.
- The reader opens the database read-only, and never creates it.
  While the daemon runs it reads the daemon's own `-shm` (SQLite's
  read-only WAL); while the daemon is stopped, when SQLite has removed
  the `-wal`, it opens the file `immutable`, which needs no WAL index.
  It must never be able to make a `-wal` or `-shm` of its own: one
  owned by the web view's user locks the daemon out of its database
  ("unable to open database file", seen in a container), so the web
  view has no write access to the directory.  No `unsafe` to keep the
  WAL files, as smartclock needs.

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
authentication.  Live updates as smartclock does them: the strip
polls the `status` row every second, and the charts read their range
again on a refresh timer (Grafana's Auto, about one read per pixel of
time, never oftener than 5 s), redrawn in place.  One bucketed query
per read is cheap at chronymon's volume, and drawing from the same
query whether live or zoomed leaves one path to get right, where a
`since` cursor appending rows would be a second.  Nothing arrives
faster than chronyd's own updates, so a push channel would buy
nothing.

The y axis of offset charts is linear near zero and logarithmic beyond
(uPlot's `asinh` scale, `distr: 4`, in 1.6.32), so a microsecond
wander and a millisecond spike are both readable on one chart.  The
linear threshold is 1, 2 or 5 times a power of ten at or above the
median magnitude; the axis is used only when the data reaches past
ten times it, a linear one otherwise, and its ticks are zero, the
threshold and decades of it (uPlot's own asinh splits crowd the
linear part).  Percentiles and histograms are on their own page.

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
- **Refclocks**: for each refclock in `refclocks.log`, its cooked
  offset on the asinh axis, the driver's raw offset, and dispersion.
- **Temperature**: tempcomp's temperature and compensation over time,
  the clock's frequency beside them, and a scatter of frequency
  against temperature, paired by bucket, with its least-squares line
  and r.  tempcomp reads its sensor file as it is, so the temperature
  is in the sensor's unit (a thermal zone's millidegrees), and is
  labeled so rather than converted on a guess.
- Live, and the frequency on Temperature, draw `tracking.log` where the
  protocol has no rows, at the bucket: the reader buckets both tables
  on one grid and keeps the log's buckets only where `tracking` has
  none, so the two never overlap.  The log's line is dashed and dimmer
  and named "from chronyd's log" in the title and the readout.  Its
  frequency and skew are ppm with the protocol's sign; its offset is
  the update's, as the report's last offset is.
- A page whose table is empty, or which the database predates, says
  why: chronyd logs refclocks and tempcomp only when told to.

## Phases

0. **Scaffold.**  Done: workspace, pinned toolchain, Makefile, CI
   through deb-workflows (`rust-ci.yml`, the commit-message check over
   the history, a scheduled audit), license files, `README.md`,
   `AGENTS.md`, the commit hook.
1. **Control client.**  `chronymon-control`: every report in the
   table, each reply type the supported versions send, with replies
   captured from each version as fixtures: a script in `tools/` runs
   each release's packaged chronyd in a Docker container with `-x`,
   synchronized to a second chronyd container serving as a local
   stratum, offline and repeatable, and records its replies.  Done:
   every report decodes as chronyc prints it, for 4.3, 4.5, 4.6.1,
   4.8 and 4.9, and the client runs against a live chronyd as
   `_chrony`.
2. **Daemon, protocol ingest.**  Schema 1 and `migrate()`, collectors,
   change detection, events, the status row the web view reads.
   Done: rows only on change (a clock update, a new sample, a changed
   regression or selection, a new valid NTP packet), so an idle second
   writes the status row alone; every event above, checked in a
   container against chronyd stopped, restarted and killed.  No socket
   of the daemon's own: the web view reads live state from the
   `status` row, written every poll, which leaves one path for data
   and nothing for the daemon to serve.
3. **Web.**  Copy in smartclock's HTTP crate and web assets; Live,
   Sources, Journal; then Distribution.  Done: `chronymon-http`,
   `chronymon-log`'s reader, `chronymon-web` with all five pages
   (Live, Sources and each source's own, Journal, Distribution),
   tested in Chromium by `make test-web` (Playwright, also in CI:
   every page against a synthetic database and against none, failing
   on any page error), first checked by hand in headless Chrome
   against synthetic data
   (`cargo run -p chronymon-web --example synthetic`) and against the
   daemon writing in a container, with chronyd stopped and restarted
   under it.
4. **Log ingest.**  Backfill, rotated and `.gz` files, header-based
   columns; then the Refclocks and Temperature pages.  Ingest done:
   schema 2 adds a table per log (`log_tracking`, `log_measurement`,
   `log_statistics`, `refclock_sample`, `tempcomp_sample`), kept apart
   from the protocol's tables because the logs carry other columns
   and times only to the second; the daemon reads every log file at
   start, every 10 s after and once more on stop, each file followed
   by inode from where it was last read, a compressed rotation read
   whole once, a line read twice recognized by its contents (times to
   the second repeat, so the keys include the values).  Checked on
   real 4.8 logs.  Pages done: Refclocks, Temperature, and Live's
   fill from `tracking.log`; checked in headless Chrome on a database
   backfilled from five days of a real host's 4.6.1 logs (a PHC PPS
   refclock locked to SHM NMEA, tempcomp from a thermal zone, 960,000
   rows) and on one with no logs at all.
5. **Packaging.**  One Debian package, `chronymon`, amd64 and arm64,
   released through deb-workflows (`rust-build-deb.yml`): both
   binaries, their systemd units and `/etc/default` files, and a
   `chronymon` system user and group for the web view (`adduser` in
   `postinst`, as smartclock does).  Done: installed under systemd in
   a container, both services start and run sandboxed, chronymond as
   `_chrony` reading chronyd's socket and logs, chronymon-web reading
   its database read-only.  A tag also triggers the APT repository's
   rebuild.
6. **Later, maybe.**  A kiosk page, smartclock-sensord's ambient
   readings, a Prometheus exporter, every packet from
   `measurements.log` on the Sources page.

## Adversarial review

- **The reports we need most are the unstable ones.**  `ntpdata`
  changed reply type in 4.6; the next change is ours to follow.
  Mitigation: fixtures per chrony version, the log tailer as
  fallback, and an "unsupported reply" state in the strip rather than
  wrong numbers.
- **A client written from GPL-2.0-only headers.**  Layouts and codes
  only, no text copied, so the client stays ours to license.
- **Privilege.**  The socket needs the `_chrony` user itself, so the
  daemon runs as it, and through the socket it could change chronyd's
  state.  It sends only reports and takes no input from the network;
  the web view, which does, runs as another user and reads the
  database through a shared group, so a compromised web view cannot
  steer the clock.  The daemon must say plainly when it is refused,
  not log nothing.
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
- The daemon runs as `_chrony`, the only user besides root that can
  use chronyd's socket (decided first as its own user in the
  `_chrony` group, which the socket's mode does not allow).  The web
  view runs as `chronymon`.
- The database is `/var/lib/chronymon/chronymon.sqlite`
  (`StateDirectory=chronymon`, owned `_chrony:chronymon`, 2750), so
  the web view, in group `chronymon`, reads it and can create nothing
  beside it.  Decided first as 2770 with the daemon under
  `UMask=0007`, so the web view could keep the WAL index itself; it
  then made WAL files the daemon could not open.
- Polled every second by default, set in `/etc/default/chronymond`.
- Web on 127.0.0.1:8323 by default; one package for amd64 and arm64.
- The logs fill the charts only where the control protocol has no
  rows -- before the daemon ran, while it was down -- drawn as lower
  precision (times to the second, four significant digits); otherwise
  they feed the pages only they can: refclocks, tempcomp, every NTP
  packet.
- No events inferred from the logs: the journal holds what the daemon
  saw, not what a backfill guesses (a NUL run left by a crash, a
  banner where chronyd reopened its logs).
- The status strip reads only the daemon's `status` row, also when
  the database holds nothing but logs: with the daemon installed, it
  is always running.
- The package enables and starts both services; releases also
  trigger the APT repository's rebuild, as smartclock's do.
- Timestamps stored as integer nanoseconds since the epoch, as
  smartclock's schema does: chrony's reference times are in
  nanoseconds, and 64 bits run to 2262.
- Debian package and systemd units; CI and release through
  deb-workflows.
- Our own control protocol client, with the logs beside it.
- Steps are judged by chronyd's own `makestep` threshold.
- License MIT OR Apache-2.0; the copied smartclock files are
  relicensed by their author.

## Open questions

- None yet.
