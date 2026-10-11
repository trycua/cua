#!/usr/bin/env python3
"""Local HTTP fixture server (stdlib only).

Routes:
  GET  /           fixture page (web/index.html): known colors + a small form
  GET  /health     "ok"
  GET  /bytes/<n>  n deterministic bytes (i % 251) for download/transfer tests
  POST /echo       echoes the request body with its sha256 in X-Sha256
  *    anything else under web/ is served as a static file

Every request is logged as JSONL (see fixturelog.py). Binds
CUA_FIXTURE_HTTP_ADDR (default 0.0.0.0) : CUA_FIXTURE_HTTP_PORT (default 18080).
"""

from __future__ import annotations

import hashlib
import os
import sys
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fixturelog import FixtureLog  # noqa: E402

NAME = os.environ.get("CUA_FIXTURE_NAME", "http")
ADDR = os.environ.get("CUA_FIXTURE_HTTP_ADDR", "0.0.0.0")
PORT = int(os.environ.get("CUA_FIXTURE_HTTP_PORT", "18080"))
WEB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "web")
MAX_BYTES = 64 * 1024 * 1024
LOG = FixtureLog(NAME)


class Handler(SimpleHTTPRequestHandler):
    server_version = "cua-fixture-http/1"

    def log_message(self, fmt, *args):  # stdout stays quiet; JSONL is the log
        pass

    def _log(self, status: int, **extra) -> None:
        LOG.emit(
            "request",
            method=self.command,
            path=self.path,
            status=status,
            client=self.client_address[0],
            user_agent=self.headers.get("User-Agent", ""),
            **extra,
        )

    def _send(self, status: int, body: bytes, ctype: str, **headers) -> None:
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in headers.items():
            self.send_header(k.replace("_", "-"), v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self._send(200, b"ok\n", "text/plain")
            self._log(200)
            return
        if self.path.startswith("/bytes/"):
            try:
                n = int(self.path.split("/", 2)[2])
            except ValueError:
                n = -1
            if not 0 <= n <= MAX_BYTES:
                self._send(400, b"bad length\n", "text/plain")
                self._log(400)
                return
            body = bytes(i % 251 for i in range(n))
            self._send(200, body, "application/octet-stream", X_Sha256=hashlib.sha256(body).hexdigest())
            self._log(200, bytes=n)
            return
        super().do_GET()
        self._log(200)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if self.path != "/echo" or length > MAX_BYTES:
            self._send(404, b"not found\n", "text/plain")
            self._log(404)
            return
        body = self.rfile.read(length)
        digest = hashlib.sha256(body).hexdigest()
        self._send(200, body, self.headers.get("Content-Type", "application/octet-stream"), X_Sha256=digest)
        self._log(200, bytes=len(body), sha256=digest, body_preview=body[:256].decode("utf-8", "replace"))


def main() -> int:
    httpd = ThreadingHTTPServer((ADDR, PORT), partial(Handler, directory=WEB))
    LOG.emit("ready", pid=os.getpid(), addr=ADDR, port=PORT)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    LOG.emit("exit")
    return 0


if __name__ == "__main__":
    sys.exit(main())
