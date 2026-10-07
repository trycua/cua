#!/usr/bin/env python3
"""Streaming example for the cua Python SDK (see ../SCENARIO.md).

Connects to a cua-spacesd, starts the grid fixture, lists targets,
streams the desktop and the grid window (decoded BGRA frames + PCM audio
through ``SpacesdClient.open_media_decoded_with_audio``), clicks a grid cell
with a media-plane ``action`` and prints one ``SUMMARY {...}`` line.

With ``CUA_BENCH_JSONL`` set it runs as a benchmark lane instead: it
streams only ``CUA_BENCH_TARGET`` for ``CUA_BENCH_SECONDS`` and appends the
per-frame JSONL described in SCENARIO.md (with ``tc_ms`` decoded from the
bench fixture's timecode strip).

Only the standard library plus the ``cua`` binding is used. Window mode
(``CUA_HEADLESS=0``) renders the video with tkinter when it is available
and otherwise falls back to headless behaviour.
"""

from __future__ import annotations

import array
import asyncio
import json
import os
import queue
import resource
import sys
import threading
import time
import uuid
import wave
from pathlib import Path
from typing import Any, Optional

import cua

EXAMPLE = "python"
GRID_TITLE = "CUA Fixture Grid"
GRID_LOG = "/tmp/cua-fixtures/grid.jsonl"
CLICK_CELL = (2, 3)
CLICK_CONTENT_PX = (200, 280)
CELL_RGB = (72, 153, 128)
CELL_TOLERANCE = 24
MAX_FPS = 30

# Hard bounds for every wait/poll loop in this file.
TARGET_POLL_ATTEMPTS = 20  # x 0.5 s
LOG_POLL_ATTEMPTS = 20  # x 0.25 s
ACTION_RESULT_WAIT_S = 3.0


def log(msg: str) -> None:
    print(msg, flush=True)


def fnv1a64(data: bytes) -> str:
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{h:016x}"


# --------------------------------------------------------------- timecode


def decode_timecode(frame: Any, scale: float, now_unix_ns: int) -> Optional[int]:
    """Reads the bench fixture's 48-cell strip at frame pixel (0, 0).

    Returns the full unix ms (rebuilt from the client clock) or None when a
    sync pattern or the checksum does not match.
    """
    data, stride = frame.data, frame.stride
    width, height = frame.width, frame.height
    bits = []
    for i in range(48):
        total = 0
        count = 0
        for ky in range(6, 10):
            y = int(ky * scale)
            if y >= height:
                return None
            row = y * stride
            for kx in range(6, 10):
                x = int((16 * i + kx) * scale)
                if x >= width:
                    return None
                o = row + 4 * x
                total += data[o] + data[o + 1] + data[o + 2]
                count += 3
        bits.append(1 if total > 128 * count else 0)
    if bits[0:4] != [1, 0, 1, 0] or bits[44:48] != [0, 1, 0, 1]:
        return None
    value = 0
    for b in bits[4:36]:
        value = (value << 1) | b
    check = 0
    for b in bits[36:44]:
        check = (check << 1) | b
    vb = value.to_bytes(4, "big")
    if vb[0] ^ vb[1] ^ vb[2] ^ vb[3] != check:
        return None
    client_ms = now_unix_ns // 1_000_000
    base = client_ms - (client_ms % (1 << 32))
    best = None
    for cand in (base - (1 << 32) + value, base + value, base + (1 << 32) + value):
        if best is None or abs(cand - client_ms) < abs(best - client_ms):
            best = cand
    return best


# ------------------------------------------------------------------ sinks


class StreamSink(cua.DecodedFrameSink, cua.PcmSink):
    """Collects decoded frames/PCM on the SDK's delivery thread.

    Callbacks must return quickly: they only count, keep the latest frame,
    append PCM and (bench mode) write one JSONL line.
    """

    def __init__(
        self,
        name: str,
        jsonl: Optional[Any] = None,
        tc_scale: Optional[float] = None,
        viewer: Optional["Viewer"] = None,
    ) -> None:
        self.name = name
        self.lock = threading.Lock()
        self.jsonl = jsonl
        self.tc_scale = tc_scale
        self.viewer = viewer
        self.frames = 0
        self.first_frame_ns: Optional[int] = None
        self.last_frame: Any = None
        self.geometry_epoch: Optional[int] = None
        self.session_opened: Optional[dict] = None
        self.pcm_packets = 0
        self.sample_rate: Optional[int] = None
        self.channels: Optional[int] = None
        self.pcm = array.array("h")
        self.events: list[tuple[str, str]] = []
        self.action_results: dict[str, dict] = {}
        self.decode_errors = 0
        self.tc_decoded = 0
        self.closed = False

    # DecodedFrameSink
    def on_decoded_frame(self, frame: Any) -> None:
        now = time.time_ns()
        line = None
        if self.jsonl is not None:
            tc = None
            if self.tc_scale is not None:
                try:
                    tc = decode_timecode(frame, self.tc_scale, now)
                except Exception:  # never let a bad frame kill the thread
                    tc = None
            line = {
                "t": "frame",
                "unix_ns": now,
                "seq": frame.sequence,
                # Not exposed by the decoded path (see README "SDK gaps").
                "bytes": None,
                "key": None,
                "cap_us": frame.capture_timestamp_us,
                "w": frame.width,
                "h": frame.height,
                "tc_ms": tc,
            }
        with self.lock:
            self.frames += 1
            if self.first_frame_ns is None:
                self.first_frame_ns = now
            self.last_frame = frame
            self.geometry_epoch = frame.geometry_epoch
            if line is not None:
                if line["tc_ms"] is not None:
                    self.tc_decoded += 1
                self.jsonl.write(json.dumps(line, separators=(",", ":")) + "\n")
        if self.viewer is not None:
            self.viewer.offer(frame)

    def on_event(self, event: Any) -> None:
        with self.lock:
            if len(self.events) < 1000:
                self.events.append((event.kind, event.json))
            if event.kind == "decode_error":
                self.decode_errors += 1
            elif event.kind == "closed":
                self.closed = True
            try:
                msg = json.loads(event.json)
            except ValueError:
                return
            payload = msg.get("payload") if isinstance(msg, dict) else None
            if not isinstance(payload, dict):
                return
            if event.kind == "session_opened":
                self.session_opened = payload
                if self.geometry_epoch is None:
                    self.geometry_epoch = payload.get("geometry_epoch")
            elif event.kind == "action_result":
                self.action_results[str(payload.get("action_id"))] = payload

    # PcmSink
    def on_pcm(self, audio: Any) -> None:
        now = time.time_ns()
        with self.lock:
            self.pcm_packets += 1
            self.sample_rate = audio.sample_rate
            self.channels = audio.channels
            if self.jsonl is None:
                self.pcm.extend(audio.samples)
            else:
                ch = max(1, audio.channels)
                self.jsonl.write(
                    json.dumps(
                        {
                            "t": "audio",
                            "unix_ns": now,
                            "pts_us": audio.pts_us,
                            # Encoded packet size is not exposed by the
                            # decoded path; see README "SDK gaps".
                            "bytes": None,
                            "samples": len(audio.samples) // ch,
                        },
                        separators=(",", ":"),
                    )
                    + "\n"
                )

    def snapshot_frame(self) -> Any:
        with self.lock:
            return self.last_frame


# ----------------------------------------------------------------- window


class Viewer:
    """Optional tkinter renderer (window mode). Frames are handed over
    through a 1-slot queue; tkinter runs on the main thread."""

    def __init__(self) -> None:
        import tkinter  # noqa: F401  (import check only)

        self.slot: "queue.Queue[Any]" = queue.Queue(maxsize=1)

    def offer(self, frame: Any) -> None:
        try:
            self.slot.get_nowait()
        except queue.Empty:
            pass
        try:
            self.slot.put_nowait(frame)
        except queue.Full:
            pass

    def run_for(self, title: str, seconds: float) -> None:
        import tkinter

        root = tkinter.Tk()
        root.title(title)
        label = tkinter.Label(root)
        label.pack()
        deadline = time.monotonic() + seconds
        holder: dict[str, Any] = {}

        def tick() -> None:
            if time.monotonic() >= deadline:
                root.destroy()
                return
            try:
                frame = self.slot.get_nowait()
            except queue.Empty:
                frame = None
            if frame is not None:
                rgb = bytearray(frame.width * frame.height * 3)
                src = frame.data
                rgb[0::3] = src[2::4]
                rgb[1::3] = src[1::4]
                rgb[2::3] = src[0::4]
                ppm = b"P6 %d %d 255\n" % (frame.width, frame.height) + bytes(rgb)
                img = tkinter.PhotoImage(data=ppm, format="PPM")
                holder["img"] = img
                label.configure(image=img)
            root.after(15, tick)

        root.after(15, tick)
        root.mainloop()


# -------------------------------------------------------------- env helpers


async def list_targets(env: Any) -> list[dict]:
    raw = await env.call_json(
        "/cua.env.v1.StreamService/ListTargets", json.dumps({"includeWindows": True})
    )
    return json.loads(raw).get("targets", [])


def describe(t: dict) -> dict:
    """Normalises one StreamTarget (proto3 JSON) into a flat dict."""
    if "display" in t:
        d = t["display"]
        b = d.get("bounds", {})
        size = d.get("nativeSize") or d.get("native_size") or {}
        return {
            "kind": "display",
            "id": d.get("id", ""),
            "title": d.get("name", ""),
            "primary": bool(d.get("primary")),
            "x": float(b.get("x", 0)),
            "y": float(b.get("y", 0)),
            "width": float(b.get("width", size.get("width", 0))),
            "height": float(b.get("height", size.get("height", 0))),
            "available": bool(t.get("available")),
        }
    w = t.get("window", {})
    b = w.get("bounds", {})
    return {
        "kind": "window",
        "id": (w.get("ref") or {}).get("id", ""),
        "title": w.get("title", ""),
        "primary": False,
        "x": float(b.get("x", 0)),
        "y": float(b.get("y", 0)),
        "width": float(b.get("width", 0)),
        "height": float(b.get("height", 0)),
        "available": bool(t.get("available")),
    }


async def find_window(env: Any, title: str) -> Optional[dict]:
    for _ in range(TARGET_POLL_ATTEMPTS):
        for t in map(describe, await list_targets(env)):
            if t["kind"] == "window" and t["title"] == title:
                return t
        await asyncio.sleep(0.5)
    return None


def write_wav(path: Path, sink: StreamSink) -> Optional[str]:
    if not sink.sample_rate:
        return None
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(sink.channels or 1)
        w.setsampwidth(2)
        w.setframerate(sink.sample_rate)
        pcm = sink.pcm
        if sys.byteorder != "little":
            pcm = array.array("h", pcm)
            pcm.byteswap()
        w.writeframes(pcm.tobytes())
    return str(path)


# ----------------------------------------------------------------- streams


async def open_stream(
    env: Any, target: dict, sink: StreamSink, audio: bool, interactive: bool = False
) -> Any:
    opts = cua.MediaOpenOptions(max_fps=MAX_FPS, audio=audio)
    if interactive:
        # Sessions are view-only by default; media-plane actions need input.
        opts.request_json = json.dumps({"policy": "SESSION_POLICY_ALLOW_ACTIVATION"})
    if target["kind"] == "window":
        opts.window_handle = target["id"]
    else:
        opts.display = target["id"] or "primary"
    return await env.open_media_decoded_with_audio(opts, sink, sink)


async def stream_step(
    env: Any,
    target: dict,
    name: str,
    seconds: float,
    out_dir: Path,
    viewer: Optional[Viewer],
    during=None,
) -> tuple[dict, StreamSink]:
    sink = StreamSink(name, viewer=viewer)
    t0 = time.time_ns()
    session = await open_stream(env, target, sink, audio=True, interactive=during is not None)
    log(f"[{name}] session {session.session_id()} codec={session.codec()}")
    extra = None
    try:
        start = time.monotonic()
        if during is not None:
            extra = await during(session, sink)
        remaining = max(0.0, seconds - (time.monotonic() - start))
        if viewer is not None:
            # tkinter owns the main thread; SDK callbacks keep running on
            # the session's delivery thread meanwhile.
            viewer.run_for(f"cua {name}", remaining)
        else:
            await asyncio.sleep(remaining)
        stats = session.stats()
    finally:
        await session.close()
    elapsed = (time.time_ns() - t0) / 1e9
    with sink.lock:
        frames = sink.frames
        first = sink.first_frame_ns
        last = sink.last_frame
    first_ms = round((first - t0) / 1e6, 1) if first else None
    span = (time.time_ns() - first) / 1e9 if first else 0.0
    fps = round(frames / span, 1) if span > 0 else 0.0
    last_hash = fnv1a64(last.data) if last is not None else None
    wav = write_wav(out_dir / f"{name}.wav", sink)
    summary = {
        "frames": frames,
        # Keyframe flags and encoded sizes are not surfaced by the decoded
        # path; see README "SDK gaps".
        "keyframes": None,
        "bytes": None,
        "encoded_frames": stats.frames,
        "frames_dropped": stats.frames_dropped,
        "decode_errors": sink.decode_errors,
        "first_frame_ms": first_ms,
        "fps": fps,
        "audio_packets": stats.audio_packets,
        "pcm_frames": sink.pcm_packets,
        "last_hash": last_hash,
        "hash_of": "decoded_bgra",
        "size": [last.width, last.height] if last is not None else None,
        "wav": wav,
        "seconds": round(elapsed, 2),
    }
    log(f"[{name}] {json.dumps(summary)}")
    if extra is not None:
        summary["_extra"] = extra
    return summary, sink


async def grid_log_lines(env: Any) -> list[str]:
    out = await env.sh(f"cat {GRID_LOG} 2>/dev/null || true", 10_000)
    return out.stdout.decode(errors="replace").splitlines()


def is_cell_press(line: str) -> bool:
    try:
        rec = json.loads(line)
    except ValueError:
        return False
    if not isinstance(rec, dict):
        return False
    kind = rec.get("event") or rec.get("kind") or rec.get("type")
    return kind == "button_press" and list(rec.get("cell") or []) == list(CLICK_CELL)


def pixel_rgb(frame: Any, x: int, y: int) -> Optional[tuple[int, int, int]]:
    if frame is None or not (0 <= x < frame.width and 0 <= y < frame.height):
        return None
    o = y * frame.stride + 4 * x
    b, g, r = frame.data[o], frame.data[o + 1], frame.data[o + 2]
    return (r, g, b)


async def click_during(env: Any, window: dict):
    async def run(session: Any, sink: StreamSink) -> dict:
        # Wait (bounded) for a first frame so the basis is real.
        for _ in range(100):
            if sink.snapshot_frame() is not None:
                break
            await asyncio.sleep(0.05)
        frame = sink.snapshot_frame()
        before = len(await grid_log_lines(env))
        result: dict[str, Any] = {"sent": False, "via": None, "logged": False, "pixel_ok": False}
        if frame is None:
            return result
        scale = frame.width / window["width"] if window["width"] else 1.0
        fx = int(round(CLICK_CONTENT_PX[0] * scale))
        fy = int(round(CLICK_CONTENT_PX[1] * scale))
        with sink.lock:
            epoch = sink.geometry_epoch or frame.geometry_epoch
            seq = sink.last_frame.sequence
        action_id = f"py-{uuid.uuid4().hex[:12]}"
        msg = {
            "type": "action",
            "payload": {
                "action_id": action_id,
                "session_id": session.session_id(),
                "tool": "click",
                "arguments": {"x": fx, "y": fy},
                "basis": {"kind": "pixel", "geometry_epoch": epoch, "frame_sequence": seq},
            },
        }
        session.send_control(json.dumps(msg))
        result.update(sent=True, via="action", frame_point=[fx, fy])
        deadline = time.monotonic() + ACTION_RESULT_WAIT_S
        ar = None
        while time.monotonic() < deadline:
            with sink.lock:
                ar = sink.action_results.get(action_id)
            if ar is not None:
                break
            await asyncio.sleep(0.05)
        result["action_result"] = ar
        if ar is None or not ar.get("delivered"):
            # Fallback: env click API in screen coordinates.
            sx = window["x"] + CLICK_CONTENT_PX[0]
            sy = window["y"] + CLICK_CONTENT_PX[1]
            await env.click(sx, sy)
            result["via"] = "env_click"
            result["screen_point"] = [sx, sy]
        for _ in range(LOG_POLL_ATTEMPTS):
            lines = await grid_log_lines(env)
            if any(is_cell_press(line) for line in lines[before:]):
                result["logged"] = True
                break
            await asyncio.sleep(0.25)
        await asyncio.sleep(0.5)  # let a post-click frame arrive
        rgb = pixel_rgb(sink.snapshot_frame(), fx, fy)
        result["pixel"] = list(rgb) if rgb else None
        result["pixel_ok"] = bool(
            rgb and all(abs(a - b) <= CELL_TOLERANCE for a, b in zip(rgb, CELL_RGB))
        )
        return result

    return run


# -------------------------------------------------------------------- modes


async def connect(url: str, token: str) -> Any:
    c = cua.embedded(fleet_from_env=False)
    return await c.spacesd(url, token)


async def scenario(url: str, token: str) -> int:
    seconds = float(os.environ.get("CUA_STREAM_SECONDS", "5"))
    headless = os.environ.get("CUA_HEADLESS", "1") != "0"
    out_dir = Path(os.environ.get("CUA_OUT_DIR", "./out"))
    viewer = None
    if not headless:
        try:
            viewer = Viewer()
        except Exception as e:  # no tkinter: behave headlessly
            log(f"window mode unavailable ({e}); running headless")

    env = await connect(url, token)
    log(f"health: {await env.health()}")

    out = await env.sh("cua-fixtures start grid", 30_000)
    log(f"cua-fixtures start grid -> exit {out.exit.code}: {out.stdout.decode().strip()}")

    window = await find_window(env, GRID_TITLE)
    targets = [describe(t) for t in await list_targets(env)]
    for t in targets:
        log(
            f"target {t['kind']:<7} id={t['id']} title={t['title']!r} "
            f"size={t['width']:.0f}x{t['height']:.0f} available={t['available']}"
        )
    displays = [t for t in targets if t["kind"] == "display"]
    display = next((t for t in displays if t["primary"]), displays[0] if displays else None)
    if display is None:
        log("no display target")
        return 1
    if window is None:
        log(f"window {GRID_TITLE!r} not found")
        return 1

    desktop, _ = await stream_step(env, display, "desktop", seconds, out_dir, viewer)
    win, _ = await stream_step(
        env, window, "window", seconds, out_dir, viewer, during=await click_during(env, window)
    )
    click = win.pop("_extra", None) or {"sent": False, "via": None, "logged": False, "pixel_ok": False}
    summary = {"example": EXAMPLE, "desktop": desktop, "window": win, "click": click}
    print("SUMMARY " + json.dumps(summary, separators=(",", ":")), flush=True)
    ok = desktop["frames"] > 0 and win["frames"] > 0 and click.get("logged")
    return 0 if ok else 1


async def bench(url: str, token: str, jsonl_path: str) -> int:
    target_spec = os.environ.get("CUA_BENCH_TARGET", "display:primary")
    seconds = float(os.environ.get("CUA_BENCH_SECONDS", "5"))
    audio = os.environ.get("CUA_BENCH_AUDIO", "0") == "1"
    kind, _, value = target_spec.partition(":")

    env = await connect(url, token)
    if kind == "window":
        target = await find_window(env, value)
        if target is None:
            log(f"bench: window {value!r} not found")
            return 1
    elif kind == "display":
        target = {"kind": "display", "id": value or "primary", "width": 0.0}
    else:
        log(f"bench: bad CUA_BENCH_TARGET {target_spec!r}")
        return 2

    with open(jsonl_path, "a", buffering=1 << 16) as f:
        # Window frames carry the strip at frame (0, 0); the scale is fixed
        # up from the first frame when the stream is scaled.
        sink = StreamSink("bench", jsonl=f, tc_scale=1.0)
        f.write(json.dumps({"t": "open", "unix_ns": time.time_ns()}, separators=(",", ":")) + "\n")
        session = await open_stream(env, target, sink, audio=audio)
        try:
            if kind == "window" and target["width"]:
                # Bounded wait for the first frame to learn the frame scale.
                for _ in range(100):
                    fr = sink.snapshot_frame()
                    if fr is not None:
                        sink.tc_scale = fr.width / target["width"]
                        break
                    await asyncio.sleep(0.02)
            await asyncio.sleep(seconds)
        finally:
            await session.close()
        ru = resource.getrusage(resource.RUSAGE_SELF)
        with sink.lock:
            f.write(
                json.dumps(
                    {
                        "t": "end",
                        "unix_ns": time.time_ns(),
                        "cpu_user_s": round(ru.ru_utime, 3),
                        "cpu_sys_s": round(ru.ru_stime, 3),
                    },
                    separators=(",", ":"),
                )
                + "\n"
            )
            frames, decoded = sink.frames, sink.tc_decoded
    log(f"bench: {frames} frames, {decoded} with tc_ms -> {jsonl_path}")
    return 0 if frames > 0 else 1


def main() -> int:
    url = os.environ.get("CUA_ENV_URL", "http://127.0.0.1:33211")
    token = os.environ.get("CUA_ENV_TOKEN")
    if not token:
        log("CUA_ENV_TOKEN is required")
        return 2
    jsonl = os.environ.get("CUA_BENCH_JSONL")
    coro = bench(url, token, jsonl) if jsonl else scenario(url, token)
    return asyncio.run(asyncio.wait_for(coro, timeout=300))


if __name__ == "__main__":
    sys.exit(main())
