# chronymon: design

Why chronymon is the way it is.  Released; see "Later" for what is
open.

## Requirements

- Its own history in SQLite, surviving logrotate and restarts of
  chronyd and the daemon.  Ranges from an hour to everything.
- Live, interactive charts: zoom, pan, one shared cursor and range per
  page.
- No blanks drawn as data: gaps are gaps, a stale feed says stale, a
  lost chronyd says down.
- Gaps explained: restarts, steps, source and leap changes and the
  daemon's own outages are events, marked on every time axis.
- Sources by name (`sourcename`), address beside it.
- Tolerant of new chrony versions and `.gz` rotations.
- A week of tracking draws in well under a second on an APU.
- smartclock-web's look and behavior.

## Data sources

chronyd's control protocol (`candm.h`, `cmdmon.c`), on the Unix socket
only.  Over UDP chronyd answers only what `opencommands` enables,
which excludes `ntpdata`, `sourcestats` and `selectdata`.  The socket
admits only `_chrony` and root: chronyd makes `/run/chrony` 0750 and
`chronyd.sock` 0755 (4.8), and the client must bind its reply socket
beside it, as chronyc does.  No access, no start.

1. **Control protocol (primary), our own client**, polled every
   second, each report kept only when it changes:

   | Report | New row when |
   | - | - |
   | `tracking` | its reference time differs from the last row's; `tracking_report`, one row, replaced when anything changes |
   | `sources` | its last-sample age resets; `source_report`, one row per source, replaced when anything else changes |
   | `sourcestats` | any value changes (no timestamp of its own) |
   | `ntpdata` | its valid-sample counter increments |
   | `selectdata` | it changes |
   | `sourcename` | once per new source |

   The strip reads `tracking_report`, not the latest reference time:
   chronyd reports 0 while unsynchronized (`REF_GetReferenceParams`,
   4.8 `reference.c:1216`), and a backward step moves it back, so the
   latest is not the greatest.  Unsynchronized is no time on a chart's
   axis, nor in a table's extent.
   The Sources table reads `source_report`, not the last sample row,
   so reach run down to 0, or no sample at all after a restart (age
   `(uint32_t)-1`, `SST_DoSourceReport`), shows as chronyd reports it,
   not as the last sample left it.  Sources named by `server` or
   `pool` start under placeholder addresses (`IPADDR_ID`,
   `NSR_AddSourceByName`; `ID#0000000001` in the log) until resolved;
   chronyc hides them without `-a` (`client.c`), and the daemon skips
   them.  An older database's, stored in hex with no `.` or `:`, are
   left out of everything the web view lists, events included.

2. **Log files**, backfilled from rotations, `.gz` included:
   `refclocks.log` and `tempcomp.log`, which hold what the protocol
   does not; `tracking`, `statistics` and `measurements`, for history
   from before the daemon ran or while it was down.  chrony 4.3 to 4.9
   write them alike.  Their column names cannot be split apart, so a
   log is known by its header matched whole, and an unknown header is
   refused with an `unsupported` event, not read by guesswork, and
   stays refused however the file grows.  A file is followed by device
   and inode, and is the file read only while the bytes before where
   its reading stopped are unchanged: one truncated and rewritten, or
   a new one on a reused inode, is read from its start.  A line is
   kept once, keyed whole: a refclock's raw and filtered samples can
   share every value both have (`log_sample`, `refclock.c`).  One
   file that cannot be read is reported and tried again; the rest,
   and the polling, go on.

### Why our own client, and the logs beside it

From chrony 4.2 to 4.9's source and `chrony-candm` 0.1.3's,
2026-10-24.

- **Precision.**  The protocol carries chrony's float (about 7
  significant digits) and nanosecond times.  `chronyc -c` prints
  frequency and skew in 1 ppb steps; the logs print 4 significant
  digits and time to the second.
- **Stability.**  chrony calls `ntpdata`, `sourcestats` and
  `selectdata` not stable, and they move: `ntpdata` is type 16
  through 4.5, type 26 from 4.6.  Replies are decoded by type code;
  any other is unsupported, not guessed at.  Fixtures from every
  supported chronyd keep that honest.
- **`chrony-candm` rejected**: nothing past type 25, GPL-2.0-only,
  async.
- **`chronyc -c` rejected**: 1 ppb, positional CSV, a process per
  report.
- **UDP rejected**: no `ntpdata`, `sourcestats` or `selectdata`.
- **Logs kept**: the only complete record, the only refclock and
  tempcomp samples, and the backfill.

### Events

Derived at poll: chronyd restart (`chronyd.pid` changing, or the
socket refusing then answering), a step (the correction jumping past
chronyd's own `makestep` threshold, 1 s if none), reference change,
source added or removed, leap change, the daemon's start and stop.
The host's:

- **Boot**: `boot_id` differs from the last run's.
- **Shutdown**: SIGTERM while systemd reports `stopping`.
- **Crash or power loss**: the last run recorded no stop.  New boot ID:
  the host (a kernel crash if pstore has a new dump); same: chronymond.
- **chronyd crash**: socket gone while `chronyd.pid` names a dead
  process.

## Architecture

| Crate | Kind | Role |
| - | - | - |
| `chronymon` | lib | units, event kinds |
| `chronymon-control` | lib | control protocol client; synchronous |
| `chronymon-http` | lib | small synchronous HTTP server, smartclock's |
| `chronymon-log` | lib | schema, `migrate()`, writer, reader, log parser |
| `chronymond` | bin | poll, change and event detection, the one writer |
| `chronymon-web` | bin | browser view, reading the database directly |

- One writer, any number of readers.  The web view reads live state
  from the `status` row, written every poll; the daemon serves
  nothing.
- Threads, no async runtime.
- Raw rows kept forever; charts get one min/mean/max bucket per pixel
  column at query time, all of a table's columns on one grid.  The
  daemon's rows are written on change, so its lines span empty buckets
  and break only where it stopped recording (its events: stopped,
  crashed, chronyd down; a source also where removed).  A log's line
  breaks at a hole wider than a few buckets and three times its own
  spacing.  Empty buckets are `null`, breaks listed as `gaps`.
- The reader opens the database read-only (the daemon's `-shm`, or
  `immutable` when there is no `-wal`) and never creates it.  It must
  not be able to make a `-wal` or `-shm`: one owned by the web view's
  user locks the daemon out.
- `immutable` is unsafe once the daemon starts under it.  So a
  connection per request, never reused, and the request read again
  when the file's inode, length or modification time changed while
  read: the daemon's writes go to its WAL, and only a checkpoint, which
  changes the time, writes the file.
- Every query the web view makes is served by an index matching its
  predicate and order (a test reads their plans): `source_sample` by
  `sample_time`, which it is charted by, and an extent as two queries,
  since SQLite scans for `MIN` and `MAX` together.

About 20,000 rows a day for ten sources and tracking: raw retention
is affordable.

## Web

smartclock-web's `style.css`, `common.js`, `charts.js` and HTTP crate,
copied; uPlot 1.6.32 from jsdelivr with SRI; assets in the binary.
The strip polls `status` every second; charts reread their range on
Grafana's Auto timer (never oftener than 5 s) and redraw in place.
One bucketed query, live or zoomed, is one path to get right; nothing
arrives faster than chronyd updates, so no push channel.

Offset axes are `asinh` (linear near zero, logarithmic beyond), used
only when the data reaches past ten times the linear threshold, with
ticks at zero, the threshold and its decades.

Pages: Live, Sources and each source's own, Refclocks, Temperature,
Journal, Distribution, Kiosk.

- Sources chronyd has removed stay listed, last and dimmed, their
  history a link away.  Removed: missing from the daemon's latest read
  (`last_seen` older than the newest), or its last event its removal.
- Live, Kiosk and the frequency on Temperature draw `tracking.log`
  where the daemon was not recording, dashed and named "from chronyd's
  log": not wherever a bucket lacks a row, which at a 64 s update
  interval is most of them, and the two lines alternated as scraps.
- Temperature is in the sensor's own unit (a thermal zone's
  millidegrees), labeled so, not converted on a guess.
- A page whose table is empty says why: chronyd logs refclocks and
  tempcomp only when told to.
- **Kiosk**: smartclock's, one summary page, with `?inset=` and
  `?debug`; the last state from localStorage paints a reload at once.
  No warning colors on offset or skew: chrony documents no threshold.
  The host's time in UTC, as smartclock's shows its receiver's: the
  server's `now`, counted on between polls, stopped after 3 s without
  an answer; local time below, smaller, since the display's zone need
  not be the host's.  The offset overlay leaves out `noselect` sources
  (selectdata state `N`): a watched NMEA refclock tens of milliseconds
  off set its axis.  Its legend is the source list, colored alike:
  eight names in its title wrapped onto the chart.  Events moved right,
  under the charts their marks are on, so eight sources leave them
  room; no colophon, as smartclock's has none.
  Rotating through the other pages, in frames, was built and dropped:
  they are for a person at a browser.

## Adversarial review

- **The reports we need most are the unstable ones.**  Fixtures per
  chrony version, the logs as fallback, and "unsupported" rather than
  wrong numbers.
- **Written from GPL-2.0-only headers.**  Layouts and codes only, no
  text copied.
- **Privilege.**  The daemon runs as `_chrony` and could change
  chronyd's state; it sends only reports and takes no network input.
  The web view, which does, runs as another user and reads through a
  group, so it cannot steer the clock.
- **Snapshots, not streams.**  `ntpdata` keeps only the last sample;
  the logs are the full record.
- **Copying from smartclock** drifts; factor the copies out before it
  does.
- **One host.**  A second means a migration adding a host to every
  table.

## Decisions

- One host per daemon and view.  More than one is a schema change.
- Raw rows forever.  No alerting.
- Separate daemon and web binaries; web served directly on
  127.0.0.1:8323, read-only, no authentication.
- Events only in the journal; none inferred from the logs (a NUL run
  left by a crash, a reopen banner): the journal holds what the daemon
  saw.
- chrony as packaged by supported Debian and Ubuntu releases, Ubuntu
  22.04 (4.2) excepted: 4.3, 4.5, 4.6.1, 4.8, 4.9.
- The daemon runs as `_chrony` (its own user in group `_chrony` was
  tried first; the socket's mode refuses it).  The web view runs as
  `chronymon`.
- `/var/lib/chronymon` is `_chrony:chronymon` 2750, so the web view
  reads and can create nothing.  2770 with `UMask=0007` was tried
  first; the web view then made WAL files the daemon could not open.
- The logs fill the charts only where the daemon was not recording
  (first decided as wherever the protocol has no rows), marked as
  lower precision; otherwise they feed only what they alone
  have: refclocks, tempcomp, every NTP packet.
- The status strip reads only the `status` row, even when the
  database holds only logs.
- Timestamps are integer nanoseconds since the epoch, as smartclock's.
- One Debian package, both services enabled; CI and release through
  deb-workflows; a release triggers the APT repository's rebuild.
- License MIT OR Apache-2.0; the copied smartclock files are
  relicensed by their author.

## Later

- smartclock-sensord's ambient readings.
- A Prometheus exporter.
- Every packet from `measurements.log` on the Sources page.
