"""Local, read-only viewer for experimental trace bundles."""

from __future__ import annotations

import json
import secrets
import threading
import webbrowser
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from .trace_bundle import TraceBundleReader


@contextmanager
def serve_bundle(directory, *, follow=False):
    reader = TraceBundleReader(directory)
    cutoff = None if follow else reader.count - 1
    token = secrets.token_urlsafe(32)
    lock = threading.Lock()
    assets = Path(__file__).parent / "www"

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, status, body, media="application/json"):
            self.send_response(status)
            self.send_header("Content-Type", media)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header(
                "Content-Security-Policy",
                "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            )
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            expected = f"127.0.0.1:{self.server.server_port}"
            if self.headers.get("Host") != expected:
                self.reply(403, b'{"error":"invalid host"}')
                return
            origin = self.headers.get("Origin")
            if origin and origin != "http://" + expected:
                self.reply(403, b'{"error":"invalid origin"}')
                return
            url = urlsplit(self.path)
            public = {
                "/": ("trace_live.html", "text/html; charset=utf-8"),
                "/viewer.js": ("trace_live.js", "text/javascript; charset=utf-8"),
                "/viewer.css": ("trace_live.css", "text/css; charset=utf-8"),
            }
            if url.path in public:
                filename, media = public[url.path]
                self.reply(200, (assets / filename).read_bytes(), media)
                return
            if not secrets.compare_digest(self.headers.get("Authorization", ""), "Bearer " + token):
                self.reply(403, b'{"error":"viewer authorization required"}')
                return
            try:
                if url.path in ("/status", "/page", "/event", "/image-neighbor"):
                    query = parse_qs(url.query)
                    with lock:
                        status = reader.status(cutoff=cutoff)
                        generation = query.get("generation", [reader.generation])[0]
                        if generation != reader.generation:
                            raise ValueError("trace generation changed; reopen the viewer")
                        through = int(query.get("through", [str(status["cursor"])])[0])
                        if through < -1 or through > status["cursor"]:
                            raise ValueError("invalid trace cursor")
                        if url.path == "/status":
                            result = {**status, "follow": follow}
                        elif url.path == "/page":
                            result = reader.page(
                                order=query.get("order", ["arrival"])[0],
                                page=int(query.get("page", ["0"])[0]),
                                through=through,
                                selected=int(query["selected"][0]) if "selected" in query else None,
                            )
                        else:
                            sequence = int(query.get("sequence", ["-1"])[0])
                            if sequence < 0 or sequence > through:
                                raise ValueError("unknown event")
                            result = (
                                reader.event(sequence)
                                if url.path == "/event"
                                else reader.image_neighbor(
                                    sequence,
                                    direction=int(query.get("direction", ["1"])[0]),
                                    order=query.get("order", ["arrival"])[0],
                                    through=through,
                                )
                            )
                    self.reply(200, json.dumps(result).encode())
                elif url.path == "/events":
                    query = parse_qs(url.query)
                    after = int(query.get("after", ["-1"])[0])
                    with lock:
                        batch = reader.batch(after, generation=query.get("generation", [None])[0])
                    if cutoff is not None:
                        batch["events"] = [e for e in batch["events"] if e["sequence"] <= cutoff]
                        batch["cursor"] = (
                            batch["events"][-1]["sequence"] if batch["events"] else after
                        )
                        batch["more"] = batch["cursor"] < cutoff
                        batch["state"] = "snapshot"
                    batch["follow"] = follow
                    self.reply(200, json.dumps(batch).encode())
                elif url.path.startswith("/attachment/"):
                    with lock:
                        handle, ref = reader.attachment(url.path.removeprefix("/attachment/"))
                    with handle:
                        self.send_response(200)
                        # Always a download at HTTP level; safe previews use local blobs/text.
                        self.send_header("Content-Type", "application/octet-stream")
                        self.send_header("Content-Disposition", 'attachment; filename="attachment"')
                        self.send_header("Content-Length", str(ref["size"]))
                        self.send_header("X-Content-Type-Options", "nosniff")
                        self.send_header("Cache-Control", "no-store")
                        self.end_headers()
                        while chunk := handle.read(64 * 1024):
                            self.wfile.write(chunk)
                else:
                    self.reply(404, b'{"error":"not found"}')
            except (ValueError, KeyError, OSError) as error:
                # Diagnostics expose categories, not local paths or payload fragments.
                message = (
                    str(error)
                    if isinstance(error, ValueError) and not isinstance(error, json.JSONDecodeError)
                    else "trace data unavailable or corrupt"
                )
                self.reply(409, json.dumps({"error": message}).encode())

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/#{token}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        reader.close()


def view_bundle(directory, *, follow=False):
    with serve_bundle(directory, follow=follow) as url:
        webbrowser.open(url)
        print("Trace viewer opened. Press Enter to stop viewing (recording continues).")
        try:
            input()
        except KeyboardInterrupt:
            pass
    return 0
