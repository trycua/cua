"""Serve a loopback-only form with an independent verification endpoint."""

from __future__ import annotations

import argparse
import html
import json
import threading
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs


PAGE = b"""<!doctype html>
<html lang="en"><meta charset="utf-8"><title>Cua Driver Jev fixture</title>
<style>
body { font: 20px system-ui; display:grid; min-height:90vh; place-items:center; }
main { width:min(640px,85vw); text-align:center; }
input,button { box-sizing:border-box; font:inherit; margin:8px; padding:16px; }
input { width:min(440px,75vw); }
output { display:block; margin-top:20px; font-family:monospace; }
</style>
<main><h1>Cua Driver + TypeSafe Jev fixture</h1>
<p>Enter the verification value and submit it.</p>
<form method="post" action="/submit">
<input name="value" required aria-label="verification value">
<button type="submit">Submit</button></form>
<output>status=waiting</output></main></html>"""


class FixtureState:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._submitted: str | None = None

    def submit(self, value: str) -> None:
        with self._lock:
            self._submitted = value

    def reset(self) -> None:
        with self._lock:
            self._submitted = None

    def snapshot(self) -> dict[str, str | None]:
        with self._lock:
            return {"submitted": self._submitted}


class FixtureHandler(BaseHTTPRequestHandler):
    server: "FixtureServer"

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/":
            self._send(HTTPStatus.OK, "text/html; charset=utf-8", PAGE)
        elif self.path == "/state":
            self._send(
                HTTPStatus.OK,
                "application/json",
                json.dumps(self.server.state.snapshot()).encode(),
            )
        else:
            self.send_error(HTTPStatus.NOT_FOUND)

    def do_POST(self) -> None:  # noqa: N802
        if self.path == "/reset":
            self.server.state.reset()
            self._send(HTTPStatus.NO_CONTENT, "text/plain", b"")
            return
        if self.path != "/submit":
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        length = int(self.headers.get("Content-Length", "0"))
        value = parse_qs(self.rfile.read(length).decode()).get("value", [""])[0]
        if not value:
            self.send_error(HTTPStatus.BAD_REQUEST, "value is required")
            return
        self.server.state.submit(value)
        safe = html.escape(value)
        body = (
            "<!doctype html><title>Cua Driver Jev verified</title>"
            f"<h1>Action received</h1><output>status=submitted:{safe}</output>"
        ).encode()
        self._send(HTTPStatus.OK, "text/html; charset=utf-8", body)

    def log_message(self, format: str, *args: object) -> None:
        return

    def _send(self, status: HTTPStatus, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class FixtureServer(ThreadingHTTPServer):
    def __init__(self, address: tuple[str, int]) -> None:
        self.state = FixtureState()
        super().__init__(address, FixtureHandler)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    server = FixtureServer(("127.0.0.1", args.port))
    print(f"Fixture ready at http://127.0.0.1:{server.server_port}/", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
