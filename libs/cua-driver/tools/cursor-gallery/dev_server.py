#!/usr/bin/env python3
"""Repository-only live rebuild server for the Cua cursor animation gallery."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from http import HTTPStatus
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse


ACTIONS = (
    "idle",
    "observe",
    "click",
    "drag",
    "scroll",
    "text",
    "key",
    "navigate",
    "app",
    "transfer",
    "record",
    "system",
)

# Loopback-only movement override contract (mirrors the Rust
# MotionOverrideFile schema in examples/export_gallery_frames.rs).
# Only fields the production movement path consumes are accepted.
MOTION_OVERRIDE_SCHEMA = "cua.cursor-gallery-motion-override/1"
MOTION_OVERRIDE_FIELDS = {
    "peak_speed": (50.0, 5000.0),
    "min_start_speed": (1.0, 5000.0),
    "min_end_speed": (1.0, 5000.0),
    "turn_radius": (1.0, 1000.0),
    "spring": (0.3, 1.0),
    "glide_duration_ms": (0.0, 5000.0),
}
MOTION_OVERRIDE_MAX_BYTES = 4096


def sanitize_motion_override(raw: bytes) -> dict[str, float]:
    """Validate a movement-override request body.

    Accepts a flat JSON object of whitelisted numeric fields (or `null`,
    which resets). Rejects anything else so no arbitrary keys, text, or
    nested data ever reach the renderer build.
    """
    if len(raw) > MOTION_OVERRIDE_MAX_BYTES:
        raise ValueError("override body too large")
    text = raw.decode("utf-8", errors="strict")
    if text.strip() == "null":
        return {}
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ValueError("override body must be a JSON object or null")
    override: dict[str, float] = {}
    for key, value in payload.items():
        bounds = MOTION_OVERRIDE_FIELDS.get(key)
        if bounds is None:
            raise ValueError(f"unknown movement field: {key}")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(f"field {key} must be a number")
        number = float(value)
        if number != number or number in (float("inf"), float("-inf")):
            raise ValueError(f"field {key} must be finite")
        low, high = bounds
        override[key] = min(max(number, low), high)
    return override


def serialize_motion_override(override: dict[str, float]) -> str:
    """One canonical writer so override-file diffs are value-only."""
    ordered: dict[str, object] = {"schema": MOTION_OVERRIDE_SCHEMA}
    for key in MOTION_OVERRIDE_FIELDS:
        if key in override:
            ordered[key] = override[key]
    return json.dumps(ordered, indent=2) + "\n"


class BuildState:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._status: dict[str, object] = {
            "schema": "cua.cursor-gallery-dev/1",
            "state": "starting",
            "message": "Waiting for the first exact render",
            "build": None,
            "previous_build": None,
            "error": None,
        }

    def begin(self) -> None:
        with self._lock:
            self._status["state"] = "building"
            self._status["message"] = "Compiling theme and rendering production frames"
            self._status["error"] = None

    def succeed(self, build: dict[str, object]) -> None:
        with self._lock:
            current = self._status.get("build")
            if current:
                self._status["previous_build"] = current
            self._status["build"] = build
            self._status["state"] = "ready"
            self._status["message"] = "Exact renderer preview is current"
            self._status["error"] = None

    def fail(self, message: str) -> None:
        with self._lock:
            self._status["state"] = "error"
            self._status["message"] = (
                "Build failed; showing the last valid render"
                if self._status.get("build")
                else "Initial build failed; no valid render is available"
            )
            self._status["error"] = message[-12_000:]

    def snapshot(self) -> dict[str, object]:
        with self._lock:
            return json.loads(json.dumps(self._status))


def watch_signature(paths: tuple[Path, ...]) -> tuple[tuple[str, int, int], ...]:
    signature = []
    for path in paths:
        try:
            stat = path.stat()
            signature.append((str(path), stat.st_mtime_ns, stat.st_size))
        except FileNotFoundError:
            signature.append((str(path), -1, -1))
    return tuple(signature)


def run_checked(command: list[str], cwd: Path) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    if result.returncode != 0:
        rendered = " ".join(command)
        raise RuntimeError(f"{rendered}\n\n{result.stdout.strip()}")
    return result.stdout.strip()


class Builder:
    def __init__(self, repo_root: Path, state: BuildState) -> None:
        self.repo_root = repo_root
        self.state = state
        self.driver_dir = repo_root / "libs" / "cua-driver"
        self.rust_manifest = self.driver_dir / "rust" / "Cargo.toml"
        self.source_generator = (
            self.driver_dir
            / "rust"
            / "crates"
            / "cursor-overlay"
            / "assets"
            / "build_default_theme.py"
        )
        self.builds_root = repo_root / "target" / "cursor-gallery" / "dev-builds"
        # Fixed-path movement override file. The loopback POST endpoint is
        # the only writer; the renderer rebuild consumes it verbatim.
        self.motion_override_path = repo_root / "target" / "cursor-gallery" / "motion-override.json"
        self._counter = 0

    @property
    def watched_paths(self) -> tuple[Path, ...]:
        return (self.source_generator, self.motion_override_path)

    def current_motion_override(self) -> dict[str, float]:
        try:
            payload = json.loads(self.motion_override_path.read_text())
        except FileNotFoundError:
            return {}
        except json.JSONDecodeError:
            return {}
        if not isinstance(payload, dict) or payload.get("schema") != MOTION_OVERRIDE_SCHEMA:
            return {}
        return {
            key: float(value)
            for key, value in payload.items()
            if key in MOTION_OVERRIDE_FIELDS and isinstance(value, (int, float))
        }

    def exporter_command(self, frames: Path, artifact: Path) -> list[str]:
        command = [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(self.rust_manifest),
            "-p",
            "cursor-overlay",
            "--example",
            "export_gallery_frames",
            "--features",
            "theme-authoring",
            "--",
            str(frames),
            "--theme",
            str(artifact),
            "--dev",
        ]
        if self.motion_override_path.exists():
            command.extend(["--motion-overrides", str(self.motion_override_path)])
        return command

    def build(self) -> None:
        self.state.begin()
        self._counter += 1
        build_id = f"build-{int(time.time() * 1000)}-{self._counter}"
        build_root = self.builds_root / build_id
        source = build_root / "cua.default.lottie"
        artifact = build_root / "cua.default.cua-theme"
        frames = build_root / "frames"
        media = build_root / "media"
        source.parent.mkdir(parents=True, exist_ok=True)

        try:
            run_checked(
                ["python3", str(self.source_generator), "--output", str(source)],
                self.repo_root,
            )
            run_checked(
                [
                    "cargo",
                    "run",
                    "--quiet",
                    "--manifest-path",
                    str(self.rust_manifest),
                    "-p",
                    "cursor-theme-cli",
                    "--",
                    "build",
                    str(source),
                    "--output",
                    str(artifact),
                ],
                self.repo_root,
            )
            run_checked(self.exporter_command(frames, artifact), self.repo_root)
            manifest = json.loads((frames / "manifest.json").read_text())
            for group in ("actions", "runtime", "movement"):
                destination = media / group
                destination.mkdir(parents=True, exist_ok=True)
                frame_rate = str(
                    manifest["movement_fps"] if group == "movement" else manifest["fps"]
                )
                for action in ACTIONS:
                    run_checked(
                        [
                            "ffmpeg",
                            "-y",
                            "-loglevel",
                            "error",
                            "-framerate",
                            frame_rate,
                            "-i",
                            str(frames / group / action / "%04d.png"),
                            "-c:v",
                            "libvpx-vp9",
                            "-pix_fmt",
                            "yuva420p",
                            "-auto-alt-ref",
                            "0",
                            "-crf",
                            "28",
                            "-b:v",
                            "0",
                            "-row-mt",
                            "1",
                            str(destination / f"{action}.webm"),
                        ],
                        self.repo_root,
                    )

            self.state.succeed(
                {
                    "id": build_id,
                    "asset_root": f"/__cursor_dev/assets/{build_id}/media",
                    "frame_root": f"/__cursor_dev/assets/{build_id}/frames",
                    "source": str(self.source_generator.relative_to(self.repo_root)),
                    "completed_at": int(time.time()),
                    "manifest": manifest,
                    "motion_override": self.current_motion_override(),
                }
            )
        except Exception as error:  # Keep the last valid render available.
            self.state.fail(str(error))


def make_handler(gallery_root: Path, builds_root: Path, state: BuildState, builder: Builder):
    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(gallery_root), **kwargs)

        def _send_json(self, payload: dict[str, object], status: HTTPStatus) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self) -> None:  # noqa: N802 - stdlib handler contract
            path = urlparse(self.path).path
            if path != "/__cursor_dev/motion-override":
                self.send_error(HTTPStatus.NOT_FOUND)
                return
            # Loopback alone does not stop a localhost page from POSTing here;
            # require an absent Origin (curl / tests) or this exact origin.
            origin = self.headers.get("Origin")
            if origin is not None:
                port = getattr(self.server, "server_port", None)
                allowed = {f"http://127.0.0.1:{port}", f"http://localhost:{port}"}
                if origin not in allowed:
                    self._send_json(
                        {"ok": False, "error": "cross-origin POST rejected"},
                        status=HTTPStatus.FORBIDDEN,
                    )
                    return
            # Trust the declared length only up to the cap, and refuse before
            # reading so an oversized body cannot be buffered at all.
            try:
                declared = int(self.headers.get("Content-Length") or 0)
            except ValueError:
                self._send_json(
                    {"ok": False, "error": "invalid Content-Length"},
                    status=HTTPStatus.BAD_REQUEST,
                )
                return
            if declared > MOTION_OVERRIDE_MAX_BYTES:
                self._send_json(
                    {"ok": False, "error": "override body too large"},
                    status=HTTPStatus.REQUEST_ENTITY_TOO_LARGE,
                )
                return
            raw = self.rfile.read(declared) if declared > 0 else b""
            try:
                override = sanitize_motion_override(raw)
            except ValueError as error:
                self._send_json(
                    {"ok": False, "error": str(error)}, status=HTTPStatus.BAD_REQUEST
                )
                return
            try:
                if override:
                    builder.motion_override_path.parent.mkdir(parents=True, exist_ok=True)
                    temporary = builder.motion_override_path.with_suffix(".json.tmp")
                    temporary.write_text(serialize_motion_override(override))
                    temporary.replace(builder.motion_override_path)
                else:
                    # `null` (or an empty object) resets to production defaults.
                    builder.motion_override_path.unlink(missing_ok=True)
            except OSError as error:
                self._send_json(
                    {"ok": False, "error": str(error)}, status=HTTPStatus.INTERNAL_SERVER_ERROR
                )
                return
            self._send_json({"ok": True, "override": override}, status=HTTPStatus.OK)

        def do_GET(self) -> None:  # noqa: N802 - stdlib handler contract
            path = urlparse(self.path).path
            if path in ("/", "/index.html"):
                html = (gallery_root / "index.html").read_text()
                payload = html.replace(
                    '<html lang="en">',
                    '<html lang="en" data-dev-server="true">',
                    1,
                ).encode()
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            if path == "/__cursor_dev/status.json":
                payload = json.dumps(state.snapshot()).encode()
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", "application/json")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            super().do_GET()

        def translate_path(self, path: str) -> str:
            parsed = urlparse(path).path
            prefix = "/__cursor_dev/assets/"
            if parsed.startswith(prefix):
                relative = Path(unquote(parsed[len(prefix) :]))
                if relative.is_absolute() or ".." in relative.parts:
                    return str(builds_root / "invalid-path")
                return str(builds_root / relative)
            return super().translate_path(path)

        def end_headers(self) -> None:
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            super().end_headers()

    return Handler


def watch(builder: Builder, interval: float) -> None:
    signature = None
    while True:
        current = watch_signature(builder.watched_paths)
        if current != signature:
            signature = current
            builder.build()
        time.sleep(interval)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=int(os.environ.get("CURSOR_GALLERY_PORT", "3001")))
    parser.add_argument("--watch-interval", type=float, default=0.5)
    args = parser.parse_args()

    gallery_root = Path(__file__).resolve().parent
    repo_root = gallery_root.parents[3]
    state = BuildState()
    builder = Builder(repo_root, state)
    builder.builds_root.mkdir(parents=True, exist_ok=True)

    watcher = threading.Thread(target=watch, args=(builder, args.watch_interval), daemon=True)
    watcher.start()
    server = ThreadingHTTPServer(
        ("127.0.0.1", args.port),
        make_handler(gallery_root, builder.builds_root, state, builder),
    )
    print(f"cursor-gallery dev: http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
