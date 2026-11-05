"""A small loopback web server for the v2 web tasks.

    python3 serve.py --app DIR --state-dir DIR --port N [--initial FILE]

* GET  /healthz          -> "ok"
* GET  /<path>           -> a file under --app (index.html for "/"); nothing outside it
* GET  /_kit/<file>      -> a shared page helper from _kit/static (kit.js)
* GET  /api/state        -> the task's saved state (state.json; created from --initial on start)
* POST /api/state        -> replace the saved state with the JSON body; logs a "state_saved" event
* POST /api/event        -> append the JSON body to events.jsonl with a server sequence number and time

The page's own script holds the app logic and saves through these two endpoints. The evaluator reads
state.json and events.jsonl. Bodies sent with navigator.sendBeacon (text/plain) are accepted too.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import shutil
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

MAX_BODY = 1_000_000


class Store:
    def __init__(self, state_dir: Path, initial: Path | None) -> None:
        self.dir = state_dir
        self.dir.mkdir(parents=True, exist_ok=True)
        self.state_path = self.dir / "state.json"
        self.events_path = self.dir / "events.jsonl"
        self.lock = threading.Lock()
        if not self.state_path.exists():
            if initial is not None and initial.is_file():
                shutil.copyfile(initial, self.state_path)
            else:
                self.state_path.write_text("{}\n", "utf-8")
        self.seq = sum(1 for _ in self.events_path.open("r", encoding="utf-8")) if self.events_path.exists() else 0

    def state(self) -> Any:
        with self.lock:
            return json.loads(self.state_path.read_text("utf-8"))

    def event(self, body: dict[str, Any]) -> dict[str, Any]:
        with self.lock:
            return self._event(body)

    def _event(self, body: dict[str, Any]) -> dict[str, Any]:
        self.seq += 1
        item = {**body, "seq": self.seq, "t": round(time.time(), 3)}
        with self.events_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(item, sort_keys=True) + "\n")
        return item

    def save(self, state: Any) -> dict[str, Any]:
        with self.lock:
            tmp = self.state_path.with_suffix(".tmp")
            tmp.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", "utf-8")
            tmp.replace(self.state_path)
            return self._event({"type": "state_saved"})


KIT_STATIC = Path(__file__).resolve().parent.parent / "static"


def make_handler(app_dir: Path, store: Store) -> type[BaseHTTPRequestHandler]:
    root = app_dir.resolve()

    class Handler(BaseHTTPRequestHandler):
        server_version = "v2kit"

        def log_message(self, fmt: str, *args: Any) -> None:  # quiet
            return

        def _send(self, code: int, body: bytes, ctype: str) -> None:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def _json(self, code: int, data: Any) -> None:
            self._send(code, json.dumps(data).encode(), "application/json")

        def do_GET(self) -> None:  # noqa: N802
            path = self.path.split("?", 1)[0].split("#", 1)[0]
            if path == "/healthz":
                return self._send(200, b"ok", "text/plain")
            if path == "/api/state":
                return self._json(200, store.state())
            base = root
            rel = "index.html" if path in ("", "/") else path.lstrip("/")
            if rel.startswith("_kit/"):  # shared page helpers (kit.js)
                base, rel = KIT_STATIC.resolve(), rel[len("_kit/") :]
            target = (base / rel).resolve()
            if base not in target.parents or not target.is_file():
                return self._send(404, b"not found", "text/plain")
            ctype = mimetypes.guess_type(target.name)[0] or "application/octet-stream"
            if ctype.startswith("text/") or ctype in ("application/javascript", "application/json"):
                ctype += "; charset=utf-8"
            return self._send(200, target.read_bytes(), ctype)

        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("Content-Length") or 0)
            if length > MAX_BODY:
                return self._json(413, {"error": "too large"})
            raw = self.rfile.read(length) if length else b""
            try:
                body = json.loads(raw.decode("utf-8") or "{}")
            except ValueError:
                return self._json(400, {"error": "bad json"})
            path = self.path.split("?", 1)[0]
            if path == "/api/state":
                return self._json(200, store.save(body))
            if path == "/api/event":
                if not isinstance(body, dict) or not isinstance(body.get("type"), str):
                    return self._json(400, {"error": "event needs a type"})
                return self._json(200, store.event(body))
            return self._json(404, {"error": "unknown endpoint"})

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--state-dir", required=True, type=Path)
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--initial", type=Path)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()
    store = Store(args.state_dir, args.initial)
    store.event({"type": "server_started"})
    server = ThreadingHTTPServer((args.host, args.port), make_handler(args.app, store))
    server.daemon_threads = True
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
