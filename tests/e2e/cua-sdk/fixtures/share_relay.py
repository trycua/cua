#!/usr/bin/env python3
"""A stand-in for the cua.ai relay + the host's cua-spacesd, for the hermetic
`cua share` e2e: just enough of both for a real cua daemon to make and serve
shares, with nothing mocked on the daemon side.

- ``GET /v1/machines`` and ``/v1/machines/<id>``: the directory (row) of this machine (owner role,
  online), whose ``url`` is ``<this server>/m/<id>`` (path mode).
- ``/m/<id>/share/...``: what the relay and the host's spacesd do together:
  a request with ``Authorization: Bearer owner`` is forwarded over the
  daemon's share gateway socket with ``x-cua-share-caller`` set to the owner
  (``Bearer other`` to a different account); a request without credentials is
  forwarded anonymously (the gateway then refuses private shares and allows
  public tokens). A client-sent ``x-cua-share-caller`` is dropped.

Usage: share_relay.py --gateway SOCK --machine-id ID [--port N]
Prints one JSON line ``{"port": N, "url": "http://127.0.0.1:N"}`` when ready.
"""

import argparse
import http.client
import json
import socket
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

OWNER = {
    "account": "acct-owner",
    "email": "owner@example.com",
    "name": "Owner",
    "role": "owner",
    "teams": [],
    "via": "relay",
}
OTHER = {
    "account": "acct-other",
    "email": "other@example.com",
    "name": "Other",
    "role": "shared",
    "teams": [],
    "via": "relay",
}


class Unix(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("share.local", timeout=30)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.path)


def make_handler(gateway, machine_id, port_holder):
    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *a):
            pass

        def _send(self, code, body=b"", headers=None):
            self.send_response(code)
            for k, v in (headers or {}).items():
                self.send_header(k, v)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def _handle(self):
            path = self.path
            if path in (f"/v1/machines/{machine_id}", "/v1/machines"):
                row = {
                    "id": machine_id,
                    "name": "share-e2e",
                    "owner": {"id": OWNER["account"], "email": OWNER["email"]},
                    "role": "owner",
                    "online": True,
                    "sharing": True,
                    "url": f"http://127.0.0.1:{port_holder[0]}/m/{machine_id}",
                }
                body = row if path != "/v1/machines" else {"machines": [row]}
                return self._send(
                    200, json.dumps(body).encode(), {"content-type": "application/json"}
                )
            prefix = f"/m/{machine_id}"
            if not path.startswith(prefix + "/share/"):
                return self._send(404, b"not found")
            gw_path = path[len(prefix) :]
            auth = self.headers.get("authorization", "")
            headers = {}
            for k, v in self.headers.items():
                if k.lower() in (
                    "host",
                    "connection",
                    "x-cua-share-caller",
                    "authorization",
                    "content-length",
                    "transfer-encoding",
                ):
                    continue
                headers[k] = v
            headers["x-forwarded-host"] = self.headers.get("host", "")
            headers["x-forwarded-proto"] = "https"
            if auth == "Bearer owner":
                headers["x-cua-share-caller"] = json.dumps(OWNER)
            elif auth == "Bearer other":
                headers["x-cua-share-caller"] = json.dumps(OTHER)
            elif auth:
                return self._send(401, b"invalid account token")
            n = int(self.headers.get("content-length") or 0)
            body = self.rfile.read(n) if n else None
            c = Unix(gateway)
            try:
                c.request(self.command, gw_path, body=body, headers=headers)
                r = c.getresponse()
                data = r.read()
                self.send_response(r.status)
                for k, v in r.getheaders():
                    if k.lower() in ("transfer-encoding", "connection", "content-length"):
                        continue
                    self.send_header(k, v)
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(data)
            except OSError as e:
                self._send(502, f"machine is not connected: {e}".encode())
            finally:
                c.close()

        do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = do_PATCH = _handle

    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gateway", required=True)
    ap.add_argument("--machine-id", required=True)
    ap.add_argument("--port", type=int, default=0)
    a = ap.parse_args()
    holder = [0]
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(a.gateway, a.machine_id, holder))
    holder[0] = srv.server_address[1]
    print(json.dumps({"port": holder[0], "url": f"http://127.0.0.1:{holder[0]}"}), flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    sys.exit(main())
