# chronymon

A daemon that records what chronyd knows about its clock and its
sources into SQLite, and a browser view that charts it live.

Early days: see `PLAN.md` for the design and the phases.

## Running

`chronymond` polls chronyd's control socket every second and writes
`/var/lib/chronymon/chronymon.sqlite`.  It must run as chronyd's user
(`_chrony` on Debian and Ubuntu) or root, the only users chronyd's
socket admits.  It also reads chronyd's log files, if chronyd keeps
them (`log tracking measurements statistics refclocks tempcomp` in
`chrony.conf`), from `/var/log/chrony`: every sample, refclock and
tempcomp samples, and the history from before it ran.
`chronymond --help` lists its options; each can also be set in the
environment (`CHRONYMOND_*`).

## Building

    make hooks   # once per clone
    make ci      # fmt, clippy, tests

## License

MIT OR Apache-2.0, at your option: `LICENSE-MIT`, `LICENSE-APACHE`.
