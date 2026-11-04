"""What the studies share: a host's readings, the lag filter, robust
fits and blocked cross-validation.

y is the clock's own frequency (chronyd's frequency less tempcomp's
compensation), in ppm; u the temperature in degrees C, from its mean.
"""

import numpy as np
from scipy.optimize import least_squares
from scipy.signal import lfilter

# Seconds a model's sample spans: readings are averaged to it.
STEP = 300
# The frequency is interpolated across gaps up to this many seconds:
# an NTP-only chronyd updates it every few minutes.
GAP = 1800
# Cross-validation folds, each a contiguous stretch of time, and the
# samples either side of a held-out fold also kept out of its fit.
BLOCKS = 6
MARGIN = 12
# Huber's constant, in robust standard deviations, and the median
# absolute deviation to the standard deviation for normal noise.
HUBER = 1.345
MAD_SIGMA = 1.4826


def load(path):
    """A fetched host's readings, `fetch.py`'s .npz: the sample times,
    u, y and which samples have both."""
    d = np.load(path)
    at, temperature, compensation, frequency = d["at"], d["temperature"], d["compensation"], d["frequency"]
    known = ~np.isnan(frequency)
    filled = np.interp(at, at[known], frequency[known])
    times = at[known]
    j = np.clip(np.searchsorted(times, at), 1, len(times) - 1)
    filled[~known & (times[j] - times[j - 1] > GAP)] = np.nan
    y = filled - compensation
    u = temperature / 1000.0
    per = int(STEP // np.median(np.diff(at)))
    n = len(at) // per

    def mean(v):
        with np.errstate(all="ignore"):
            return np.nanmean(v[: n * per].reshape(n, per), 1)

    t, u, y = mean(at), mean(u), mean(y)
    good = ~np.isnan(u) & ~np.isnan(y)
    # The filter needs the temperature throughout.
    u = np.interp(t, t[~np.isnan(u)], u[~np.isnan(u)])
    return t, u - u[good].mean(), y, good


def lag(u, tau):
    """u through a first-order lag of time constant `tau` seconds,
    started settled at the first reading."""
    if tau <= 0:
        return u
    a = np.exp(-STEP / tau)
    return lfilter([1 - a], [1, -a], u, zi=[a * u[0]])[0]


def design(x, quadratic):
    return np.column_stack([np.ones_like(x), x] + ([x * x] if quadratic else []))


def huber_fit(X, y):
    """Robust (Huber) least squares coefficients."""
    beta = np.linalg.lstsq(X, y, rcond=None)[0]
    r = y - X @ beta
    scale = MAD_SIGMA * np.median(np.abs(r - np.median(r))) or 1e-9
    return least_squares(lambda b: y - X @ b, beta, loss="huber", f_scale=HUBER * scale).x


def cross_validate(predict, good, y):
    """Held-out RMSE: `predict(train)` fits on the samples `train`
    marks and predicts every sample from the temperature alone; each
    fold is held out in turn."""
    errors = []
    for test in np.array_split(np.flatnonzero(good), BLOCKS):
        train = good.copy()
        train[max(0, test[0] - MARGIN): test[-1] + MARGIN + 1] = False
        errors.append(y[test] - predict(train)[test])
    e = np.concatenate(errors)
    return float(np.sqrt(np.mean(e ** 2)))
