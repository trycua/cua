#!/usr/bin/env python3
"""Record from a Pulse source and report what is in it (stdlib only).

  audio_probe.py [--source cua_desktop.monitor] [--seconds 3] [--freq 440 --freq 880 ...]

Records mono s16le at 48 kHz with `parec`, then prints one JSON object:
  rms_dbfs      overall level (-inf for digital silence)
  tones         per requested frequency: relative power (Goertzel) of the
                strongest 50 ms window, and the fraction of windows where that
                frequency dominates the others requested
  windows       number of 50 ms windows analysed
Exit status 0 if the capture is non-silent (rms above --min-dbfs), else 1.
Used by smoke-test.sh (desktop capture via the null sink's monitor, uplink via
cua_mic) and by conformance tests that need an in-guest audio oracle.
"""

from __future__ import annotations

import argparse
import array
import json
import math
import subprocess
import sys

RATE = 48000


def goertzel(samples: array.array, start: int, n: int, freq: float) -> float:
    k = 2 * math.cos(2 * math.pi * freq / RATE)
    s1 = s2 = 0.0
    for i in range(start, start + n):
        s0 = samples[i] + k * s1 - s2
        s2, s1 = s1, s0
    return (s1 * s1 + s2 * s2 - k * s1 * s2) / (n * n)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--source", default="cua_desktop.monitor")
    ap.add_argument("--seconds", type=float, default=3.0)
    ap.add_argument("--freq", type=float, action="append", default=[])
    ap.add_argument("--min-dbfs", type=float, default=-50.0)
    a = ap.parse_args()

    nbytes = int(a.seconds * RATE) * 2
    proc = subprocess.Popen(
        ["parec", f"--device={a.source}", "--raw", "--format=s16le", f"--rate={RATE}", "--channels=1",
         "--latency-msec=20"],
        stdout=subprocess.PIPE,
    )
    data = bytearray()
    while len(data) < nbytes:
        chunk = proc.stdout.read(nbytes - len(data))
        if not chunk:
            break
        data += chunk
    proc.terminate()
    proc.wait(timeout=5)

    samples = array.array("h")
    samples.frombytes(bytes(data[: len(data) // 2 * 2]))
    n = len(samples)
    rms = math.sqrt(sum(s * s for s in samples) / n) if n else 0.0
    dbfs = 20 * math.log10(rms / 32768) if rms > 0 else float("-inf")

    win = RATE // 20
    windows = n // win
    tones = {}
    if a.freq and windows:
        powers = [[goertzel(samples, w * win, win, f) for f in a.freq] for w in range(windows)]
        for j, f in enumerate(a.freq):
            peak = max(p[j] for p in powers)
            dominant = sum(1 for p in powers if p[j] == max(p) and p[j] > 1e3) / windows
            tones[str(int(f))] = {"peak_power": round(peak, 1), "dominant_fraction": round(dominant, 3)}
    out = {
        "source": a.source,
        "seconds": round(n / RATE, 3),
        "rms_dbfs": round(dbfs, 2) if dbfs != float("-inf") else None,
        "windows": windows,
        "tones": tones,
    }
    print(json.dumps(out, sort_keys=True))
    return 0 if dbfs > a.min_dbfs else 1


if __name__ == "__main__":
    sys.exit(main())
