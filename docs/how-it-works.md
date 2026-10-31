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
strip says "chrony.conf not read" until a read succeeds, journaled
`chrony_config_read`.  The daemon
runs as `_chrony` and chronyd reads its configuration as root, so a
file only root can read is one chronyd starts with and chronymond
cannot read.  `CHRONYMOND_CHRONY_CONFIG=none` reads nothing: no
pairing, and every large correction a large offset.

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
| `chronyd_down`, `chronyd_up` | chronyd stopped or started answering |
| `chronyd_restart` | chronyd's PID changed |
| `chronyd_crash` | chronyd's PID file names a process that is gone |
| `boot` | the host booted |
| `shutdown` | the host was shut down or powered off |
| `host_crash` | the last boot ended without a shutdown: power loss, hard reset, or a kernel crash (said so when pstore holds a record) |
| `daemon_start`, `daemon_stop`, `daemon_crash` | chronymond's own |
| `unsupported` | a reply or log layout this version cannot read |
| `chrony_config_error`, `chrony_config_read` | chronyd's configuration could not be read as chronyd reads it, what was read last kept; read again after that |

Between a leap change to unsynchronized and one from it, the clock's
charts on Live and the kiosk break their lines and shade the span:
not under 2 s, as chronyd reports a leap second as unsynchronized.
So do they while chronyd serves its local reference (`local` in
`chrony.conf`), named so; a reference change to it says `local`.
