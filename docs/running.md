# Running chronymon

## Installing

One Debian package, `chronymon`, carries both programs and starts
both:

    sudo apt install ./chronymon_<version>_amd64.deb

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

## Serving the network

Set `CHRONYMON_WEB_LISTEN=0.0.0.0:8323` in `/etc/default/chronymon-web`
and restart it.  The view is read-only and asks for no password.

## chronyd's logs

chronyd's logs give chronymon what its control socket cannot: every
sample (the socket gives the last), refclock and temperature
compensation samples, and the history from before chronymond ran.
chronyd writes them only when told to, in `chrony.conf`:

    logdir /var/log/chrony
    log tracking measurements statistics refclocks tempcomp

chronymond reads every file there at start, rotated and compressed
ones too, then every 10 s.

## Events

The journal records, besides chronyd's own changes (steps, reference
and leap changes, sources added and removed):

| Event | Means |
| - | - |
| `chronyd_down`, `chronyd_up` | chronyd stopped or started answering |
| `chronyd_restart` | chronyd's PID changed |
| `chronyd_crash` | chronyd's PID file names a process that is gone |
| `boot` | the host booted |
| `shutdown` | the host was shut down or powered off |
| `host_crash` | the last boot ended without a shutdown: power loss, hard reset, or a kernel crash (said so when pstore holds a record) |
| `daemon_start`, `daemon_stop`, `daemon_crash` | chronymond's own |
| `unsupported` | a reply or log layout this version cannot read |
