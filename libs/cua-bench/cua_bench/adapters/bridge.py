"""A loopback HTTP bridge to a service URL that has a path prefix.

Upstream benchmark controllers (OSWorld's ``PythonController`` and
``SetupController``, CDP clients, BrowserGym ...) take a host and a port and
build ``http://{host}:{port}/path`` themselves. Locally a guest port is
exactly that (``http://127.0.0.1:<published>``). On Fleet a service is a
signed URL such as ``https://<gateway>/api/svc/<ns>/<sandbox>-<svc>?...``: a
path prefix (and maybe a query) that such code cannot express.

:class:`PathBridge` listens on ``127.0.0.1:<ephemeral>`` and forwards every
request to the URL:

* the request path is appended to the URL's path prefix and the URL's query
  is merged into the request's query;
* ``Host`` is rewritten to the upstream, TLS is used for ``https`` URLs;
* plain requests get ``Connection: close`` (one request per connection, so
  every request is rewritten);
* WebSocket upgrades (Chrome DevTools) are rewritten once and then piped
  byte for byte in both directions;
* small JSON responses have the upstream's ``ws://``/``http://`` origin
  replaced by the bridge's own, so DevTools' ``webSocketDebuggerUrl`` points
  back through the bridge.

asyncio streams only; every read is bounded.
"""

from __future__ import annotations

import asyncio
import contextlib
import ssl
from typing import Optional
from urllib.parse import urlsplit

#: Largest request or response head accepted.
MAX_HEAD = 64 * 1024
#: JSON responses up to this size are rewritten (larger ones stream as is).
MAX_REWRITE_BODY = 1024 * 1024
CHUNK = 64 * 1024


class BridgeError(RuntimeError):
    pass


async def _read_head(reader: asyncio.StreamReader) -> Optional[bytes]:
    try:
        return await reader.readuntil(b"\r\n\r\n")
    except asyncio.IncompleteReadError as e:
        if not e.partial:
            return None
        raise BridgeError("connection closed inside a message head") from e
    except asyncio.LimitOverrunError as e:
        raise BridgeError(f"message head larger than {MAX_HEAD} bytes") from e


def _split_head(head: bytes) -> tuple[str, list[tuple[str, str]]]:
    lines = head.decode("latin-1").split("\r\n")
    first = lines[0]
    headers = []
    for line in lines[1:]:
        if not line:
            continue
        name, _, value = line.partition(":")
        headers.append((name.strip(), value.strip()))
    return first, headers


def _join_head(first: str, headers: list[tuple[str, str]]) -> bytes:
    return ("\r\n".join([first] + [f"{k}: {v}" for k, v in headers]) + "\r\n\r\n").encode("latin-1")


def _get(headers: list[tuple[str, str]], name: str) -> Optional[str]:
    for k, v in headers:
        if k.lower() == name.lower():
            return v
    return None


def _without(headers: list[tuple[str, str]], *names: str) -> list[tuple[str, str]]:
    drop = {n.lower() for n in names}
    return [(k, v) for k, v in headers if k.lower() not in drop]


class PathBridge:
    """``127.0.0.1:<port>`` -> ``url`` (see the module docstring)."""

    def __init__(self, url: str, *, ssl_context: Optional[ssl.SSLContext] = None) -> None:
        u = urlsplit(url)
        if u.scheme not in ("http", "https") or not u.hostname:
            raise BridgeError(f"cannot bridge {url!r}: expected an http(s) URL")
        self.url = url
        self.scheme = u.scheme
        self.upstream_host = u.hostname
        self.upstream_port = u.port or (443 if u.scheme == "https" else 80)
        self.prefix = u.path.rstrip("/")
        self.query = u.query
        default = (u.scheme == "https" and self.upstream_port == 443) or (
            u.scheme == "http" and self.upstream_port == 80
        )
        self.host_header = self.upstream_host if default else f"{self.upstream_host}:{self.upstream_port}"
        self._ssl = (ssl_context or ssl.create_default_context()) if u.scheme == "https" else None
        self._server: Optional[asyncio.base_events.Server] = None
        self._tasks: set[asyncio.Task] = set()
        self.port: Optional[int] = None

    async def start(self) -> tuple[str, int]:
        if self._server is None:
            self._server = await asyncio.start_server(self._serve, "127.0.0.1", 0, limit=MAX_HEAD)
            self.port = self._server.sockets[0].getsockname()[1]
        return "127.0.0.1", int(self.port)

    async def aclose(self) -> None:
        if self._server is not None:
            self._server.close()
            with contextlib.suppress(Exception):
                await self._server.wait_closed()
            self._server = None
        for t in list(self._tasks):
            t.cancel()
        if self._tasks:
            await asyncio.gather(*self._tasks, return_exceptions=True)

    # ── one client connection ─────────────────────────────────────────────

    def _rewrite_target(self, target: str) -> str:
        path, _, query = target.partition("?")
        if not path.startswith("/"):
            # absolute-form (proxy style) request
            path = urlsplit(target).path or "/"
        joined = (self.prefix + path) if self.prefix else path
        q = "&".join(p for p in (query, self.query) if p)
        return joined + (f"?{q}" if q else "")

    async def _serve(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        if task is not None:
            self._tasks.add(task)
        up_writer = None
        try:
            head = await _read_head(reader)
            if head is None:
                return
            first, headers = _split_head(head)
            method, target, version = first.split(" ", 2)
            upgrade = (_get(headers, "upgrade") or "").lower() == "websocket"
            headers = _without(headers, "host", "proxy-connection")
            headers.insert(0, ("Host", self.host_header))
            if not upgrade:
                headers = _without(headers, "connection", "keep-alive")
                headers.append(("Connection", "close"))
            up_reader, up_writer = await asyncio.open_connection(
                self.upstream_host,
                self.upstream_port,
                ssl=self._ssl,
                server_hostname=self.upstream_host if self._ssl else None,
                limit=MAX_HEAD,
            )
            up_writer.write(_join_head(f"{method} {self._rewrite_target(target)} {version}", headers))
            await up_writer.drain()
            if upgrade:
                await asyncio.gather(_pipe(reader, up_writer), _pipe(up_reader, writer))
            else:
                # The response ends the exchange: the client may keep its side
                # open (keep-alive), so the request pipe is cancelled then.
                req = asyncio.create_task(_pipe(reader, up_writer))
                try:
                    await self._response(up_reader, writer)
                finally:
                    req.cancel()
                    await asyncio.gather(req, return_exceptions=True)
        except (BridgeError, ConnectionError, OSError, ValueError) as e:
            with contextlib.suppress(Exception):
                body = f"bridge error: {e}".encode()
                writer.write(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nConnection: close\r\n"
                    + f"Content-Length: {len(body)}\r\n\r\n".encode()
                    + body
                )
                await writer.drain()
        finally:
            for w in (up_writer, writer):
                if w is not None:
                    with contextlib.suppress(Exception):
                        w.close()
            if task is not None:
                self._tasks.discard(task)

    async def _response(self, up: asyncio.StreamReader, down: asyncio.StreamWriter) -> None:
        head = await _read_head(up)
        if head is None:
            return
        first, headers = _split_head(head)
        headers = _without(headers, "connection", "keep-alive") + [("Connection", "close")]
        ctype = (_get(headers, "content-type") or "").lower()
        length = _get(headers, "content-length")
        if "json" in ctype and length is not None and length.isdigit() and int(length) <= MAX_REWRITE_BODY:
            body = await up.readexactly(int(length))
            body = self._rewrite_body(body)
            headers = _without(headers, "content-length") + [("Content-Length", str(len(body)))]
            down.write(_join_head(first, headers) + body)
            await down.drain()
            return
        down.write(_join_head(first, headers))
        await down.drain()
        await _pipe(up, down)

    def _rewrite_body(self, body: bytes) -> bytes:
        mine = f"127.0.0.1:{self.port}".encode()
        for scheme_in, scheme_out in ((b"wss://", b"ws://"), (b"ws://", b"ws://"),
                                      (b"https://", b"http://"), (b"http://", b"http://")):
            for origin in {self.host_header.encode(), f"{self.upstream_host}:{self.upstream_port}".encode()}:
                body = body.replace(scheme_in + origin + self.prefix.encode(), scheme_out + mine)
                body = body.replace(scheme_in + origin, scheme_out + mine)
        return body


async def _pipe(src: asyncio.StreamReader, dst: asyncio.StreamWriter) -> None:
    try:
        while True:
            data = await src.read(CHUNK)
            if not data:
                break
            dst.write(data)
            await dst.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        with contextlib.suppress(Exception):
            if dst.can_write_eof():
                dst.write_eof()
