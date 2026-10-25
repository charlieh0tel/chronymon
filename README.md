# chronymon

A daemon that records what chronyd knows about its clock and its
sources into SQLite, and a browser view that charts it live.

Early days: see `PLAN.md` for the design and the phases.

## Running

`chronymond` polls chronyd's control socket every second and writes
`/var/lib/chronymon/chronymon.sqlite`.  It must run as chronyd's user
(`_chrony` on Debian and Ubuntu) or root, the only users chronyd's
socket admits.  `chronymond --help` lists its options; each can also
be set in the environment (`CHRONYMOND_*`).

`chronymon-web` serves the browser view from that database on
`http://127.0.0.1:8323/`: the clock live, every source and each one's
own page, the journal of events, and distributions over any range.
It reads the database and nothing else, so it needs no access to
chronyd, only to the database: run it as a user in the database's
group, with no write access to its directory.  `--listen 0.0.0.0:8323`
(or `CHRONYMON_WEB_LISTEN`) serves it to the network; it has no
authentication of its own.  The charts' library, uPlot, comes from a
CDN, so the browser needs internet access.

To look at the pages without a chronyd, fill a database with made-up
readings and serve that:

    cargo run -p chronymon-web --example synthetic -- /tmp/demo.sqlite 48
    cargo run -p chronymon-web -- --database /tmp/demo.sqlite

## Building

    make hooks   # once per clone
    make ci      # fmt, clippy, tests

## License

MIT OR Apache-2.0, at your option: `LICENSE-MIT`, `LICENSE-APACHE`.
