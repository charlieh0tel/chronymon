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

The sensor also has its own noise, and is discretized (below).

Given the raw reading, tempcomp compensates for T, while the clock
drifts with Tc.  The error it puts into the frequency is

    e(t) = k1 (T(t) - Tc(t))

For changes slower than tau, Tc keeps up and e is small.  For faster
ones, e is nearly the whole change times k1: a load spike that heats a
CPU 30 °C in a minute is compensated in full at the next reading,
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
the raw fit's slope is -0.127 ppm/°C against -0.19 ppm/°C through the lag; a
line fitted on raw readings both under-corrects the slow drift and
injects the fast noise.

### Discretization

Linux reports a sensor in millidegrees Celsius, but each sensor
resolves only its own step, which differs from sensor to sensor.
Measured from an hour of each host's readings (the gaps between
distinct values), with the jump each step makes in the compensation
at the host's fitted slope:

| Host | Sensor | Step | Slope | Jump per step |
|---|---|---|---|---|
| tare | coretemp | 1 °C | about -0.09 ppm/°C | about 0.09 ppm |
| draws | board sensor | 0.538 °C | -0.177 ppm/°C | 0.095 ppm |
| chronos | thermal zone | 0.454 °C | -0.19 ppm/°C | 0.086 ppm |
| apu-0 | SoC | 0.1 °C | -0.131 ppm/°C | 0.013 ppm |

The step matters at every stage:

- **Analysis.**  A reading is the temperature to within a step: an
  error of at least step^2 / 12 in variance (0.29 °C RMS for a 1 °C
  step), but not white noise: while the temperature drifts slowly
  near a step's edge, the reading flips between two levels, a
  square wave the crystal never feels.  It is part of the error in
  the temperature that dilutes a raw fit's slope.  The steps are
  found from the data, the gaps between distinct readings, not
  assumed.
- **Post-processing.**  The lag filter averages the flips, and
  resolves the temperature finer than a step only because the
  temperature wanders across steps (dither); with too short a tau
  the flips pass through.  A Kalman filter's measurement noise is at
  least step^2 / 12.  The filtered value is written with finer
  resolution than the sensor's: chronyd reads the file as a double
  (`fscanf("%lf")`), so fractional millidegrees are kept.
- **Control loop.**  Read raw, every flip is a jump of the
  compensation by the table's last column, applied at once: on
  chronos about 0.09 ppm every time the reading flips, the same size
  as the 0.09 ppm RMS error the raw sensor injects there.  Filtered,
  the compensation moves smoothly.  The program measures the step at
  start and reports it (`status`), and checks that its tau is long
  against how often the reading flips.

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

| Host | Sensor | Step | Reference | Lag | Held-out RMSE, no lag / lag, ppm |
|---|---|---|---|---|---|
| tare | CPU coretemp | 1 °C | NTP | 30–90 min | 0.77 / 0.26 |
| chronos | thermal zone | 0.454 °C | PPS | 30 min | 0.11 / 0.023 |
| draws | board sensor | 0.538 °C | PPS | 10 min | 0.029 / 0.020 |
| apu-0 | SoC, at the crystal | 0.1 °C | PPS | about 0 | 0.034 / 0.032 |

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

## Proposal for chrony

The cleaner home for the filter is chronyd itself: one number, a lag,
on `tempcomp`.  It removes the separate program, and with it the
stale-file fault, for anyone who only needs the filter.

### Syntax

An optional keyword at the end of either form, in chrony's usual
style for options:

    tempcomp <file> <interval> <T0> <k0> <k1> <k2> lag <seconds>
    tempcomp <file> <interval> <points-file> lag <seconds>

No `lag`, or `lag 0`, is today's behavior.

### Behavior

In `read_timeout`, before the reading reaches the formula or the
points:

    f += (1 - exp(-dt / lag)) * (T - f)

with `dt` the time since the last good reading.

- The filter starts from the first good reading.
- A failed read skips the update and keeps the state; a gap of many
  lags starts it again from the next good reading.
- The filtered value goes through the existing formula or points and
  the existing 10 ppm limit, unchanged.

### Log

`tempcomp.log` gains the filtered temperature as a new last column,
after the raw reading and the compensation, so a tool can fit either
and see what was applied.  The existing columns keep their places.

### Documentation

A paragraph in `chrony.conf.adoc`: when to use it (a sensor away from
the crystal, so the crystal warms more slowly than the sensor; or a
noisy or coarsely discretized sensor), and how to choose the value
(fit the frequency against the temperature through a first-order lag
for a range of lags, and keep the one that best predicts readings
left out of the fit).  It does not promise that the value is the
crystal's thermal time constant: a lag fitted from chronyd's
frequency also includes chronyd's own smoothing of its estimate,
larger with NTP than with a PPS.

### Why one number

A second number was tried on the same data: an immediate share (part
of the temperature reaching the crystal at once, the rest through the
lag), and a dead time.  Held-out RMSE, ppm, lag alone / with the
share / with the dead time: apu-0 0.032 / 0.029 / 0.032, draws
0.020 / 0.019 / 0.020, chronos 0.046 / 0.041 / 0.046, tare 0.37 /
0.37 / 0.31.  The share gains at most 0.005 ppm (under 0.1 ns over a
16 s PPS update), within the uncertainty of a few days' data, and
passes its share of the sensor's noise and steps straight through;
the dead time helps only tare, an NTP host.  The lag alone gets nearly
all of the gain.

### Case to make

- Evidence: the held-out RMSE per host, no lag against lag ("Why",
  above), and, from phase 1, offset and holdover before and after.
  Lean on the PPS hosts, whose frequency chronyd estimates with
  little smoothing of its own.
- The likely answer, "write the file with an external program": a
  helper's file going stale keeps chronyd compensating with its last
  value, silently (`read_timeout` keeps the last compensation when it
  cannot read); a helper is one more package and service; and the
  smoothing is part of the compensation's model.
- The discretization: on chronos every flip of the reading between
  two steps jumps the compensation by about 0.09 ppm, which the lag
  averages away.

### Process

Ask on chrony-dev first, with the problem, the evidence and the
syntax, before writing the patch; the maintainer's answer shapes it.
Then a patch against chrony's current release, tested on a host built
from source.  chrony takes patches on the mailing list, not as merge
requests.

## Open

- Name and repository.
- Host for the phase 1 experiment.
- Frequency source for learning: chronyd or chronymon's database.
- Learning by daily refit or by Kalman filter.
- Proposal for chrony: ask before or after the phase 1 experiment.
