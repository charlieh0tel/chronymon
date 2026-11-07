//! The Temperature page's fit: the clock's own frequency against the
//! temperature chronyd's `tempcomp` reads, and robust linear and
//! quadratic fits of it.

use chronymon_log::reader::ClockUpdate;

/// Fewest pairs a fit is given for.
const MIN_PAIRS: usize = 3;

/// Huber's constant, in robust standard deviations of the residuals: a
/// residual past it counts for less, an NTP source's wander or a step
/// pulling the fit less than it would least squares; on normal noise
/// the fit is 95% as efficient as least squares.
const HUBER: f64 = 1.345;
/// The median absolute deviation to the standard deviation, for normal
/// noise.
const MAD_SIGMA: f64 = 1.4826;
/// Most rounds of reweighting, and the change in the residuals' scale,
/// relative, below which the weights have settled.
const ROBUST_ROUNDS: usize = 50;
const ROBUST_SETTLED: f64 = 1e-6;

/// An update is set against a reading no further back than this many
/// of the readings' usual interval: past it, the sensor or chronyd
/// stopped.
const GAP_READINGS: f64 = 3.0;

/// A fit of degree d is made only where the temperatures fill at least
/// d + 2 of this many equal stretches of their range, each by this
/// share of the pairs, the range from the lowest and highest this share
/// in: two temperatures fit a line exactly and three a curve, and a
/// stretch's few strays are not a spread.
pub(crate) const SPREAD_STRETCHES: usize = 5;
const SPREAD_SHARE: f64 = 0.02;
const SPREAD_TAIL: f64 = 0.01;
/// A pivot this small beside the pairs' total weight, x scaled to about
/// 1, leaves the normal equations singular: the fit is not determined.
const SINGULAR: f64 = 1e-9;

/// A clock update set against a temperature: its time, unix seconds,
/// the temperature, and the clock's own frequency, ppm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pair {
    pub(crate) at: f64,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// A robust polynomial fit of the frequency against the temperature.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fit {
    /// 1 or 2.
    pub(crate) degree: usize,
    /// Its coefficients, constant first, x from T0.
    pub(crate) c: Vec<f64>,
    /// The RMS of what it leaves, ppm.
    pub(crate) left: f64,
    /// R^2, the share of the frequency's variance it accounts for; none
    /// where the frequency does not vary.
    pub(crate) determined: Option<f64>,
}

/// The fit of a range.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scatter {
    /// What is fitted, oldest first.
    pub(crate) pairs: Vec<Pair>,
    /// The mean temperature, rounded: x of the fits is from it.
    pub(crate) t0: f64,
    /// The frequency's own RMS about its mean, ppm.
    pub(crate) spread: f64,
    /// The linear fit, then the quadratic, those that could be made.
    pub(crate) fits: Vec<Fit>,
    /// Those that could not, and why: their degrees.
    pub(crate) unfitted: Vec<(usize, Unfitted)>,
}

/// Why a fit is not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unfitted {
    /// The temperatures fill only this many of `SPREAD_STRETCHES`
    /// stretches of their range: too few to fit a curve of the degree.
    Narrow(usize),
    /// Its normal equations are singular.
    Singular,
}

/// The root mean square of `values`.
fn rms(values: impl ExactSizeIterator<Item = f64>) -> f64 {
    let n = values.len() as f64;
    (values.map(|v| v * v).sum::<f64>() / n).sqrt()
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

fn evaluate(c: &[f64], x: f64) -> f64 {
    c.iter().rev().fold(0.0, |s, v| s * x + v)
}

/// The least squares polynomial of `degree` through `pairs`, x already
/// from T0, each pair weighted by `weights`' own, 1 if none: its
/// coefficients, constant first.  x is scaled to about 1 for the normal
/// equations, then the coefficients back.
fn polynomial(pairs: &[(f64, f64)], degree: usize, weights: Option<&[f64]>) -> Option<Vec<f64>> {
    let scale = pairs.iter().fold(0.0_f64, |m, (x, _)| m.max(x.abs()));
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let size = degree + 1;
    let mut sums = vec![0.0; 2 * size - 1];
    let mut y_sums = vec![0.0; size];
    for (n, (x, y)) in pairs.iter().enumerate() {
        let x = x / scale;
        let mut power = weights.map_or(1.0, |w| w[n]);
        for (k, sum) in sums.iter_mut().enumerate() {
            *sum += power;
            if k < size {
                y_sums[k] += power * y;
            }
            power *= x;
        }
    }
    let mut a: Vec<Vec<f64>> = (0..size)
        .map(|i| (0..size).map(|j| sums[i + j]).chain([y_sums[i]]).collect())
        .collect();
    // Gaussian elimination, with partial pivoting.
    for col in 0..size {
        let pivot = (col..size)
            .max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()).then(q.cmp(&p)))
            .expect("a row");
        a.swap(col, pivot);
        if a[col][col].is_nan() || a[col][col].abs() < SINGULAR * sums[0] {
            return None;
        }
        let (above, below) = a.split_at_mut(col + 1);
        let top = &above[col];
        for row in below {
            let f = row[col] / top[col];
            for (r, t) in row[col..].iter_mut().zip(&top[col..]) {
                *r -= f * t;
            }
        }
    }
    let mut c = vec![0.0; size];
    for row in (0..size).rev() {
        let s = (row + 1..size).fold(a[row][size], |s, k| s - a[row][k] * c[k]);
        c[row] = s / a[row][row];
    }
    Some(
        c.into_iter()
            .zip(0..)
            .map(|(v, n)| v / scale.powi(n))
            .collect(),
    )
}

/// The robust (Huber) polynomial of `degree` through `pairs`, x from
/// T0, by least squares reweighted until the weights settle: its
/// coefficients, constant first; or none.
fn robust_polynomial(pairs: &[(f64, f64)], degree: usize) -> Option<Vec<f64>> {
    let mut c = polynomial(pairs, degree, None)?;
    let mut scale: Option<f64> = None;
    for _ in 0..ROBUST_ROUNDS {
        let residuals: Vec<f64> = pairs.iter().map(|(x, y)| y - evaluate(&c, *x)).collect();
        let next = MAD_SIGMA * median(&residuals.iter().map(|r| r.abs()).collect::<Vec<_>>());
        // None, or not a number: as good as it gets.
        if next.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Some(c);
        }
        let settled = scale.is_some_and(|s| (next - s).abs() <= ROBUST_SETTLED * s);
        scale = Some(next);
        if settled {
            break;
        }
        let weights: Vec<f64> = residuals
            .iter()
            .map(|r| (HUBER * next / r.abs()).min(1.0))
            .collect();
        c = polynomial(pairs, degree, Some(&weights))?;
    }
    Some(c)
}

/// The usual interval of `at`, times in order: the median step.
fn usual_step(at: &[f64]) -> f64 {
    if at.len() < 2 {
        return 0.0;
    }
    median(&at.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>())
}

/// Each of `updates` set against the reading of `temperature`, read at
/// `at`, in force at it: the last at or before it, and no further back
/// than `GAP_READINGS` intervals.
fn updates_on(
    at: &[f64],
    temperature: &[f64],
    updates: &[ClockUpdate],
    interval: f64,
) -> Vec<Pair> {
    let mut found = Vec::new();
    let mut i: Option<usize> = None;
    for u in updates {
        while i.map_or(0, |i| i + 1) < at.len() && at[i.map_or(0, |i| i + 1)] <= u.at {
            i = Some(i.map_or(0, |i| i + 1));
        }
        let Some(i) = i else { continue };
        if u.at - at[i] > GAP_READINGS * interval {
            continue;
        }
        found.push(Pair {
            at: u.at,
            x: temperature[i],
            y: u.frequency,
        });
    }
    found
}

/// How many of `SPREAD_STRETCHES` equal stretches of `xs`' range, short
/// of its tails, hold `SPREAD_SHARE` of them.
fn stretches_filled(xs: &[f64]) -> usize {
    let mut sorted = xs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let tail = (SPREAD_TAIL * n as f64) as usize;
    let (lo, hi) = (sorted[tail], sorted[n - 1 - tail]);
    if hi <= lo {
        return 1;
    }
    let mut counts = [0_usize; SPREAD_STRETCHES];
    for x in sorted.iter().filter(|x| (lo..=hi).contains(*x)) {
        let stretch = ((x - lo) / (hi - lo) * SPREAD_STRETCHES as f64) as usize;
        counts[stretch.min(SPREAD_STRETCHES - 1)] += 1;
    }
    counts
        .iter()
        .filter(|count| **count as f64 >= SPREAD_SHARE * n as f64)
        .count()
}

/// The robust fits, linear and quadratic, of the frequency against the
/// temperature over `pairs`, from T0, their mean temperature rounded;
/// the frequency's own spread about its mean; and those not made, and
/// why.
fn fits_of(pairs: &[Pair]) -> (f64, f64, Vec<Fit>, Vec<(usize, Unfitted)>) {
    let n = pairs.len() as f64;
    // Half up, as JavaScript's Math.round, as the page did.
    let t0 = (pairs.iter().map(|p| p.x).sum::<f64>() / n + 0.5).floor();
    let from: Vec<(f64, f64)> = pairs.iter().map(|p| (p.x - t0, p.y)).collect();
    let mean = from.iter().map(|(_, y)| y).sum::<f64>() / n;
    let spread = rms(from.iter().map(|(_, y)| y - mean));
    let filled = stretches_filled(&from.iter().map(|(x, _)| *x).collect::<Vec<_>>());
    let mut fits = Vec::new();
    let mut unfitted = Vec::new();
    for degree in [1, 2] {
        if filled < degree + 2 {
            unfitted.push((degree, Unfitted::Narrow(filled)));
            continue;
        }
        let Some(c) = robust_polynomial(&from, degree) else {
            unfitted.push((degree, Unfitted::Singular));
            continue;
        };
        let left = rms(from.iter().map(|(x, y)| y - evaluate(&c, *x)));
        let determined = (spread > 0.0).then(|| 1.0 - (left / spread).powi(2));
        fits.push(Fit {
            degree,
            c,
            left,
            determined,
        });
    }
    (t0, spread, fits, unfitted)
}

/// The fit of the clock `updates` against the temperatures
/// `temperature` read at `at`; or none, with too few updates to set
/// against the readings.
pub(crate) fn scatter(at: &[f64], temperature: &[f64], updates: &[ClockUpdate]) -> Option<Scatter> {
    let pairs = updates_on(at, temperature, updates, usual_step(at));
    if pairs.len() < MIN_PAIRS {
        return None;
    }
    let (t0, spread, fits, unfitted) = fits_of(&pairs);
    Some(Scatter {
        pairs,
        t0,
        spread,
        fits,
        unfitted,
    })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    /// Two days of readings every 16 s, as the synthetic sensor's.
    const READ_EVERY: f64 = 16.0;
    const HOURS: f64 = 48.0;
    const DAY: f64 = 86_400.0;
    /// The clock's slope, ppm a millidegree, and its temperature's mean
    /// and daily swing, millidegrees.
    const SLOPE: f64 = 8.75e-5;
    const MEAN: f64 = 41_000.0;
    const SWING: f64 = 4_000.0;
    /// Seconds after each reading its clock update comes.
    const UPDATE_AFTER: f64 = 1.0;

    fn times() -> Vec<f64> {
        (0..(HOURS * 3600.0 / READ_EVERY) as usize)
            .map(|n| n as f64 * READ_EVERY)
            .collect()
    }

    fn daily(at: &[f64]) -> Vec<f64> {
        at.iter()
            .map(|t| MEAN + SWING * (TAU * t / DAY).sin())
            .collect()
    }

    /// An update just after each of the readings at `at`, the clock's
    /// own frequency `frequency`.
    fn updates(at: &[f64], frequency: &[f64]) -> Vec<ClockUpdate> {
        at.iter()
            .zip(frequency)
            .map(|(at, frequency)| ClockUpdate {
                at: at + UPDATE_AFTER,
                frequency: *frequency,
                pps: true,
            })
            .collect()
    }

    #[test]
    fn the_fits_find_the_clocks_line_and_curve() {
        let at = times();
        let air = daily(&at);
        let curve = 3e-8;
        let frequency: Vec<f64> = air
            .iter()
            .map(|t| -12.0 + SLOPE * (t - MEAN) + curve * (t - MEAN).powi(2))
            .collect();
        let fitted = scatter(&at, &air, &updates(&at, &frequency)).expect("a fit");
        assert_eq!(fitted.pairs.len(), at.len());
        let [linear, quadratic] = &fitted.fits[..] else {
            panic!("two fits")
        };
        assert_eq!((linear.degree, quadratic.degree), (1, 2));
        // From T0, the mean temperature: the curve's slope there.
        let slope_at_t0 = SLOPE + 2.0 * curve * (fitted.t0 - MEAN);
        assert!((quadratic.c[1] - slope_at_t0).abs() < 1e-9);
        assert!((quadratic.c[2] - curve).abs() < 1e-12);
        assert!(quadratic.left < 1e-9);
        assert!(quadratic.determined.expect("a spread") > linear.determined.expect("a spread"));
    }

    #[test]
    fn the_fit_is_robust_a_few_wild_readings_pulling_it_little() {
        // One update in twenty of the warmest 1 ppm off: least squares
        // would steepen the line by about a fifth.
        let at = times();
        let air = daily(&at);
        let frequency: Vec<f64> = air
            .iter()
            .enumerate()
            .map(|(i, t)| {
                SLOPE * (t - MEAN)
                    + if *t > MEAN + 2_000.0 && i % 20 == 0 {
                        1.0
                    } else {
                        0.0
                    }
            })
            .collect();
        let fitted = scatter(&at, &air, &updates(&at, &frequency)).expect("a fit");
        assert!((fitted.fits[0].c[1] / SLOPE - 1.0).abs() < 0.02);
    }

    #[test]
    fn an_update_is_set_against_the_reading_in_force_and_not_one_long_gone() {
        let at = [0.0, 16.0, 32.0, 48.0, 64.0];
        let temperature = [1.0, 2.0, 3.0, 4.0, 5.0];
        let at_update = |at: f64| ClockUpdate {
            at,
            frequency: 0.0,
            pps: true,
        };
        // Before any reading; at one; between two; long after the last.
        let fitted = scatter(
            &at,
            &temperature,
            &[
                at_update(-1.0),
                at_update(16.0),
                at_update(40.0),
                at_update(70.0),
                at_update(200.0),
            ],
        )
        .expect("a fit");
        let xs: Vec<f64> = fitted.pairs.iter().map(|p| p.x).collect();
        assert_eq!(xs, [2.0, 3.0, 5.0]);
    }

    #[test]
    fn two_temperatures_fit_no_line_and_three_no_curve() {
        // A hundred readings at each, as a sensor parked between two of
        // its steps; R^2 0.9998 for a curve through them, determined by
        // nothing.
        let at: Vec<f64> = (0..300).map(|n| f64::from(n) * READ_EVERY).collect();
        let levels = |count: usize| -> Vec<f64> {
            (0..at.len())
                .map(|i| MEAN + 100.0 * (i % count) as f64)
                .collect()
        };
        let two = levels(2);
        let fitted = scatter(&at, &two, &updates(&at, &vec![0.0; at.len()])).expect("pairs");
        assert!(fitted.fits.is_empty());
        assert_eq!(
            fitted.unfitted,
            [(1, Unfitted::Narrow(2)), (2, Unfitted::Narrow(2))]
        );
        let three = levels(3);
        let fitted = scatter(&at, &three, &updates(&at, &vec![0.0; at.len()])).expect("pairs");
        assert_eq!(
            fitted.fits.iter().map(|f| f.degree).collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(fitted.unfitted, [(2, Unfitted::Narrow(3))]);
    }

    #[test]
    fn a_spread_fills_every_stretch() {
        let at = times();
        assert_eq!(stretches_filled(&daily(&at)), SPREAD_STRETCHES);
        assert_eq!(stretches_filled(&[MEAN; 10]), 1);
    }

    #[test]
    fn too_few_updates_fit_nothing() {
        let at = times();
        let air = daily(&at);
        assert_eq!(scatter(&at, &air, &updates(&at[..2], &[0.0, 0.0])), None);
        assert_eq!(scatter(&[], &[], &updates(&at[..5], &[0.0; 5])), None);
    }
}
