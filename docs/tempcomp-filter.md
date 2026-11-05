# A tempcomp filter for chronyd: design note

Working name: `tempcomp-filter` (to be decided).  A small daemon and
CLI, in its own repository when it is written, that feeds chronyd's `tempcomp` a
temperature filtered to match how the crystal actually follows the
sensor.  chronymon stays an observer; this program steers the clock,
so it lives and ships apart from it.

## Background

### What chronyd's tempcomp does

Every `interval` seconds chronyd reads one number from the sensor
file, T, computes

    comp = k0 + (T - T0) k1 + (T - T0)^2 k2      (ppm)

and applies it to the clock's frequency at once (`read_timeout`,
`LCL_SetTempComp`, `tempcomp.c`, chrony 4.6).  There is no smoothing:
each reading's compensation replaces the last.  chronyd's own loop
then estimates whatever frequency error remains, from its reference,
at its update interval.

### Why reading the sensor directly fails

The crystal's frequency depends on the crystal's temperature, Tc, not
the sensor's.  The sensor is somewhere else (a CPU's die, an SoC, a
board), and heat reaches the crystal through the board's thermal
resistance and capacitance.  To first order the crystal follows the
sensor through a low-pass, time constant tau:

    tau dTc/dt = T - Tc

The sensor also has its own noise.

Given the raw reading, tempcomp compensates for T, while the clock
drifts with Tc.  The error it puts into the frequency is

    e(t) = k1 (T(t) - Tc(t))

For changes slower than tau, Tc keeps up and e is small.  For faster
ones, e is nearly the whole change times k1: a load spike that heats a
CPU 30 C in a minute is compensated in full at the next reading,
while the crystal has barely warmed, then compensated back when the
load ends.  In the frequency domain the error is the high-pass part of
T: |e| / |k1 T| = w tau / sqrt(1 + (w tau)^2).  The sensor's jitter
passes straight through the same way.

A frequency error integrates into offset: e for one update interval
is e x interval of time error before chronyd's loop measures and
corrects it.  On chronos (PPS, 16 s updates, 32 s readings) the raw
sensor's jitter alone is about 0.09 ppm RMS, about 3 us per reading,
twice its present RMS offset.  On tare (NTP, 257 s updates), load
spikes give errors of up to 3 ppm.

The fit suffers too.  Regressing the frequency on the raw temperature
is an errors-in-variables problem: the fast swings and jitter in T
that the crystal never feels dilute the slope toward zero.  On chronos
the raw fit's slope is -0.127 ppm/C against -0.19 through the lag; a
line fitted on raw readings both under-corrects the slow drift and
injects the fast noise.

### How the lag and the fit are found

The model is the frequency the clock would run at with no
compensation, y = chronyd's frequency less the compensation applied,
against the temperature through some response h:

    y(t) = c + k1 (h * T)(t) + k2 (h * T)(t)^2 + noise

**Middle ground (what chronymon's page does).**  h is a first-order
lag, `f += (1 - exp(-dt / tau)) (T - f)`.  For each tau on a grid
(none, 5 min to 4 h), filter T, fit c, k1 (and k2) by robust (Huber)
least squares, so steps and NTP outliers pull little, and score the
fit by blocked cross-validation: split the record into six contiguous
stretches, fit on the others (leaving out an hour either side), and
predict the held-out stretch from the temperature alone.  The tau with
the smallest held-out RMSE wins, and is reported against tau = 0.

Cross-validation, not the in-sample fit, because the frequency's
noise is slow and strongly correlated (an AR(1) fit gives rho about
0.99): consecutive samples are not independent, so in-sample
criteria reward any extra freedom.  A lag judged in-sample won where
there is none: on apu-0 a sensor step the frequency followed at once
was smeared into a trail, and 20 min "fitted" better.  Correcting for
the correlated noise instead (generalized least squares) fails the
other way: with rho near 1 it effectively differences the data and
loses the slope.  Held-out stretches sidestep both.

**Full system identification (the study's check).**  h is an
output-error model, y = c + B(q)/F(q) T, with B and F of order 1 or 2
and a dead time of 0 to 60 min, fitted by robust nonlinear least
squares on the simulated output, orders and dead time chosen by the
same blocked cross-validation.  Its step response shows the shape a
single lag cannot: on apu-0 and draws most of a step arrives at once,
the rest within 30 min; on tare almost nothing for 15 min, then most
within two hours.  It beats the middle ground clearly only on tare.

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

A Kalman filter with this model (the crystal's temperature as the
state, a first-order lag as its process, the sensor's jitter as the
measurement noise) settles to a fixed gain: the same exponential
filter.  It earns its place in phase 2, not here, beyond setting the
gain from the noise and handling irregular readings and gaps.

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

A Kalman filter is the online alternative to the daily refit: state
the crystal's temperature and the line's coefficients, measurements
the sensor reading and the clock's own frequency, so it estimates the
crystal's temperature and learns the line together, with their
uncertainty.  Holdover handles itself: with no frequency measured,
the update is skipped, learning stops and prediction carries on.
Costs: the noise variances must be chosen, tuned or estimated (EM,
innovation statistics); the coefficients as states make it nonlinear
(extended or unscented); and a mistuned filter fails quietly, so the
guards above still apply.  Start with the daily refit, whose results
chronymon's page can reproduce; consider the Kalman filter once that
is understood.

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
- Learning by daily refit or by Kalman filter.
