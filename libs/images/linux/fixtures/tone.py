#!/usr/bin/env python3
"""Audio fixture: loop a known tone sequence into the default sink.

Sequence (one cycle = 1.5 s), 48 kHz stereo s16le at half scale:
    440 Hz 500 ms, silence 250 ms, 880 Hz 500 ms, silence 250 ms

A client that captures desktop audio (cua_desktop.monitor) can verify
frequency by FFT and timing against the JSONL log: one "segment" record per
segment with its frequency, duration and the monotonic/wall time it was handed
to the server (plays ~20 ms later).

Environment: CUA_FIXTURE_NAME (default "tone"), CUA_FIXTURE_AUDIO_SINK
(default: the server's default sink), CUA_TONE_CYCLES (0 = forever).
"""

from __future__ import annotations

import os
import signal
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fixtureaudio import CHANNELS, RATE, PacedPlayer, silence, tone  # noqa: E402
from fixturelog import FixtureLog  # noqa: E402

NAME = os.environ.get("CUA_FIXTURE_NAME", "tone")
CYCLES = int(os.environ.get("CUA_TONE_CYCLES", "0"))
SEQUENCE = [(440, 500), (0, 250), (880, 500), (0, 250)]


def main() -> int:
    log = FixtureLog(NAME)
    pcm = [(f, ms, tone(f, ms) if f else silence(ms)) for f, ms in SEQUENCE]
    player = PacedPlayer(NAME)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    log.emit("ready", pid=os.getpid(), rate=RATE, channels=CHANNELS, sequence=[[f, ms] for f, ms in SEQUENCE])
    cycle = 0
    try:
        while CYCLES == 0 or cycle < CYCLES:
            for freq, ms, data in pcm:
                mono = player.play(data)
                log.emit("segment", cycle=cycle, freq_hz=freq, ms=ms, start_mono=round(mono, 6),
                         start_wall=round(time.time() - (time.monotonic() - mono), 6))
            cycle += 1
    finally:
        log.emit("exit", cycles=cycle)
        player.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
