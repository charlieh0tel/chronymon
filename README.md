# chronymon

A daemon that records what chronyd knows about its clock and its
sources into SQLite, and a browser view that charts it live.

Early days: see `PLAN.md` for the design and the phases.

## Building

    make hooks   # once per clone
    make ci      # fmt, clippy, tests

## License

MIT OR Apache-2.0, at your option: `LICENSE-MIT`, `LICENSE-APACHE`.
