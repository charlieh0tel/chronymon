# chronymon: design

Why chronymon is the way it is.  Released; see "Later" for what is
open.

## Requirements

- Its own history in SQLite, surviving logrotate and restarts of
  chronyd and the daemon.  Ranges from an hour to everything.
- Live, interactive charts: zoom, pan, one shared cursor and range per
  page.
- No blanks drawn as data: gaps are gaps, a stale feed says stale, a
  lost chronyd says down, an unsynchronized clock is shaded so.
- Gaps explained: restarts, steps, source and leap changes and the
  daemon's own outages are events, marked on every time axis.
- Sources by name (`sourcename`), address beside it.
- Tolerant of new chrony versions and `.gz` rotations.
- A week of tracking draws in well under a second on an APU.
- smartclock-web's look and behavior.

## Data sources

chronyd's control protocol (`candm.h`, `cmdmon.c`), on the Unix socket
or, an option, UDP.  Over UDP chronyd answers only the commands open to the network:
through 4.6 a fixed set without `ntpdata` and `selectdata` (4.6
`cmdmon.c`, `permissions`); from 4.7 those `opencommands` names, its
list replacing the default, which has neither (4.7 `conf.c`,
`parse_open_commands`).  The socket admits only `_chrony` and root:
chronyd makes `/run/chrony` 0750 and `chronyd.sock` 0755 (4.8), and
the client must bind its reply socket beside it, as chronyc does.  No
access, no start.

1. **Control protocol (primary), our own client**, polled every
   second, each report kept only when it changes:

   | Report | New row when |
   | - | - |
   | `tracking` | chronyd updates the clock: its last offset or their RMS changes, not the reference time alone, which it moves on every slew (`handle_slew`, 4.8 `reference.c:175`); and the first read, unless of the last row's update, as a restart's is.  `tracking_report`, one row, replaced when anything changes |
   | `sources` | its last-sample age resets; `source_report`, one row per source, replaced when anything else changes |
   | `sourcestats` | its regression changes: not the estimated offset and its error alone, which chronyd extrapolates to the report's time (`SST_DoSourcestatsReport`, 4.8 `sourcestats.c:1019`), so they change every poll |
   | `ntpdata` | its valid-sample counter increments |
   | `selectdata` | it changes |
   | `sourcename` | once per new source |

   The strip reads `tracking_report`, not the latest reference time:
   chronyd reports 0 while unsynchronized (`REF_GetReferenceParams`,
   4.8 `reference.c:1216`), and a backward step moves it back, so the
   latest is not the greatest.  Unsynchronized is no time on a chart's
   axis, nor in a table's extent.  On the clock's charts, Live's and
   the kiosk's (not its offset overlay: the sources' go on), it is a
   span, shaded and named, the lines broken across it, its values
   stale: from a leap change to unsynchronized until one
   from it, or from a row at reference time 0 (the daemon's first read;
   an older daemon's every loss of sync) until the next update; at the
   latest until the daemon stopped recording.  Leap changes are
   chronyd's report: unsynchronized during a leap second too
   (`leap_in_progress`, 4.8 `reference.c:1178`), so a span under 2 s
   is none.  So is chronyd on its local reference (`local`), named so:
   reference ID 127.127.1.1 (`NTP_REFID_LOCAL`, 4.8 `ntp.h:113`),
   address unspecified, leap normal, reference time kept near now
   (`REF_GetReferenceParams`, `reference.c:1184`).  From a reference
   change to it, its detail naming it `local`, or the daemon's first
   read of it, until one from it; an unsynchronized span ends there.
   An older daemon named it `-`, as unsynchronized, so those are not
   found.  The strip names it "local reference".
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
   stays refused however the file grows.  A file is followed by inode,
   not device, whose number can change across boots (LVM, btrfs), and
   is the file read only while the bytes before where
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
- **UDP an option, not the default**: `ntpdata` and `selectdata` need
  4.7's `opencommands`, and the package supports chrony from 4.3.  It
  needs no access to the socket, which can steer chronyd; a report
  chronyd refuses there is fatal, naming the `opencommands` line, as no
  retry fixes chronyd's configuration.  The unit allows `AF_INET` and
  `AF_INET6` for it.
- **Logs kept**: the only complete record, the only refclock and
  tempcomp samples, and the backfill.

### Events

Derived at poll: chronyd restart (`chronyd.pid` changing, or the
socket refusing then answering), a step, a large offset, reference
change, source added or removed, leap change, the daemon's start and
stop.  Reference and leap changes between any two polls, a `tracking`
row or not: losing sync is no update.

chronyd's configuration (`chrony.conf`, its `include` and `confdir`
files, as `conf.c` reads them) gives `makestep` and the refclocks'
`lock`s, read at start and again whenever chronyd starts, which is
when chronyd reads it.  Where chronyd would refuse to start, the read
fails, and the daemon keeps what it read last (see "The receiver",
step 1).

A step or a large offset: an update's last offset past `makestep`'s
threshold (1 s if none).  A step only as chronyd would make one: in
its first `limit` updates, any if negative, none without `makestep`
(`is_step_limit_reached`, `reference.c`; `conf.c`).  Otherwise slewed:
a large offset.  An update is the last offset or its RMS changing; both
0 is a chronyd not yet updated, which starts the count.  A chronyd
already updating when the daemon first saw it, or back unseen, has an
unknown count: a large offset, unless the limit is negative.  Unseen:
`-R` (no limited steps), `chronyc makestep`, two updates within one
poll.

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
  crashed, chronyd down; a source also where removed; the clock also
  where chronyd lost sync).  A log's line
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

Rows a day, measured on one host (chrony 4.6.1, eight sources, a PPS
at poll 2), under a day of chronymon 0.2.0: `selection` 70,922;
`source_stats` 388,729, before it was written only when its
regression changed; `source_sample` 14,465; `tracking` 11,797,
before its history was only updates; `ntp_sample` 1,965.  The
database, the logs' backfill included, 138 MB.  Raw retention is
affordable.  Every `selection` row is kept, not thinned, decided
after measuring: about 7 MB a day there.

## Web

smartclock-web's `style.css`, `common.js`, `charts.js` and HTTP crate,
copied; uPlot 1.6.32 from jsdelivr with SRI; assets in the binary.
The strip polls `status` every second; charts reread their range on
Grafana's Auto timer (never oftener than 5 s) and redraw in place.
One bucketed query, live or zoomed, is one path to get right; nothing
arrives faster than chronyd updates, so no push channel.

A request is checked before the database is opened, and refused with
a 400 saying why: a name not served, a number that does not parse,
a column listed twice, more than 5000 points, 5000 events, or 80,000
buckets of columns (Live's every column and its log's at 5000 points,
with room).  A column repeated 600 times in a 3 KB request once cost
hundreds of megabytes.  The database's own failures stay a 200 with
the error, said where the chart would be.

Offset axes are `asinh` (linear near zero, logarithmic beyond), used
only when the data reaches past ten times the linear threshold, with
ticks at zero, the threshold and its decades, as many as fit, less
any whose label would print over zero's or another's: a step of
seconds beside nanoseconds put the threshold a few pixels from zero.

Pages: Live, Sources and each source's own, Refclocks, Temperature,
Journal, Distribution, Kiosk.

- A page's charts read their tables over one range and are drawn over
  it, not fitted to their own times: fitted, one source's measurements
  spanning hours above its selection spanning minutes put one time at
  two places, and readings that stopped short of now hid that they had.

- On Sources, a source's name in a chart's title is a button
  (`aria-pressed`) that hides its line on every chart, dimmed and struck
  through, and the y axes refit to what is left: NMEA's milliseconds of
  wander otherwise flatten everything else.  Hidden by name, or address
  for one without, in the address (`?hide=NMEA,...`).  Not on the
  kiosk, which has nothing to click and leaves paired refclocks out,
  nor a source's own page, which has one source.
- **Paired refclocks**: one another is locked to (`refclock ... lock
  NMEA`), its PPS's pulses numbered by its time of day.  Beside the PPS
  its offsets are noise: with only an NMEA it is the source, with a
  PPS locked to it not.  `/api/sources` names the refclock locked to it
  (`locked_by`), which Sources' table says beside its name, muted
  ("PPS locked to it"); Sources starts with its lines hidden, shown
  by its name (`?show=NMEA`), and the kiosk leaves it out.  A lone NMEA,
  `noselect` or not, is charted as any source.  The kiosk once left out
  `noselect` sources instead, which hid a lone NMEA and kept a paired
  one that was not `noselect`.
- Sources' offset chart is each sample's as `sources` reports it
  (`source_sample`, chronyc's Last sample: positive when the local
  clock is ahead), as the kiosk's and the table's: a refclock has no
  `ntpdata`, so `ntp_sample`'s offset, which was drawn first, left
  refclocks out and has the opposite sign.  `refclocks.log` has them
  too, but only where chronyd is told to log them.
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
  not be the host's.  The offset overlay leaves out paired refclocks, saying which: a
  paired NMEA tens of milliseconds off set its axis.  Its legend is the source list, colored alike:
  eight names in its title wrapped onto the chart.  Events moved right,
  under the charts their marks are on, so eight sources leave them
  room; no colophon, as smartclock's has none.
  Each y axis is fitted to the bulk of what it draws (buckets' lowest
  and highest included): the 0.5% to 99.5%, Distribution's, and half
  that span again either side, past a normal spread's tails; the asinh
  choice made on what is left.  A bucket beyond is drawn at the edge,
  marked, and counted in the title, in Distribution's words.  A host's
  startup transients (a source's first samples, -20 s after a boot) set
  axes to a million times the wander, which no asinh axis reads.  Not
  on the browser pages, whose full range is for finding those; and
  nothing clipped when nothing lies beyond.
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
- **Indexes move.**  `sources`, `sourcestats` and `selectdata` are
  asked by index, and chronyd renumbers after a removal
  (`SRC_DestroyInstance`, `sources.c`): a poll whose reports name
  another source than `sources` did keeps none of them; the next
  reads them again.
- **Snapshots, not streams.**  `ntpdata` keeps only the last sample;
  the logs are the full record.
- **Copying from smartclock** drifts; factor the copies out before it
  does.
- **One host.**  A second means a migration adding a host to every
  table.

## Decisions

- One host per daemon and view.  More than one is a schema change.
- Raw rows forever, every `selection` row included.  No alerting.
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
- The refclocks' locks are a `meta` row (`refclock_locks`), replaced
  whenever the daemon reads chronyd's configuration: configuration, not
  a time series, and no schema change.  So a pairing undone is undone
  for all of history, and a database no daemon has read a
  configuration into pairs nothing.
- Timestamps are integer nanoseconds since the epoch, as smartclock's.
- One Debian package, both services enabled; CI and release through
  deb-workflows; a release triggers the APT repository's rebuild.
- License MIT OR Apache-2.0; the copied smartclock files are
  relicensed by their author.

## Later

- smartclock-sensord's ambient readings.
- A Prometheus exporter.
- Every packet from `measurements.log` on the Sources page.

### The receiver, decided 2026-10-30

chronyd's reports show a refclock's effect, not its cause: a PPS's
reach falling to 0 is a pulse not arriving, or, with `lock NMEA`,
pulses discarded while the NMEA is invalid.  Whether the receiver has
a fix and whether the pulse lands are asked of the receiver's side
directly.  In this order, each its own change:

1. **chronyd's configuration, loud.**  Done.  chrony has no configuration
   reload: `chronyc reload sources` rereads only `sourcedir`, so
   reading at chronyd's start stays right.  Read as strictly as
   chronyd reads it: chronyd exits on a file it cannot open, the main
   one or one an `include` glob or `confdir` matched (`CNF_ReadFile`,
   `UTI_OpenFile` mode `'R'`, fatal), on a glob failing other than by
   matching nothing, and past its include depth of 10 (4.8 `conf.c`;
   glob(3)'s `GLOB_NOMAGIC | GLOB_ERR` tried on glibc: a pattern's
   directory missing or unreadable fails, a pattern matching nothing
   does not, a path with no pattern is read there or not).  Such a
   file was taken for an empty one, silently, and the empty locks
   replaced the `meta` row, unpairing all history; the reader also
   matched only `*` in a last component, read dotfiles glob(3) hides,
   and stopped short of chronyd's depth.  Now a failure keeps the last
   configuration and the `meta` row, logged and journaled
   (`chrony_config_error`) once per read, the next read that succeeds
   journaled too (`chrony_config_read`), and the daemon runs on: a configuration
   chronyd refused leaves chronyd down, which is journaled already,
   and a fixed one is read when chronyd next starts.  Exiting was
   decided against: the unit's start limit (10 in 300 s) would leave
   the daemon failed before chrony's fix, and chronyd reads its
   configuration as root, the daemon as `_chrony`, so a file chronyd
   reads the daemon may not.  The unit is also `WantedBy=
   chrony.service`, so every start of chrony starts the daemon too, if
   it is not running; not `PartOf=` or `BindsTo=`, which would stop it
   with chronyd, whose stopping it records.  `--chrony-config none`
   for a chronyd given its directives on its command line (it then
   reads no file, 4.8 `main.c`) or reached over UDP on another host:
   no locks, and steps seen on the clock alone.  `confdir`'s order is
   `search_dirs`': by file name, of a name in more than one directory
   the first listed.  Kept for the refclocks' locks, which the
   protocol does not give.  While a failure stands, the strip stays
   LIVE, polling being fine, with a muted note that chrony.conf could
   not be read, the error in its title (`meta` `chrony_config_error`).
2. **PPS from the kernel.**  Each device `--pps` names, none by
   default, read every poll, independent of gpsd and of chronyd's
   lock.  A pulse reaches chronyd's `refclocks.log` only once chronyd
   accepts it, after its lock and offset checks (4.8 `refclock.c`,
   `RCL_AddCookedPulse`), so a gap there is a pulse missing or
   refused, which the kernel tells apart.  Two kinds of device:
   - `/dev/ppsN`: its `/sys/class/pps/ppsN/assert`, the last edge's
     time and sequence, readable by anyone.
   - `/dev/ptpN[:channel]`, a NIC's clock timestamping the pulse on a
     pin (chrony's `refclock PHC ...:extpps`), channel 0 by default:
     its external timestamp events, read as a second reader.  Linux
     6.7 gave each open file its own queue of them, every channel
     unmasked (`PTP_MASK_*`), so reading takes none from chronyd and
     needs no ioctl; on an older kernel a reader would take chronyd's
     events, so one is refused.  A plain blocking `read()` of 32-byte
     `ptp_extts_event`s, on a thread of its own.  The daemon's unit allows
     `char-ptp` read, `PrivateDevices=` given up for
     `DevicePolicy=closed`.  Device groups differ: udev makes PTP
     devices `root:clock` only from systemd 258 (af96ccf, by chrony's
     maintainer, for linuxptp), Debian 13 defines `clock` but has no
     such rule, Debian 12 and Ubuntu 24.04 neither, and apu-0's
     (Debian 13) are `root:_chrony`, which the daemon's `_chrony`
     reads.  So the package adds `clock` to the daemon, in a drop-in,
     only where the group exists; it creates no group, which would own
     nothing and only let the unit start.  A device the daemon cannot
     read is `pps_down`, saying why.
   apu-0 (Debian 13, kernel 6.12, igb) takes its GPS pulse on
   `ptp2`'s pin 0, so it watches `ptp2`.  The PPS source the igb
   driver registers for `ptp2`, `pps2`, stayed within a few
   microseconds of each second over 30 s, but whether it is the pulse
   or the clock's own second is not settled; `/dev/ptpN` is what to
   watch for such a clock.
   Stored only as changes: the device present or gone, pulses arriving
   or missing, `pps_*` events.  Not every edge: chronyd's PPS samples
   already carry the offsets.  A single missed edge counts: not judged
   by the count between polls, which a 1 Hz poll of a 1 Hz pulse sees
   as 0 or 2, but by the edges' times against their count, and, while
   none come, the last one's age at each poll: past 1.5 s, half a
   period's slack for the poll's jitter, the edge is missed.  An event
   per change of state, not per edge: `pps_lost` at the first missed
   edge, then nothing while pulses stay missing, `pps_back` with the
   count missed when they resume.  An edge missed among good ones is
   a lost and back a second apart.  Flapping is capped: a second loss
   within a settling window, 60 s, of the last `pps_back` is
   `pps_flapping`, once, and nothing more until pulses have run
   unbroken for the window, then `pps_steady` with the edges missed
   and the losses in between.  The journal says when it started
   flapping and when it stopped, not every flap.
3. **gpsd**, only with `--gpsd ADDRESS`: its connection is its health
   (`gpsd_down`, `gpsd_up`).  `?WATCH` for its generic reports alone,
   the same whatever the receiver (`gpsd_json(5)`, 3.25): TPV's fix
   mode and status, and SKY's satellites used and seen and its DOPs,
   in `gnss` (schema 7): a row when the mode or status changes, and
   for satellites and DOPs at most once a minute, the DOPs compared to
   0.1.  On apu-0 a satellite at the edge of use flipped the count
   between 9 and 10, the DOPs with it, 6 rows in 30 s on change alone,
   so about 17,000 a day; once a minute bounds it at 1,440.  Every other
   class is read past: a u-blox's RAW and SUBFRAME, much of the
   stream on apu-0, are the driver's own.  apu-0's gpsd 3.25 sends two
   SKYs a second, one listing satellites, one of DOPs alone that
   disagree with it (GDOP 2.06 against 0.33), so only a SKY listing
   its satellites is taken.  A fix is a 3D one, or TPV status 7, a
   timing receiver's surveyed-in position whatever its mode (apu-0's
   NEO-M8T reports mode 3, status 7); `fix_lost` and `fix_regained`
   once two TPVs agree, so one wavering is not two events, and no TPV
   for 10 s is no fix.  Everyone running gpsd for chronyd runs it with
   `-n`, so connecting starts nothing that was not running.  gpsd's PPS
   messages, a cross-check on the kernel's count, are deferred: on
   apu-0 the pulse goes to the NIC's pin, not to gpsd, which so has
   none to report.
4. **Steps seen on the clock**, beside `makestep`'s rule: the
   realtime clock moved against the monotonic one between polls past
   any slew.  Catches `chronyc makestep` and `-R`, which the rule
   cannot see; where the two disagree, journaled.

A device or gpsd named but absent is recorded as down, as chronyd is,
and the daemon runs on.  On the web view, fix mode and pulse presence
are bands over the Refclocks charts (pulses missing or a device
unreadable in red, a flapping span in amber, shaded whole from
`pps_flapping` to `pps_steady`: no edge inside it is journaled, so the
ticks for each first planned have nothing to draw), satellites and
DOPs charts there,
and the events marked on every time axis.  apu-0 (gpsd and a PPS) is
the test host.

## Open questions

- Before 1.0.0 a reference change to the local reference was
  journaled as `-`, like a loss of sync, so an older database shows no
  local spans.  Infer them from a change to `-` with no leap change to
  unsynchronized?
- Two daemon restarts while chronyd stays unsynchronized: the second
  reference-time-0 row is dropped as a repeat.  Keep those, as rows on
  the local reference are?
- The log tables' unique keys cover every column, about doubling their
  indexes, `refclock_sample` at 1 Hz most.  Key on a hash of the line?
- `log_file` rows for files long gone are never pruned.
- chronyd's `-R` disables makestep's limit, unseen here; read it from
  `/proc/<pid>/cmdline`?  Steps seen on the clock (Later) would
  answer it.
- The copies of smartclock's web assets and HTTP crate: share them
  before they drift.
- The kiosk's `chronyd@host` head line can't be seen on the user's TV
  (overscan cutting the top, or too small?).
- Not yet seen on a real host: the boot, shutdown and crash events
  across a deliberate reboot; refclock fixtures captured there.
