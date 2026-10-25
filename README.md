# chronymon

A daemon that records what chronyd knows about its clock and its
sources into SQLite, and a browser view that charts it live.

Early days: see `PLAN.md` for the design and the phases, and
`RELEASING.md` for how a release is made.

![The live page: six hours of the clock's offset, frequency and skew](docs/images/web-live.png)

| Sources | Refclocks |
| - | - |
| ![The sources page: each source as last reported, and every source's offset, delay and frequency over six hours](docs/images/web-sources.png) | ![The refclocks page: six hours of each refclock's offsets from refclocks.log](docs/images/web-refclocks.png) |
| **Temperature** | **Distribution** |
| ![The temperature page: tempcomp's temperature and compensation over six hours, and the clock's frequency](docs/images/web-temperature.png) | ![The distribution page: a histogram and percentiles of six hours' offsets](docs/images/web-distribution.png) |

Shown from the synthetic database below; `make screenshots` takes
them again.

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

`chronymon-web` serves the browser view from that database on
`http://127.0.0.1:8323/`: the clock live, every source and each one's
own page, the refclocks and tempcomp's temperature from chronyd's
logs, the journal of events, and distributions over any range.  Where
the daemon was not running, the clock's charts are filled from
`tracking.log`, dashed.
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

    make hooks        # once per clone
    make ci           # fmt, clippy, tests
    make test-web     # every page in Chromium, against a synthetic database
    make screenshots  # the pictures above, into docs/images

Both run Playwright natively after `make web-deps`, or with no Node
installed, in Playwright's container (Docker).

## License

MIT OR Apache-2.0, at your option: `LICENSE-MIT`, `LICENSE-APACHE`.
