# chronymon

Records chronyd's tracking and source statistics in SQLite and charts
them in a browser.

![Live: six hours of offset, frequency and skew](docs/images/web-live.png)

| Sources | Refclocks |
| - | - |
| ![Sources: each source, and their offsets, delays and frequencies](docs/images/web-sources.png) | ![Refclocks: each refclock's offsets, from refclocks.log](docs/images/web-refclocks.png) |
| **Temperature** | **Distribution** |
| ![Temperature: tempcomp's temperature and compensation, and the frequency](docs/images/web-temperature.png) | ![Distribution: a histogram and percentiles of the offset](docs/images/web-distribution.png) |

![Kiosk: the clock's state and the time in large type, sources, events and a day of charts](docs/images/web-kiosk.png)

Synthetic data; `make screenshots` retakes them.

## Installing

Installing the Debian package is the easiest way.  One package,
`chronymon`, carries both programs and starts both.  Take the amd64 or
arm64 `.deb` from the
[latest release](https://github.com/charlieh0tel/chronymon/releases/latest),
or from the [APT repository](https://github.com/charlieh0tel/apt-repo):

    sudo apt install ./chronymon_<version>_amd64.deb

Betas, tagged like `v1.2.0-beta.1`, are
[pre-releases](https://github.com/charlieh0tel/chronymon/releases) on
GitHub only, never in the APT repository.

To build the package yourself, with
[cargo-deb](https://github.com/kornelski/cargo-deb):

    cargo install cargo-deb
    cargo build --release --workspace
    cargo deb --no-build -p chronymond   # target/debian/chronymon_*.deb

## How it works

More in [docs/how-it-works.md](docs/how-it-works.md): settings, the
network, the kiosk, chronyd's logs and events.

- `chronymond` polls chronyd's control socket, or
  [its UDP command port](docs/how-it-works.md#over-udp), every second
  and reads chronyd's logs (`log tracking measurements statistics
  refclocks tempcomp`), history included.  It runs as `_chrony`: the socket
  admits only chronyd's user and root.
- `chronymon-web` serves the database read-only on
  `127.0.0.1:8323`, without authentication.  uPlot comes from a CDN.
  `/kiosk` is one screen for a display across a room
  ([options](docs/how-it-works.md#kiosk)).

Options: `--help`, or `CHRONYMOND_*` and `CHRONYMON_WEB_*` in the
environment.

## Developing

    cargo run -p chronymon-web --example synthetic -- /tmp/demo.sqlite
    cargo run -p chronymon-web -- --database /tmp/demo.sqlite

    make hooks        # once per clone
    make ci           # fmt, clippy, tests
    make test-web     # browser tests
    make screenshots  # docs/images

The browser targets use Node after `make web-deps`, else Docker.  The
synthetic data can end as other kinds of host, `ntp`, `gpsd-stopped`
or `backups` (`synthetic -- DATABASE HOURS HOST`), and the browser
tests read each through the real server: what its strip and kiosk say,
and that nothing in them overflows or overlaps at phone, laptop and
television sizes.

## License

MIT OR Apache-2.0.
