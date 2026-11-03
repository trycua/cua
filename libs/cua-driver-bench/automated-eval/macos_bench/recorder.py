# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Screen recording for every trial, plus background compression.

Why not `screencapture -v` or ffmpeg avfoundation: both are attributed to the launching app
(Terminal.app), which has no Screen Recording grant on this host and cannot get one unattended.
A separate Cua Driver 0.34.0 daemon (own socket, own HOME, no overlay) already holds the grant
under its own identity and records the main display with ScreenCaptureKit when asked through one
long-lived MCP connection (`start_recording` with `record_video`). The same recorder runs for both
arms, apart from the agent under test, so it is not part of what is compared.
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, Callable

import desktop_runtime as desktop
from desktop_runtime import stop_group, tool


class McpStdioClient:
    """Minimal MCP client over a child process' stdio (newline-delimited JSON-RPC)."""

    def __init__(
        self,
        argv: list[str],
        env: dict[str, str],
        elicitation: Callable[[str], dict[str, Any]] | None = None,
    ) -> None:
        self.elicitation = elicitation
        self.proc = subprocess.Popen(
            argv,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        try:
            desktop.record_pid(self.proc.pid, "mcp-client")
        except BaseException:
            desktop.stop_group(self.proc.pid)
            self.proc.wait()
            raise
        self._next = 0
        self._responses: dict[int, dict[str, Any]] = {}
        self._cond = threading.Condition()
        self._reader = threading.Thread(target=self._pump, daemon=True)
        self._reader.start()
        try:
            reply = self.request(
                "initialize",
                {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"elicitation": {}} if elicitation else {},
                    "clientInfo": {"name": "cdb-bench-recorder", "version": "1"},
                },
                timeout=30,
            )
            if reply.get("error"):
                raise RuntimeError(str(reply["error"])[:300])
            self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        except Exception:
            self.close()
            raise

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "id" in obj and obj.get("method") == "elicitation/create" and self.elicitation:
                answer = self.elicitation((obj.get("params") or {}).get("message", ""))
                self._send({"jsonrpc": "2.0", "id": obj["id"], "result": answer})
            elif "id" in obj and ("result" in obj or "error" in obj):
                with self._cond:
                    self._responses[int(obj["id"])] = obj
                    self._cond.notify_all()

    def _send(self, obj: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict[str, Any], timeout: float = 30) -> dict[str, Any]:
        self._next += 1
        ident = self._next
        self._send({"jsonrpc": "2.0", "id": ident, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        with self._cond:
            while ident not in self._responses:
                left = deadline - time.monotonic()
                if left <= 0 or self.proc.poll() is not None:
                    raise TimeoutError(f"MCP {method} got no response")
                self._cond.wait(timeout=min(left, 0.5))
            return self._responses.pop(ident)

    def call_tool(
        self, name: str, arguments: dict[str, Any], timeout: float = 30
    ) -> dict[str, Any]:
        reply = self.request("tools/call", {"name": name, "arguments": arguments}, timeout=timeout)
        if "error" in reply:
            raise RuntimeError(f"{name}: {reply['error']}")
        result = reply.get("result") or {}
        if isinstance(result.get("structuredContent"), dict):
            return result["structuredContent"]
        for block in result.get("content") or []:
            if isinstance(block, dict) and block.get("type") == "text":
                try:
                    return json.loads(block["text"])
                except (json.JSONDecodeError, TypeError):
                    return {"text": block.get("text")}
        return result.get("structuredContent") or {}

    def close(self) -> None:
        try:
            assert self.proc.stdin is not None
            self.proc.stdin.close()
        except (OSError, AssertionError):
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            stop_group(self.proc.pid)
            self.proc.wait(timeout=5)


class NullRecorder:
    name = "none"

    def start(self, out_dir: Path) -> bool:
        return False

    def stop(self) -> Path | None:
        return None

    def close(self) -> None:
        pass


class CuaRecorder:
    """Records the main display through a dedicated Cua Driver daemon (see module docstring)."""

    name = "cua-driver-recording"

    def __init__(self, binary: Path, socket: str, env: dict[str, str]) -> None:
        self.binary, self.socket, self.env = binary, socket, env
        self.client: McpStdioClient | None = None
        self.active_dir: Path | None = None
        self.last_video: Path | None = None
        self.last_error: str | None = None

    def _connect(self) -> McpStdioClient:
        if self.client is None or self.client.proc.poll() is not None:
            self.client = McpStdioClient(
                [str(self.binary), "--socket", self.socket, "mcp"], self.env
            )
        return self.client

    def start(self, out_dir: Path) -> bool:
        try:
            out_dir.mkdir(parents=True, exist_ok=True)
            client = self._connect()
            state = client.call_tool(
                "start_recording", {"output_dir": str(out_dir), "record_video": True}, timeout=30
            )
            self.active_dir = out_dir if state.get("video_active") else None
            self.last_video = None
            self.last_error = state.get("last_error")
            return bool(state.get("video_active"))
        except (OSError, TimeoutError, RuntimeError) as error:
            self.last_error = f"{type(error).__name__}: {error}"
            self.client = None
            return False

    def stop(self) -> Path | None:
        if self.active_dir is None:
            return self.last_video
        try:
            client = self._connect()
            state = client.call_tool("stop_recording", {}, timeout=60)
            self.active_dir = None
            path = state.get("last_video_path")
            if not path:
                state = client.call_tool("get_recording_state", {}, timeout=15)
                path = state.get("last_video_path")
            self.last_error = state.get("last_error") or self.last_error
            if path and Path(path).is_file():
                self.last_video = Path(path)
                return self.last_video
        except (OSError, TimeoutError, RuntimeError) as error:
            self.last_error = f"{type(error).__name__}: {error}"
            self.client = None
        return None

    def close(self) -> None:
        if self.active_dir is not None:
            self.stop()
        if self.client is not None:
            self.client.close()
            self.client = None


def capture_detail(path: Path, ffmpeg: str | None = None) -> str:
    ffmpeg = ffmpeg or tool("ffmpeg")
    done = subprocess.run(
        [
            ffmpeg,
            "-nostdin",
            "-v",
            "error",
            "-i",
            str(path),
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "ppm",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ],
        capture_output=True,
        timeout=30,
    )
    if done.returncode or not done.stdout:
        raise RuntimeError("capture decode failed or empty image")
    try:
        magic, dimensions, maximum, pixels = done.stdout.split(b"\n", 3)
        w, h = map(int, dimensions.split())
        if magic != b"P6" or maximum != b"255" or len(pixels) != w * h * 3 or not pixels:
            raise ValueError("invalid RGB frame")
    except ValueError as error:
        raise RuntimeError("capture decode failed") from error
    if pixels == pixels[:3] * (w * h):
        raise RuntimeError("blank image (uniform RGB frame)")
    return f"{w}x{h} bytes={path.stat().st_size} path={path}"


def ffprobe_duration(path: Path) -> float | None:
    try:
        done = subprocess.run(
            [
                tool("ffprobe"),
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nw=1:nk=1",
                str(path),
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return float(done.stdout.strip())
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return None


def compress_video(raw: Path, dest_dir: Path, ffmpeg: str | None = None) -> dict[str, Any]:
    """H.264 (libx264, crf 28, yuv420p, faststart) full-size copy plus a 720p copy; delete the raw
    file only when both copies exist and decode to a plausible duration."""
    ffmpeg = ffmpeg or tool("ffmpeg")
    dest_dir.mkdir(parents=True, exist_ok=True)
    full = dest_dir / "video.mp4"
    small = dest_dir / "video-720p.mp4"
    result: dict[str, Any] = {"raw_bytes": raw.stat().st_size if raw.exists() else 0}
    base = [
        "nice",
        "-n",
        "10",
        ffmpeg,
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-i",
        str(raw),
        "-an",
    ]
    enc = [
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
    ]
    try:
        subprocess.run(
            base + ["-vf", "fps=15", *enc, "-crf", "28", str(full)], check=True, timeout=900
        )
        subprocess.run(
            base + ["-vf", "fps=15,scale=-2:720", *enc, "-crf", "30", str(small)],
            check=True,
            timeout=900,
        )
    except (OSError, subprocess.SubprocessError) as error:
        result["error"] = f"{type(error).__name__}: {error}"
        return result
    raw_s, full_s = ffprobe_duration(raw), ffprobe_duration(full)
    result.update(
        video=str(full),
        video_720p=str(small),
        bytes=full.stat().st_size,
        bytes_720p=small.stat().st_size,
        duration_s=full_s,
        raw_duration_s=raw_s,
    )
    if full_s and raw_s and abs(full_s - raw_s) <= max(1.0, 0.1 * raw_s):
        raw.unlink(missing_ok=True)
        result["raw_deleted"] = True
    else:
        result["raw_deleted"] = False
        result["error"] = "duration mismatch; raw kept"
    return result


class CompressQueue:
    """Background compression off the critical path. ``join`` blocks until the queue is empty."""

    def __init__(self) -> None:
        self.q: queue.Queue[tuple[Path, Path] | None] = queue.Queue()
        self.results: dict[str, dict[str, Any]] = {}
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self) -> None:
        while True:
            item = self.q.get()
            if item is None:
                self.q.task_done()
                return
            raw, dest = item
            try:
                self.results[str(dest)] = compress_video(raw, dest)
                (dest / "video.json").write_text(
                    json.dumps(self.results[str(dest)], indent=2) + "\n", "utf-8"
                )
            except Exception as error:  # noqa: BLE001 - compression must never kill the run
                self.results[str(dest)] = {"error": f"{type(error).__name__}: {error}"}
            finally:
                self.q.task_done()

    def submit(self, raw: Path, dest_dir: Path) -> None:
        self.q.put((raw, dest_dir))

    def pending(self) -> int:
        return self.q.unfinished_tasks

    def join(self) -> None:
        self.q.join()

    def close(self) -> None:
        self.q.put(None)
        self.thread.join(timeout=30)


def free_gb(path: Path) -> float:
    usage = shutil.disk_usage(path)
    return usage.free / 1e9
