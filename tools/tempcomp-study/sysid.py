"""System identification of how the clock's frequency follows its
temperature: output-error models, y = c + B(q)/F(q) [u (+ a u^2)],
fitted robustly, their orders and dead time chosen by blocked
cross-validation, against the static fit tempcomp's own line is.

Usage: python3 sysid.py HOST.npz...
"""

import sys

import numpy as np
from scipy.optimize import least_squares
from scipy.signal import lfilter, lfilter_zi

from common import HUBER, MAD_SIGMA, STEP, cross_validate, design, huber_fit, load

# Orders of B and F, and dead times in samples, tried.
B_ORDERS = (1, 2)
F_ORDERS = (1, 2)
DELAYS = (0, 1, 3, 6, 12)
# Minutes after a step of the temperature its response is reported at.
REPORT_AT = (5, 30, 120)
# Samples of a step response taken as its end.
SETTLED = 2000


def simulate(p, u, nb, nf, delay, quadratic):
    """The model's frequency from the temperature alone; p is c, B's
    coefficients, F's, then the quadratic's, started settled."""
    b, f = p[1: 1 + nb], p[1 + nb: 1 + nb + nf]
    x = u + (p[-1] * u * u if quadratic else 0)
    if delay:
        x = np.concatenate([np.full(delay, x[0]), x[:-delay]])
    den = np.concatenate([[1], f])
    return p[0] + lfilter(b, den, x, zi=lfilter_zi(b, den) * x[0])[0]


def fit(u, y, train, nb, nf, delay, quadratic):
    slope, intercept = np.polyfit(u[train], y[train], 1)
    p0 = np.concatenate([[intercept], [slope * 0.5 / nb] * nb, [-0.5] + [0] * (nf - 1), [0] if quadratic else []])
    residuals = lambda p: (y - simulate(p, u, nb, nf, delay, quadratic))[train]
    r = residuals(p0)
    scale = MAD_SIGMA * np.median(np.abs(r - np.median(r))) or 1e-9
    p = least_squares(residuals, p0, loss="huber", f_scale=HUBER * scale, max_nfev=400).x
    stable = np.all(np.abs(np.roots(np.concatenate([[1], p[1 + nb: 1 + nb + nf]]))) < 0.999)
    return p, stable


def step_response(p, nb, nf, delay):
    """The static gain, ppm/C, and the share of a step reached at each
    of REPORT_AT."""
    b, f = p[1: 1 + nb], p[1 + nb: 1 + nb + nf]
    step = lfilter(b, np.concatenate([[1], f]), np.concatenate([np.zeros(delay), np.ones(SETTLED)]))
    return b.sum() / (1 + f.sum()), [step[(m * 60) // STEP] / step[-1] for m in REPORT_AT]


def main():
    for path in sys.argv[1:]:
        t, u, y, good = load(path)
        print(f"=== {path}: {(t[-1] - t[0]) / 86400:.1f} days")
        for quadratic in (False, True):
            name = "quadratic" if quadratic else "linear"
            x = design(u, quadratic)
            static = cross_validate(lambda train: x @ huber_fit(x[train], y[train]), good, y)
            best = None
            for nb in B_ORDERS:
                for nf in F_ORDERS:
                    for delay in DELAYS:
                        model = (nb, nf, delay, quadratic)
                        score = cross_validate(lambda train: simulate(fit(u, y, train, *model)[0], u, *model),
                                               good, y)
                        p, stable = fit(u, y, good, *model)
                        if stable and (best is None or score < best[0]):
                            best = (score, p, nb, nf, delay)
            score, p, nb, nf, delay = best
            gain, shares = step_response(p, nb, nf, delay)
            print(f" {name:9} static held-out RMSE {static:.4f} ppm; best model nb {nb} nf {nf} "
                  f"dead time {delay * STEP // 60} min: {score:.4f}, gain {gain:+.4f} ppm/C, step reached at "
                  + ", ".join(f"{m} min {s:.2f}" for m, s in zip(REPORT_AT, shares)))


if __name__ == "__main__":
    main()
