#!/usr/bin/env python3
"""bench-web-ctl: a small HTTP API over the in-guest Chromium's DevTools.

Adapters reach it the same way locally and on Fleet (plain HTTP through the
sandbox's service gateway), so they never need a WebSocket to the guest.

    GET  /healthz            {"ok": true, "browser": "...", "tabs": N}   503 until CDP answers
    POST /reset   {"url"}    clear cookies, cache and storage; close every tab
                             but one and load url (default about:blank)
    POST /navigate {"url", "wait": "load"|"none", "timeout": s}
    POST /eval    {"expression", "await": bool}   -> {"value": ...} (returnByValue)
    GET  /state              {"url", "title"} of the active tab

Listens on 0.0.0.0:$BENCH_WEB_CTL_PORT (7000). CDP: 127.0.0.1:$BENCH_WEB_CDP_PORT (9223).
"""

from __future__ import annotations

import itertools
import json
import os
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import websocket  # websocket-client

CDP = f"http://127.0.0.1:{int(os.environ.get('BENCH_WEB_CDP_PORT', '9223'))}"
PORT = int(os.environ.get("BENCH_WEB_CTL_PORT", "7000"))
LOCK = threading.Lock()
IDS = itertools.count(1)
MAX_BODY = 1 << 20


class CdpError(Exception):
    pass


def _json(url: str, method: str = "GET", timeout: float = 5.0):
    """A DevTools HTTP endpoint's JSON (some, like /json/close, answer plain text)."""
    req = urllib.request.Request(url, method=method)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        body = r.read()
    try:
        return json.loads(body or b"null")
    except ValueError:
        return body.decode("utf-8", "replace")


def pages() -> list[dict]:
    return [t for t in _json(f"{CDP}/json/list") if t.get("type") == "page"]


def active_page() -> dict:
    ps = pages()
    if not ps:
        _json(f"{CDP}/json/new?about:blank", method="PUT")
        ps = pages()
    if not ps:
        raise CdpError("no page target")
    return ps[0]


def call(ws_url: str, method: str, params: dict | None = None, timeout: float = 30.0,
         wait_event: str | None = None) -> dict:
    """One CDP command on a fresh connection (optionally waiting for an event)."""
    ws = websocket.create_connection(ws_url, timeout=timeout, suppress_origin=True)
    try:
        if wait_event:
            ws.send(json.dumps({"id": next(IDS), "method": wait_event.split(".")[0] + ".enable"}))
        mid = next(IDS)
        ws.send(json.dumps({"id": mid, "method": method, "params": params or {}}))
        deadline = time.monotonic() + timeout
        result = None
        seen_event = wait_event is None
        # Bounded: at most 10000 messages or the deadline.
        for _ in range(10000):
            if result is not None and seen_event:
                return result
            if time.monotonic() > deadline:
                raise CdpError(f"{method}: timed out")
            msg = json.loads(ws.recv())
            if msg.get("id") == mid:
                if "error" in msg:
                    raise CdpError(f"{method}: {msg['error'].get('message')}")
                result = msg.get("result", {})
            elif wait_event and msg.get("method") == wait_event:
                seen_event = True
        raise CdpError(f"{method}: too many messages")
    finally:
        ws.close()


def browser_ws() -> str:
    return _json(f"{CDP}/json/version")["webSocketDebuggerUrl"]


def _origin(url: str) -> str | None:
    parts = urllib.parse.urlsplit(url or "")
    if parts.scheme in ("http", "https") and parts.netloc:
        return f"{parts.scheme}://{parts.netloc}"
    return None


def reset(url: str) -> dict:
    ps = pages()
    keep = ps[0] if ps else None
    origins = {o for o in (_origin(p.get("url")) for p in ps) if o}
    for p in ps[1:]:
        _json(f"{CDP}/json/close/{p['id']}")
    # Storage of every origin the tabs showed (a "*" origin can hang the
    # browser endpoint); cookies and cache are cleared below for all origins.
    for origin in sorted(origins):
        try:
            call(browser_ws(), "Storage.clearDataForOrigin",
                 {"origin": origin, "storageTypes": "all"}, timeout=10)
        except Exception as e:  # noqa: BLE001 - best effort, logged
            print(f"reset: clear {origin}: {e}", flush=True)
    if keep is None:
        _json(f"{CDP}/json/new?about:blank", method="PUT")
        keep = active_page()
    ws = keep["webSocketDebuggerUrl"]
    # Best effort, bounded: right after start a fresh profile can stall these;
    # the navigation below is what a task needs.
    for method in ("Network.clearBrowserCookies", "Network.clearBrowserCache"):
        try:
            call(ws, method, timeout=10)
        except Exception as e:  # noqa: BLE001 - logged, reset continues
            print(f"reset: {method}: {e}", flush=True)
    return navigate(url, "load", 60)


def navigate(url: str, wait: str, timeout: float) -> dict:
    page = active_page()
    call(page["webSocketDebuggerUrl"], "Page.navigate", {"url": url}, timeout=timeout,
         wait_event="Page.loadEventFired" if wait == "load" and url != "about:blank" else None)
    _json(f"{CDP}/json/activate/{page['id']}")
    return state()


def evaluate(expression: str, await_promise: bool) -> dict:
    page = active_page()
    r = call(page["webSocketDebuggerUrl"], "Runtime.evaluate",
             {"expression": expression, "returnByValue": True, "awaitPromise": await_promise})
    if r.get("exceptionDetails"):
        raise CdpError("exception: " + json.dumps(r["exceptionDetails"])[:500])
    return {"value": (r.get("result") or {}).get("value")}


def state() -> dict:
    p = active_page()
    return {"url": p.get("url"), "title": p.get("title")}


class Handler(BaseHTTPRequestHandler):
    server_version = "bench-web-ctl/1"

    def log_message(self, fmt, *args):  # noqa: D401 - quieter log
        print("%s %s" % (self.address_string(), fmt % args), flush=True)

    def _send(self, code: int, obj: object) -> None:
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self) -> dict:
        n = int(self.headers.get("content-length") or 0)
        if n > MAX_BODY:
            raise CdpError("body too large")
        return json.loads(self.rfile.read(n) or b"{}") if n else {}

    def do_GET(self):  # noqa: N802
        try:
            if self.path == "/healthz":
                # Ready only when a page target answers: right after start,
                # DevTools lists targets before their sessions respond.
                v = _json(f"{CDP}/json/version", timeout=2)
                call(active_page()["webSocketDebuggerUrl"], "Runtime.evaluate", {"expression": "1"}, timeout=5)
                self._send(200, {"ok": True, "browser": v.get("Browser"), "tabs": len(pages())})
            elif self.path == "/state":
                with LOCK:
                    self._send(200, state())
            else:
                self._send(404, {"error": "not found"})
        except Exception as e:  # noqa: BLE001 - report every failure as JSON
            self._send(503, {"ok": False, "error": str(e)})

    def do_POST(self):  # noqa: N802
        try:
            body = self._body()
            with LOCK:
                if self.path == "/reset":
                    self._send(200, reset(body.get("url") or "about:blank"))
                elif self.path == "/navigate":
                    self._send(200, navigate(body["url"], body.get("wait", "load"),
                                             float(body.get("timeout", 60))))
                elif self.path == "/eval":
                    self._send(200, evaluate(body["expression"], bool(body.get("await", False))))
                else:
                    self._send(404, {"error": "not found"})
        except KeyError as e:
            self._send(400, {"error": f"missing {e}"})
        except Exception as e:  # noqa: BLE001
            self._send(500, {"error": str(e)})


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    print(f"bench-web-ctl on :{PORT}, CDP {CDP}", flush=True)
    srv.serve_forever()
