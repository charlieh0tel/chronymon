"""Fetch a host's temperature, compensation and frequency from
chronymon-web's API, in one-minute buckets, into <host>.npz.

Usage: python3 fetch.py [--days N] HOST[:PORT]...
"""

import argparse
import json
import urllib.parse
import urllib.request

import numpy as np

# Seconds a bucket spans, and buckets asked for at a time (the API
# gives at most 5000).
BUCKET = 60
CHUNK = 4800
PORT = 8323
TIMEOUT = 120


def ask(base, path, query=None):
    url = f"{base}{path}" + ("?" + urllib.parse.urlencode(query) if query else "")
    with urllib.request.urlopen(url, timeout=TIMEOUT) as answer:
        return json.load(answer)


def column(got, name, table=None):
    return next(p["mean"] for p in got["plots"]
                if p["column"] == name and (table is None or p["table"] == table))


def fetch(host, days):
    base = f"http://{host if ':' in host else f'{host}:{PORT}'}"
    # Readings since the sensor last changed only: before, another's.
    since = (ask(base, "/api/status").get("tempcomp") or {}).get("since") or 0
    probe = ask(base, "/api/history", {"table": "tempcomp_sample", "columns": "temperature", "points": 16})
    last = probe["last"]
    first = max(probe["first"], last - days * 86400, since)
    at, temperature, compensation, frequency = [], [], [], []
    start = first
    while start < last:
        end = min(last, start + CHUNK * BUCKET)
        points = int((end - start) // BUCKET)
        sensor = ask(base, "/api/history", {"table": "tempcomp_sample", "columns": "temperature,compensation",
                                            "from": int(start), "to": int(end), "points": points})
        clock = ask(base, "/api/history", {"table": "tracking", "columns": "frequency", "fill": "log_tracking",
                                           "fill_columns": "frequency", "from": sensor["from"],
                                           "to": sensor["to"], "points": points})
        reported = column(clock, "frequency", "tracking")
        logged = next((p["mean"] for p in clock["plots"] if p["table"] == "log_tracking"), [None] * len(reported))
        bucket = lambda t: int((t - sensor["from"]) // BUCKET)
        by_bucket = {bucket(t): r if r is not None else g for t, r, g in zip(clock["at"], reported, logged)}
        at += sensor["at"]
        temperature += column(sensor, "temperature")
        compensation += column(sensor, "compensation")
        frequency += [by_bucket.get(bucket(t)) for t in sensor["at"]]
        start = end
    array = lambda xs: np.array([np.nan if x is None else x for x in xs], float)
    name = host.split(":")[0].split(".")[0]
    np.savez(f"{name}.npz", at=array(at), temperature=array(temperature),
             compensation=array(compensation), frequency=array(frequency))
    print(f"{name}.npz: {(last - first) / 86400:.1f} days, {len(at)} buckets")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--days", type=float, default=14, help="most days back to fetch (14)")
    parser.add_argument("hosts", nargs="+")
    args = parser.parse_args()
    for host in args.hosts:
        fetch(host, args.days)


if __name__ == "__main__":
    main()
