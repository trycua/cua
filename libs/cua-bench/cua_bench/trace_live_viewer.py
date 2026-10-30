"""Local, read-only viewer for experimental trace bundles."""

from __future__ import annotations

import json
import secrets
import threading
import time
import webbrowser
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from .trace_bundle import (
    TraceBundleReader,
    MAX_ATTACHMENT,
    HARD_MAX_ATTACHMENT,
    MAX_TEXT_PREVIEW,
    MAX_IMAGE_PREVIEW,
)


@contextmanager
def serve_bundle(directory, *, follow=False, max_download=MAX_ATTACHMENT):
    if not isinstance(max_download, int) or not 0 < max_download <= HARD_MAX_ATTACHMENT:
        raise ValueError("viewer download limit must be between 1 byte and 4 GiB")
    transfers = threading.BoundedSemaphore(2)
    tickets = {}  # single-use capabilities, bounded to 16 with 60-second expiry
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

        def transfer(self, artifact_id, *, preview=False):
            if not transfers.acquire(blocking=False):
                self.reply(429, b'{"error":"two attachment transfers are already active"}')
                return
            headers_sent = False
            try:
                with lock:
                    ref = reader.attachment_ref(artifact_id)
                is_text = ref["media_type"] in ("text/plain", "application/json")
                is_image = ref["media_type"] in (
                    "image/png",
                    "image/jpeg",
                    "image/gif",
                    "image/webp",
                )
                if preview and not (is_text or is_image):
                    raise ValueError("attachment is download-only")
                if preview and is_image and ref["size"] > MAX_IMAGE_PREVIEW:
                    raise ValueError("image exceeds 8 MiB preview limit; download instead")
                # Integrity verification produces a private disk snapshot. No whole-file
                # Python or JavaScript buffer, and mutation after validation cannot alter it.
                if ref["size"] > max_download:
                    raise ValueError("attachment exceeds viewer download limit")
                handle, ref = reader.attachment_snapshot(ref)
                with handle:
                    size = (
                        min(ref["size"], MAX_TEXT_PREVIEW) if preview and is_text else ref["size"]
                    )
                    self.send_response(200)
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Content-Disposition", 'attachment; filename="attachment"')
                    self.send_header("Content-Length", str(size))
                    self.send_header("X-Content-Type-Options", "nosniff")
                    self.send_header("Cache-Control", "no-store")
                    self.send_header("Referrer-Policy", "no-referrer")
                    self.send_header("Content-Security-Policy", "default-src 'none'; sandbox")
                    self.end_headers()
                    headers_sent = True
                    while size:
                        chunk = handle.read(min(64 * 1024, size))
                        if not chunk:
                            raise OSError("incomplete attachment snapshot")
                        self.wfile.write(chunk)
                        size -= len(chunk)
            except (ValueError, KeyError, OSError) as error:
                if isinstance(error, ConnectionError):
                    raise
                if headers_sent:
                    self.close_connection = True
                    return
                message = str(error) if isinstance(error, ValueError) else "attachment unavailable"
                self.reply(409, json.dumps({"error": message}).encode())
            finally:
                transfers.release()

        def do_GET(self):
            self.connection.settimeout(30)
            try:
                self._get()
            except (ConnectionError, TimeoutError):
                # A tab closing or going offline is an ordinary reader disconnect.
                pass

        def _get(self):
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
            if url.path.startswith("/download/"):
                with lock:
                    ticket = tickets.pop(url.path.removeprefix("/download/"), None)
                if ticket is None or ticket[1] < time.monotonic():
                    self.reply(403, b'{"error":"download authorization expired or invalid"}')
                    return
                self.transfer(ticket[0])
                return
            if not secrets.compare_digest(self.headers.get("Authorization", ""), "Bearer " + token):
                self.reply(403, b'{"error":"viewer authorization required"}')
                return
            try:
                if url.path in ("/status", "/page", "/event", "/image-neighbor", "/related"):
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
                            if url.path == "/related":
                                result = reader.related(
                                    sequence,
                                    through=through,
                                    order=query.get("order", ["arrival"])[0],
                                    page=int(query.get("page", ["0"])[0]),
                                )
                            else:
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
                elif url.path == "/download-ticket":
                    artifact_id = parse_qs(url.query).get("id", [""])[0]
                    with lock:
                        ref = reader.attachment_ref(artifact_id)
                        if ref["size"] > max_download:
                            raise ValueError("attachment exceeds viewer download limit")
                        now = time.monotonic()
                        for key in list(tickets):
                            if tickets[key][1] < now:
                                del tickets[key]
                        if len(tickets) >= 16:
                            raise ValueError("too many pending downloads")
                        capability = secrets.token_urlsafe(32)
                        tickets[capability] = (artifact_id, now + 60)
                    self.reply(200, json.dumps({"url": "/download/" + capability}).encode())
                elif url.path.startswith("/attachment/"):
                    self.transfer(url.path.removeprefix("/attachment/"))
                elif url.path.startswith("/preview/"):
                    self.transfer(url.path.removeprefix("/preview/"), preview=True)
                else:
                    self.reply(404, b'{"error":"not found"}')
            except ConnectionError:
                raise
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


def view_bundle(directory, *, follow=False, max_download=MAX_ATTACHMENT):
    with serve_bundle(directory, follow=follow, max_download=max_download) as url:
        webbrowser.open(url)
        print("Trace viewer opened. Press Enter to stop viewing (recording continues).")
        try:
            input()
        except KeyboardInterrupt:
            pass
    return 0
