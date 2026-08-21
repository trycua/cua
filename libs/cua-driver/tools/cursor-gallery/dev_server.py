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
        self._counter = 0

    @property
    def watched_paths(self) -> tuple[Path, ...]:
        return (self.source_generator,)

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
            run_checked(
                [
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
                ],
                self.repo_root,
            )
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
                }
            )
        except Exception as error:  # Keep the last valid render available.
            self.state.fail(str(error))


def make_handler(gallery_root: Path, builds_root: Path, state: BuildState):
    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(gallery_root), **kwargs)

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
        make_handler(gallery_root, builder.builds_root, state),
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
