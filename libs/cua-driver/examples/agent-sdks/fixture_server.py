"""Serve a deterministic desktop fixture for Cua Driver examples.

The browser page has one autofocus input. Submitting it records the value on a
loopback-only endpoint so examples can verify the GUI action independently of
the driver's own response.
"""

from __future__ import annotations

import argparse
import html
import json
import threading
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs


PAGE = """<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>Cua Driver action fixture</title>
<style>
  body { font: 20px system-ui; display: grid; min-height: 90vh; place-items: center; }
  main { width: min(640px, 85vw); text-align: center; }
  input, button { box-sizing: border-box; font: inherit; margin: 8px; padding: 16px; }
  input { width: min(440px, 75vw); }
  output { display: block; margin-top: 20px; font-family: monospace; }
</style>
<main>
  <h1>Cua Driver action fixture</h1>
  <p>Keep this window focused. The input below has autofocus.</p>
  <form method="post" action="/submit">
    <input name="value" autofocus required aria-label="verification value">
    <button type="submit">Submit</button>
  </form>
  <output>status=waiting</output>
</main>
</html>
"""


def submitted_page(value: str) -> bytes:
    safe = html.escape(value)
    return f"""<!doctype html>
<html lang="en"><meta charset="utf-8"><title>Cua Driver verified</title>
<style>body {{ font: 24px system-ui; display:grid; min-height:90vh; place-items:center; }}</style>
<main><h1>Action received</h1><output>status=submitted:{safe}</output></main>
</html>""".encode()


class FixtureState:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._submitted: str | None = None

    def reset(self) -> None:
        with self._lock:
            self._submitted = None

    def submit(self, value: str) -> None:
        with self._lock:
            self._submitted = value

    def snapshot(self) -> dict[str, str | None]:
        with self._lock:
            return {"submitted": self._submitted}


class FixtureHandler(BaseHTTPRequestHandler):
    server: "FixtureServer"

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path == "/":
            self._send(HTTPStatus.OK, "text/html; charset=utf-8", PAGE.encode())
            return
        if self.path == "/state":
            payload = json.dumps(self.server.state.snapshot()).encode()
            self._send(HTTPStatus.OK, "application/json", payload)
            return
        self.send_error(HTTPStatus.NOT_FOUND)

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path == "/reset":
            self.server.state.reset()
            self._send(HTTPStatus.NO_CONTENT, "text/plain", b"")
            return
        if self.path == "/submit":
            length = int(self.headers.get("Content-Length", "0"))
            values = parse_qs(self.rfile.read(length).decode())
            value = values.get("value", [""])[0]
            if not value:
                self.send_error(HTTPStatus.BAD_REQUEST, "value is required")
                return
            self.server.state.submit(value)
            self._send(HTTPStatus.OK, "text/html; charset=utf-8", submitted_page(value))
            return
        self.send_error(HTTPStatus.NOT_FOUND)

    def log_message(self, format: str, *args: object) -> None:
        return

    def _send(self, status: HTTPStatus, content_type: str, payload: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class FixtureServer(ThreadingHTTPServer):
    def __init__(self, address: tuple[str, int]) -> None:
        self.state = FixtureState()
        super().__init__(address, FixtureHandler)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--no-open", action="store_true")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    server = FixtureServer(("127.0.0.1", args.port))
    url = f"http://127.0.0.1:{server.server_port}/"
    print(f"Fixture ready at {url}", flush=True)
    if not args.no_open:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
