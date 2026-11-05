# A tempcomp filter for chronyd: design note

Working name: `tempcomp-filter` (to be decided).  A small daemon and
CLI, in its own repository when it is written, that feeds chronyd's `tempcomp` a
temperature filtered to match how the crystal actually follows the
sensor.  chronymon stays an observer; this program steers the clock,
so it lives and ships apart from it.

## Why

chronyd applies `tempcomp` to each raw sensor reading as it arrives
(`read_timeout`, `tempcomp.c`, chrony 4.6), with no smoothing.  Hosts
measured with chronymon's Temperature page and
[`tools/tempcomp-study`](../tools/tempcomp-study) show that the frequency follows the
sensor through a lag, and that sensors are noisy:

| Host | Sensor | Lag | Held-out RMSE, no lag / lag, ppm |
|---|---|---|---|
| tare | CPU coretemp, NTP only | 30–90 min | 0.77 / 0.26 |
| chronos | thermal zone, PPS, ±1 C jitter | 30 min | 0.11 / 0.023 |
| draws | board sensor, PPS | 10 min | 0.029 / 0.020 |
| apu-0 | SoC, at the crystal | ~0 | 0.034 / 0.032 |

A line fitted with the lag but given raw readings makes things worse:
on chronos about 0.09 ppm RMS of fast error (about 3 us of offset per
32 s reading, twice its present RMS offset); on tare 0.67 ppm RMS,
peaks of 3 ppm.  The line needs the filtered temperature.

## Phase 1: a fixed filter

- Read the sensor (`/sys/.../temp*_input`) every `period` seconds.
- First-order filter, time constant `tau`:
  `f += (1 - exp(-dt / tau)) * (reading - f)`; restart from the
  reading after a gap of many `tau`.
- Write `f`, in the sensor's own units, atomically (temp file, then
  rename) to e.g. `/run/tempcomp-filter/temp`.  chrony.conf's
  `tempcomp` points at it, with the line chronymon fitted with the
  lag on.
- Save the filter's state to `/run` and reload it on restart, so a
  restart mid-day does not reset it to the raw reading.
- Settings: sensor, `tau`, `period`, output path; flags, or
  `/etc/default/tempcomp-filter`.
- systemd unit: `Restart=always`, ordered before `chrony.service`,
  `WatchdogSec` with `sd_notify` pings.

Phase 1 can start as a shell script and a unit on one host, to measure
the gain (offset RMS, and holdover with the reference cut) against
tempcomp off, before any of this is written properly.

## Fail-safes

chronyd keeps its last compensation when it cannot read the file: it
logs "Could not read temperature" and carries on (`read_timeout`).  So
removing the file does not turn compensation off.

- On stop (`ExecStopPost`), write T0: zero compensation, the same as
  tempcomp off.
- A hang (alive but not writing) is caught by the systemd watchdog
  and restarted.
- A crash leaves the last value, which is at worst a stale but sane
  temperature; `Restart=always` brings it back.
- chronyd ignores a compensation past 10 ppm, so a wrong line cannot
  push the clock past that.

## Phase 2: guarded learning

Refit `tau` and the line periodically (daily) from the clock's own
frequency (chronyd's frequency less the compensation) against the
temperature, the method chronymon's page uses: robust (Huber) fits,
`tau` chosen by blocked cross-validation.  Adopt a candidate only if:

- it predicts held-out readings better than both the current fit and
  no compensation;
- the temperature spread over the window is at least a minimum;
- coefficients stay within bounds, and change by at most a limit per
  refit.

Freeze learning while chronyd is not synchronized (holdover is when
tempcomp matters most and the frequency is not measured), and drop
readings near restarts and steps.  The learned fit is kept in
`/var/lib/tempcomp-filter` and each adoption logged, old and new.

Since the program would write the compensation itself, it could write
the predicted correction directly (tempcomp `0 0 1 0`) and use models
chrony's formula cannot, e.g. a two-speed response.  Not in the first
versions: chrony.conf would no longer say what is applied.

Source of the frequency: chronyd's tracking report (read-only control
protocol) or chronymon's database.  Undecided; the first keeps the two
programs independent.

## CLI

Over a Unix socket in `/run`; `status` unprivileged, the rest root or
a group.

- `status`: current `tau` and line, last refit, held-out errors
  (current, last candidate, none), frozen or not and why.
- `refit [--days N] [--apply] [--force]`: show a candidate; adopt it
  with `--apply`, guards overridden with `--force`.
- `freeze` / `resume`: stop or restart learning.
- `reset`: back to the configured `tau` and line.
- `off`: write T0 and hold it.

Not a button on chronymon's page: that page is served without
authentication and stays read-only.

## Effects on chronymon

chronyd's `tempcomp.log` records what chronyd read, so it would hold
the filtered temperature; the page then fits against it, and its lag
should come out near zero, a check that `tau` is right.  The raw
sensor drops out of the record unless chronymond reads it directly, a
separate, observe-only feature.

## Upstream

The cleaner home for the filter is chronyd itself: an optional time
constant on `tempcomp` (`[lag <seconds>]`) smoothing readings in
`read_timeout`, with both raw and filtered readings logged.  Worth
proposing on chrony-dev once phase 1 shows the gain, with the numbers.

## Open

- Name and repository.
- Host for the phase 1 experiment.
- Frequency source for learning: chronyd or chronymon's database.
