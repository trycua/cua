"""PCM helpers for the audio fixtures (stdlib only).

Audio goes to the session's PulseAudio-compatible server (pipewire-pulse)
through `pacat`, as 48 kHz stereo s16le. Writes are paced against the
monotonic clock in small chunks so almost nothing sits in the pipe: the time a
chunk is written is (within the stream's ~20 ms latency) the time it plays,
which is what the A/V sync fixture's timestamps rely on.
"""

from __future__ import annotations

import math
import os
import struct
import subprocess
import time

RATE = 48000
CHANNELS = 2
CHUNK_MS = 10
FRAMES_PER_CHUNK = RATE * CHUNK_MS // 1000
AMPLITUDE = 0.5


def tone(freq_hz: float, ms: int, phase0: int = 0) -> bytes:
    """ms of a sine at freq_hz, stereo s16le. phase0 = starting sample index."""
    n = RATE * ms // 1000
    out = bytearray()
    for i in range(n):
        v = int(32767 * AMPLITUDE * math.sin(2 * math.pi * freq_hz * (phase0 + i) / RATE))
        out += struct.pack("<hh", v, v)
    return bytes(out)


def silence(ms: int) -> bytes:
    return bytes(RATE * ms // 1000 * CHANNELS * 2)


class PacedPlayer:
    """A pacat playback stream fed in CHUNK_MS chunks on the monotonic clock."""

    def __init__(self, name: str, device: str | None = None) -> None:
        cmd = [
            "pacat",
            "--playback",
            "--raw",
            "--format=s16le",
            f"--rate={RATE}",
            f"--channels={CHANNELS}",
            "--latency-msec=20",
            f"--client-name=cua-fixture-{name}",
            f"--stream-name=cua-fixture-{name}",
        ]
        device = device or os.environ.get("CUA_FIXTURE_AUDIO_SINK")
        if device:
            cmd.append(f"--device={device}")
        self.proc = subprocess.Popen(cmd, stdin=subprocess.PIPE)
        self.t0 = time.monotonic()
        self.frames = 0

    def play(self, pcm: bytes, on_start=None) -> float:
        """Queue pcm, pacing each chunk to real time. Returns the monotonic
        time at which the first chunk was handed to the server; on_start (if
        given) is called at that same instant, just before the write."""
        step = FRAMES_PER_CHUNK * CHANNELS * 2
        first = None
        for off in range(0, len(pcm), step):
            due = self.t0 + self.frames / RATE
            delay = due - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            elif delay < -0.2:  # fell behind (e.g. stream stalled): resync, don't burst
                self.t0 = time.monotonic() - self.frames / RATE
            if first is None:
                first = time.monotonic()
                if on_start is not None:
                    on_start()
            chunk = pcm[off : off + step]
            self.proc.stdin.write(chunk)
            self.proc.stdin.flush()
            self.frames += len(chunk) // (CHANNELS * 2)
        return first if first is not None else time.monotonic()

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        finally:
            self.proc.wait(timeout=5)
