# How chronymon works

## Programs

The `chronymon` package ([installing](../README.md#installing))
carries both programs and starts both:

| Program | Runs as | Does |
| - | - | - |
| `chronymond` | `_chrony`, group `chronymon` | polls chronyd every second, reads its logs, writes `/var/lib/chronymon/chronymon.sqlite` |
| `chronymon-web` | `chronymon` | serves the charts on 127.0.0.1:8323 |

Each reads its settings from `/etc/default/<program>`; every setting
there is commented out and shows its default.

## Why `_chrony`

chronyd's control socket admits only its own user and root:
chronyd makes `/run/chrony` 0750 and `chronyd.sock` 0755, so being in
the `_chrony` group is not enough to send to it, nor to bind the
client's socket beside it for chronyd's replies.  chronymond sends
only read-only reports.  The web view, which is what faces the
network, runs as its own user and reads the database through group
`chronymon`, read-only, so a fault in it can neither reach chronyd's
socket nor alter the record.

Without access to the socket chronymond does not start, and says
which user it needs.  With access but no chronyd, it runs and records
chronyd as down until chronyd answers.

`ReadWritePaths=-/run/chrony`: the `-` lets the unit start, and record
chronyd as down, when `/run/chrony` does not exist yet (chrony not
started, or stopped: chrony's unit removes it), where without it the
unit fails at namespace setup (`226/NAMESPACE`).  Whether a
`/run/chrony` made later is writable depends on systemd: 259 leaves
`/run` writable under `ProtectSystem=strict`, and chronymond carries
on.  Where it is read-only, chronymond cannot bind its socket there,
records its stop, and exits with an error; systemd restarts it 5 s
later, with `/run/chrony` writable.  Both tested in a systemd
container.

The unit is wanted by chrony's as well as by boot
(`WantedBy=chrony.service`): each start of chrony starts chronymond if
it is not running, such as after its start limit was reached while
chrony was broken.  It is not stopped with chrony, whose stopping it
records.

## Over UDP

chronymond can ask chronyd's UDP command port instead of its socket:
`CHRONYMOND_SOCKET=127.0.0.1` in `/etc/default/chronymond`, or `::1`,
or with a port, `127.0.0.1:323`, `[::1]:323`.  chronyd answers there
only the commands open to the network, through 4.6 never `ntpdata`
or `selectdata`.  From 4.7 `chrony.conf` can open them; the list
replaces the default, so it names every report chronymond reads:

    opencommands sources sourcestats sourcename tracking ntpdata selectdata

Without it chronymond names that line and exits.  chronyd answers
localhost and the hosts `cmdallow` allows; others it ignores, and
chronymond records chronyd as down.  With `cmdratelimit`, a poll,
four requests a source, may be over the limit.

The package's unit runs chronymond as `_chrony` either way, for
chronyd's logs and PID file.

## Serving the network

Set `CHRONYMON_WEB_LISTEN=0.0.0.0:8323` in `/etc/default/chronymon-web`
and restart it.  The view is read-only and asks for no password.

## Kiosk

`http://<host>:8323/kiosk` is one screen for an unattended landscape
display, headed `chronyd@<host>`; every page is titled
`chronymon@<host>`.  Left, in large type, the sync state, the host's time in UTC
(below it the date, and the display's local time), the reference,
offset, frequency and skew, then the sources; right, a day of charts
and the newest events.  The time is the server's at its last answer,
counted on by the display's own clock; it stops, dimmed and showing
its age, once no answer has come for 3 s, and is dimmed while the feed
is not live.  The sources and events lose their last rows rather than
scroll, the sources with a row saying how many more.  The offset
overlay draws the sources listed, each in its color there, but a
refclock another is locked to (see "Paired refclocks").  Each chart's
y axis fits the bulk of what it draws, its 0.5% to 99.5% and half that
span again either side; a bucket past that, as a source's first
samples after chronyd restarts or a step after a boot, is drawn at the
axis's edge with a marker, and the title counts them ("0 below and 12
above the axis").  Nothing past it, nothing is clipped.  The other
pages always show the whole range.  No nav, controls,
colophon or pointer, and it never scrolls: full size from 1440x1080
CSS pixels, scaled down to fit a smaller screen.  It never changes its
address.  A reload paints the last status, sources and
events, kept in the browser's local storage and marked LAST SEEN,
before the first poll answers; without storage it starts empty.

Under the sync state, a line for each thing wrong now that the clock's
figures do not show, red or amber, worst first, and the panel edged in
the worst one's color: a PPS pulse missing, flapping or its device
unreadable, a receiver without a fix, gpsd down ("Now wrong" below),
and chrony.conf not read.  The sync state says ON BACKUPS, in amber,
the panel edged amber too, while chronyd is synchronized to other than
its own refclocks: a refclock is listed, not counting one another is
locked to, and none is selected (`/api/status`'s `on_backups`).  A
refclock kept only to be watched, `noselect` and paired to nothing,
reads as one chronyd should be using.  It says COASTING, in amber, the
panel edged amber, and the strip's "last update" turns amber, while
the source chronyd has selected answered none of its last 8 polls
(`/api/status`'s `selected_unreachable`): chronyd keeps such a source
selected, as a PPS locked to an NMEA whose gpsd went away, until it is
stale beside the sources still answering, which can be hours, and
meanwhile the clock runs on its last frequency.  A source missing a
few polls is not coasting, however long since the last update.

The other pages' strip shows all of these, coasting and on backups
too, as pills after LIVE, worst first, on a line of their own above
the readings, which sit on a grid, each column lined up from row to
row, and keep their places as the pills come and go.  The strip is
edged in the worst one's color, red or amber, as the kiosk's panel is.

| Parameter | |
| - | - |
| `last` | range in seconds, default 86400 (`from`/`to` as on the other pages) |
| `inset` | overscan margin: `N` percent (of width at the sides, of height top and bottom, at most 25), `Npx` pixels; one value for every edge or `X,Y` for sides, then top and bottom (`3`, `2,4`, `40px`); anything else is ignored |
| `debug` | a foot line: how many times this browser has loaded the page with `debug`, whether this load was a navigation or a reload, its uptime, the Chrome version, the viewport, device pixel ratio and scale, and the last uncaught error |

## Paired refclocks

A refclock that another is locked to, `lock` in `chrony.conf`, is
paired: an NMEA receiver whose time of day numbers a PPS's pulses.

    refclock SHM 0 refid NMEA poll 6 offset 43e-3 delay 10e-3 noselect
    refclock PHC /dev/ptp2:extpps:pin=0 refid PPS poll 2 lock NMEA

Beside the PPS its offsets are noise, so the Sources page says "PPS
locked to it" beside its name and starts with its lines hidden,
`?show=NMEA` or its name in a chart's title showing them, and the
kiosk leaves it out.  An NMEA no refclock is locked to
is a source like any other, `noselect` or not.  chronymond reads the
pairing from `chrony.conf`, its `include` and `confdir` files too
(`CHRONYMOND_CHRONY_CONFIG`), when it starts and whenever chronyd
starts again, as chronyd reads it only then.

It reads them as chronyd does, and where chronyd would refuse to start
-- a file it cannot open, an `include` pattern's directory it cannot
search, includes more than 10 deep -- chronymond keeps what it read
last, pairing included, journals `chrony_config_error`, and the status
strip and the kiosk say "chrony.conf not read" until a read succeeds, journaled
`chrony_config_read`.  The daemon
runs as `_chrony` and chronyd reads its configuration as root, so a
file only root can read is one chronyd starts with and chronymond
cannot read.  `CHRONYMOND_CHRONY_CONFIG=none` reads nothing: no
pairing, and every large correction a large offset.

## PPS edges

chronyd logs a pulse only once it accepts it, after its lock and
offset checks, so a gap in its samples is a pulse missing or refused.
chronymond can watch the pulse where the kernel sees it, before
chronyd, and journal every missed edge: `CHRONYMOND_PPS`, comma
separated, none by default.

- `/dev/ppsN`: read through `/sys/class/pps/ppsN/assert`, the last
  edge's time and sequence, which anyone can read.
- `/dev/ptpN[:channel]`: a network card's clock timestamping the
  pulse on a pin, as chrony's `refclock PHC /dev/ptpN:extpps` uses it,
  channel 0 by default.  chronymond reads the clock's timestamp events
  as a second reader.  From Linux 6.7 each reader has its own queue
  of them, so reading takes none from chronyd; on an older kernel
  chronymond refuses the device.  The daemon must be able to read it.
  From systemd 258, udev makes PTP devices `root:clock`, and where
  group `clock` exists the package adds it to the daemon in a drop-in,
  `/etc/systemd/system/chronymond.service.d/clock-group.conf`.  Where
  chrony's packaging makes them `root:_chrony`, as Debian 13's appear,
  the daemon's `_chrony` reads them.  Elsewhere, `root:root` on Debian
  12 and Ubuntu 24.04 without chrony's rule, the device needs a group
  the daemon is in; `pps_down` says why it could not be read.

A network card's clock with a PPS output may also register a
`/dev/ppsN` for it, named after the clock (`/sys/class/pps/ppsN/name`
says `ptp2`).  That may be the clock's own second rather than the
pulse on its pin, so for such a clock watch `/dev/ptpN`.

A single missed edge is journaled: `pps_lost`, then `pps_back` with
how many edges were missed, once pulses land again, however long the
outage.  A loss within 60 s of the last starts `pps_flapping`, and
nothing more is journaled until pulses have run unbroken for 60 s:
`pps_steady`, with the edges missed and the losses in between.

The Refclocks page and the kiosk shade these spans behind their charts: a pulse
missing or a device unreadable in red, flapping in amber, shaded whole,
as no edge within it is journaled.  A single missed edge is a thin
line.

## gpsd

`CHRONYMOND_GPSD=localhost` (or `host:port`) records what gpsd says of
each receiver: whether it has a fix, and its satellites used and seen
and its dilutions of precision, from gpsd's generic TPV and SKY
reports, whatever the receiver.  A row is written when the fix's mode
or status changes, and for satellites and dilutions, compared to 0.1,
at most once a minute: a satellite at the edge of being used flips the
count every few seconds.  gpsd must run with `-n`, as it
does for chronyd, so connecting starts nothing.

A fix is a 3D one, or a timing receiver's surveyed-in position, gpsd's
status 7, whatever its mode.  `fix_lost` and `fix_regained` are
journaled once two reports agree, so a fix wavering once is not two
events; a receiver gpsd says nothing of for 10 s has none.  gpsd
unreachable, or going away, is `gpsd_down`, and `gpsd_up` once it
answers again.

The Refclocks page charts each receiver's satellites used and seen,
PDOP and TDOP, and shades the spans without a fix and with gpsd down.

## Now wrong

What the PPS and gpsd watchers find wrong now, the spans they have
journaled open and not yet closed, the daemon keeps in `meta`
(`troubles`), written when it changes, so a page reads it with the
status rather than from events that may be older than its range.  The
strip and the kiosk show it while the feed is live, and the charts of
Refclocks and the kiosk, over a range reaching now, shade a span open
with no event in the range across all of it, as begun before.

## Steps

`step` and `large_offset` are `makestep`'s rule, from chronyd's
configuration: what chronyd would do with an update's offset.  The
daemon also watches the clock itself: each poll it reads the realtime
clock and boot time, which runs on through a suspend, and a jump of
the one against the other by more than 10% of the time between polls
(the most Linux lets chronyd slew), plus 50 ms, is a step.  Smaller
steps cannot be told from a slew.  A step on the clock with no `step`
by the rule within three polls is `clock_step`; one by the rule the
clock did not take is `step_unseen`, unless it was too small to see.  A second back at midnight UTC is
said to be a leap second.

## chronyd's logs

chronyd's logs give chronymon what its control socket cannot: every
sample (the socket gives the last), refclock and temperature
compensation samples, and the history from before chronymond ran.
chronyd writes them only when told to, in `chrony.conf`:

    logdir /var/log/chrony
    log tracking measurements statistics refclocks tempcomp

chronymond reads every file there at start, rotated and compressed
ones too, then every 10 s.  One it cannot read, a damaged `.gz` say,
is named once in its journal and tried again each time; the others,
and the polling, go on.

## Events

The journal records, besides chronyd's own changes (reference and leap
changes, sources added and removed):

| Event | Means |
| - | - |
| `step` | an update's offset past chronyd's `makestep` threshold, within its update limit: stepped |
| `large_offset` | the same past the limit, or with no `makestep`: slewed (threshold 1 s without one) |
| `clock_step` | the clock jumped between polls, against boot time, by more than any slew could, with no `step` by `makestep`'s rule within three polls: `chronyc makestep`, chronyd's `-R`, a leap second, or something else setting the clock |
| `step_unseen` | `makestep`'s rule said `step` big enough to see, and the clock did not jump within three polls |
| `chronyd_down`, `chronyd_up` | chronyd stopped or started answering |
| `chronyd_restart` | chronyd's PID changed |
| `chronyd_crash` | chronyd's PID file names a process that is gone |
| `boot` | the host booted |
| `shutdown` | the host was shut down or powered off |
| `host_crash` | the last boot ended without a shutdown: power loss, hard reset, or a kernel crash (said so when pstore holds a record) |
| `daemon_start`, `daemon_stop`, `daemon_crash` | chronymond's own |
| `unsupported` | a reply or log layout this version cannot read |
| `chrony_config_error`, `chrony_config_read` | chronyd's configuration could not be read as chronyd reads it, what was read last kept; read again after that |
| `pps_lost`, `pps_back` | a PPS device missed an edge; its pulses landing again, and how many edges were missed |
| `pps_flapping`, `pps_steady` | a loss within 60 s of the last; pulses unbroken for 60 s after, the edges missed and the losses |
| `pps_down`, `pps_up` | a PPS device could not be read; could again |
| `gpsd_down`, `gpsd_up` | gpsd could not be reached or went away; answered again |
| `fix_lost`, `fix_regained` | a receiver gpsd reports had no fix, two reports in a row or none for 10 s; had one again |

A reference change says from what to what: an NTP source by address,
shown by its name where the sources have one, a refclock by its
reference ID (`GPS`), `local`, or `none` while unsynchronized.  The kiosk's list shows each
event's source and detail after it, as the Journal does.

Between a leap change to unsynchronized and one from it, the clock's
charts on Live and the kiosk break their lines and shade the span:
not under 2 s, as chronyd reports a leap second as unsynchronized.
So do they while chronyd serves its local reference (`local` in
`chrony.conf`), named so; a reference change to it says `local`.
