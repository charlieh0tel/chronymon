# Control protocol fixtures

What chronyc sent chronyd and what chronyd answered, captured from each
supported release, with chronyc's own output as the expected decoding.
Independent of chronymon's client: the bytes are chronyc's, read from
`strace`.

## Layout

`<chrony version>/<command>.txt`, one file per chronyc command:
`tracking`, `sources` (`-N sources`, which asks for source names),
`sources-n` (`-n sources`), `sourcestats`, `ntpdata`, `selectdata`,
`activity`, `serverstats`.

Each line is one of:

| Prefix | Content |
| - | - |
| `#` | chronyd's version and build, the image and package, the capture time; or a resend left out |
| `request` | a datagram chronyc sent, hex |
| `reply` | a datagram chronyd answered, hex |
| `csv` | a line of `chronyc -c` output, verbatim |
| `text` | a line of chronyc's human output, from a second run a moment later: for reference, its values may differ |

Requests and replies are in the order they went, so a report by index
(`sources`, `sourcestats`, `selectdata`) is `n_sources` and its reply
followed by one exchange per source.

## Topology

Two containers on a Docker network, both running `chronyd -x` (the
clock is never touched):

- `server`, Debian trixie's chronyd, `local stratum 8`.
- `client`, the release under test, with three sources: `server`
  polled every second, `192.0.2.1` (never answers), and `refclock
  SHM 0` (no data).  Captured once it has selected `server` and holds
  8 samples of it.

## Regenerating

    tools/capture-fixtures                  # every supported release
    tools/capture-fixtures debian:forky     # one

Needs Docker; about a minute for all five.  The values change on every
run, so a regeneration rewrites every file.
