"""The Temperature page's lag, as a reference: a first-order lag of the
temperature, robust fits, the lag chosen by blocked cross-validation
from a fixed set and compared with none.

Usage: python3 lag.py HOST.npz...
"""

import sys

import numpy as np

from common import cross_validate, design, huber_fit, lag, load

# Lags tried, in minutes.
LAGS = [0, 5, 10, 20, 30, 60, 90, 120, 180, 240]


def main():
    for path in sys.argv[1:]:
        t, u, y, good = load(path)
        print(f"=== {path}: {(t[-1] - t[0]) / 86400:.1f} days")
        for quadratic in (False, True):
            scores = []
            for minutes in LAGS:
                x = design(lag(u, minutes * 60), quadratic)
                scores.append(cross_validate(lambda train, x=x: x @ huber_fit(x[train], y[train]), good, y))
            best = int(np.argmin(scores))
            x = design(lag(u, LAGS[best] * 60), quadratic)
            k1 = huber_fit(x[good], y[good])[1]
            name = "quadratic" if quadratic else "linear"
            print(f" {name:9} held-out RMSE, ppm, by lag in min: "
                  + "  ".join(f"{m}:{s:.4f}" for m, s in zip(LAGS, scores)))
            print(f" {name:9} best {LAGS[best]} min: {scores[best]:.4f} against {scores[0]:.4f} with none, "
                  f"k1 {k1:+.4f} ppm/C")


if __name__ == "__main__":
    main()
