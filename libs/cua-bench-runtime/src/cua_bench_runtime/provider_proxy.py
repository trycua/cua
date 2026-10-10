"""Bounded, evidence-producing HTTP CONNECT proxy for provider benchmarks.

The proxy deliberately implements only the smallest useful subset of HTTP:
one canonical ``CONNECT hostname:port HTTP/1.1`` request per TCP connection.
DNS resolution is performed by the host after the authority has matched the
allowlist.  Request headers and tunnel contents are never retained in evidence.
"""

from __future__ import annotations

import hashlib
import ipaddress
import re
import select
import socket
import threading
import time
from pathlib import Path
from queue import Empty, Queue
from typing import Any, Iterable

from cua_bench_runtime.canon import canonical_json, digest_json


_IMPLEMENTATION_IDENTITY = "cb.provider-connect-proxy/v2"
_EMPTY_CHAIN = hashlib.sha256(b"cb.provider-connect-proxy/transcript/v2\n").digest()
_ECH_EXTENSION_TYPES = frozenset((0xFE0D, 0xFFCE))
_TRIAL_ID = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9._:-]{0,126}[A-Za-z0-9])?\Z")
_NAT64_WELL_KNOWN_PREFIX = ipaddress.ip_network("64:ff9b::/96")
_NAT64_LOCAL_USE_PREFIX = ipaddress.ip_network("64:ff9b:1::/48")


def _canonical_authority(value: str, *, allow_non_443: bool) -> tuple[str, int]:
    """Validate an already-canonical DNS hostname and decimal port."""

    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError("authority must be a non-empty canonical string")
    if "@" in value or value.count(":") != 1:
        raise ValueError("authority must be hostname:port without userinfo")
    hostname, port_text = value.split(":", 1)
    if hostname != hostname.lower() or hostname.endswith("."):
        raise ValueError("authority hostname must be canonical lowercase DNS")
    try:
        ipaddress.ip_address(hostname)
    except ValueError:
        pass
    else:
        raise ValueError("IP-literal authorities are forbidden")
    if len(hostname) > 253 or not hostname.isascii():
        raise ValueError("authority hostname is not canonical DNS")
    labels = hostname.split(".")
    if any(
        not label
        or len(label) > 63
        or label[0] == "-"
        or label[-1] == "-"
        or any(not (char.islower() or char.isdigit() or char == "-") for char in label)
        for label in labels
    ):
        raise ValueError("authority hostname is not canonical DNS")
    if not port_text.isdigit() or (len(port_text) > 1 and port_text.startswith("0")):
        raise ValueError("authority port must be canonical decimal")
    port = int(port_text)
    if not 1 <= port <= 65535:
        raise ValueError("authority port is out of range")
    if port != 443 and not allow_non_443:
        raise ValueError("non-443 authority requires allow_non_443=True")
    return hostname, port


def _canonical_request_authority(value: str) -> tuple[str, int]:
    """Canonicalize a wire authority while allowing DNS case variation."""

    if not isinstance(value, str) or value != value.strip():
        raise ValueError("authority must be a canonical string")
    if value.count(":") != 1:
        raise ValueError("authority must be hostname:port")
    hostname, port_text = value.split(":", 1)
    return _canonical_authority(
        hostname.lower() + ":" + port_text,
        allow_non_443=True,
    )


def _is_global_upstream_ip(value: ipaddress.IPv4Address | ipaddress.IPv6Address) -> bool:
    """Reject non-global and IPv4-embedding transition addresses."""

    if not value.is_global:
        return False
    if isinstance(value, ipaddress.IPv4Address):
        return True
    # Never send provider traffic through a translator selected by untrusted
    # DNS. In particular, the RFC 6052 well-known and RFC 8215 local-use
    # prefixes can turn an apparently global IPv6 answer into an IPv4 target.
    if value in _NAT64_WELL_KNOWN_PREFIX or value in _NAT64_LOCAL_USE_PREFIX:
        embedded = ipaddress.ip_address(value.packed[-4:])
        if not _is_global_upstream_ip(embedded):
            return False
        # The well-known translator prefix is forbidden even when its embedded
        # destination is public: the proxy dials only direct provider addresses.
        return False
    if value.ipv4_mapped is not None:
        return False
    if value.sixtofour is not None and not value.sixtofour.is_global:
        return False
    if value.teredo is not None:
        server, client = value.teredo
        if not server.is_global or not client.is_global:
            return False
    return True


class ProviderConnectProxy:
    """Synchronous lifecycle wrapper around a bounded threaded CONNECT proxy."""

    def __init__(
        self,
        listen_host: str,
        port: int,
        allowed_authorities: Iterable[str],
        trial_id: str,
        allowed_client_ip: str,
        *,
        allow_non_443: bool = False,
        connect_timeout: float = 5.0,
        idle_timeout: float = 30.0,
        drain_timeout: float = 5.0,
        max_header_bytes: int = 8192,
        max_client_hello_bytes: int = 65536,
        max_connections: int = 8,
        max_bytes_per_connection: int = 512 * 1024 * 1024,
        max_total_bytes: int = 2 * 1024 * 1024 * 1024,
    ) -> None:
        try:
            parsed_listen_ip = ipaddress.ip_address(listen_host)
        except ValueError as error:
            raise ValueError("listen_host must be a canonical IP literal") from error
        if listen_host != str(parsed_listen_ip) or (
            isinstance(parsed_listen_ip, ipaddress.IPv6Address) and parsed_listen_ip.ipv4_mapped
        ):
            raise ValueError("listen_host must be a canonical, non-mapped IP literal")
        if not isinstance(port, int) or not 0 <= port <= 65535:
            raise ValueError("port must be between 0 and 65535")
        try:
            parsed_client_ip = ipaddress.ip_address(allowed_client_ip)
        except ValueError as error:
            raise ValueError("allowed_client_ip must be a canonical IP literal") from error
        if allowed_client_ip != str(parsed_client_ip) or (
            isinstance(parsed_client_ip, ipaddress.IPv6Address) and parsed_client_ip.ipv4_mapped
        ):
            raise ValueError("allowed_client_ip must be a canonical, non-mapped IP literal")
        if parsed_listen_ip.version != parsed_client_ip.version:
            raise ValueError("listen_host and allowed_client_ip must use the same IP family")
        if not isinstance(trial_id, str) or _TRIAL_ID.fullmatch(trial_id) is None:
            raise ValueError("trial_id must be a canonical ASCII token of at most 128 characters")
        authorities = tuple(sorted(set(allowed_authorities)))
        if not authorities:
            raise ValueError("allowed_authorities must not be empty")
        parsed = {
            authority: _canonical_authority(authority, allow_non_443=allow_non_443)
            for authority in authorities
        }
        numeric_bounds = {
            "connect_timeout": connect_timeout,
            "idle_timeout": idle_timeout,
            "drain_timeout": drain_timeout,
            "max_header_bytes": max_header_bytes,
            "max_client_hello_bytes": max_client_hello_bytes,
            "max_connections": max_connections,
            "max_bytes_per_connection": max_bytes_per_connection,
            "max_total_bytes": max_total_bytes,
        }
        if any(value <= 0 for value in numeric_bounds.values()):
            raise ValueError("proxy bounds must be positive")

        self.listen_host = listen_host
        self.port = port
        self.trial_id = trial_id
        self.allowed_client_ip = allowed_client_ip
        self.allowed_authorities = authorities
        self._parsed_authorities = parsed
        self.connect_timeout = float(connect_timeout)
        self.idle_timeout = float(idle_timeout)
        self.drain_timeout = float(drain_timeout)
        self.max_header_bytes = int(max_header_bytes)
        self.max_client_hello_bytes = int(max_client_hello_bytes)
        self.max_connections = int(max_connections)
        self.max_bytes_per_connection = int(max_bytes_per_connection)
        self.max_total_bytes = int(max_total_bytes)
        self._implementation_digest = (
            "sha256:" + hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        )

        self._lock = threading.Lock()
        self._seal_lock = threading.Lock()
        self._listener: socket.socket | None = None
        self._accept_thread: threading.Thread | None = None
        self._workers: set[threading.Thread] = set()
        self._resolvers: set[threading.Thread] = set()
        self._clients: set[socket.socket] = set()
        self._upstreams: set[socket.socket] = set()
        self._stop = threading.Event()
        self._started = False
        self._sealed = False
        self._final_evidence: dict[str, Any] | None = None
        self._accepted = 0
        self._rejected = 0
        self._live_connections = 0
        self._bytes_guest_to_provider = 0
        self._bytes_provider_to_guest = 0
        self._bytes_reserved = 0
        self._chain = _EMPTY_CHAIN

    def start(self) -> dict[str, Any]:
        """Bind the configured endpoint and begin accepting connections."""

        with self._lock:
            if self._sealed:
                raise RuntimeError("proxy is sealed")
            if self._started:
                return self._evidence_locked(active=True, sealed=False)
            listener = socket.socket(socket.AF_INET6 if ":" in self.listen_host else socket.AF_INET)
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            if listener.family == socket.AF_INET6:
                listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
            try:
                listener.bind((self.listen_host, self.port))
                listener.listen(min(self.max_connections, socket.SOMAXCONN))
                listener.settimeout(0.2)
            except BaseException:
                listener.close()
                raise
            self._listener = listener
            self.port = int(listener.getsockname()[1])
            self._started = True
            thread = threading.Thread(
                target=self._accept_loop, name="provider-connect-accept", daemon=True
            )
            self._accept_thread = thread
            thread.start()
            return self._evidence_locked(active=True, sealed=False)

    def seal(self) -> dict[str, Any]:
        """Stop admission, drain boundedly, close sockets, and freeze evidence."""

        with self._seal_lock:
            with self._lock:
                if self._final_evidence is not None:
                    return dict(self._final_evidence)
                self._sealed = True
                self._stop.set()
                listener = self._listener
                self._listener = None
            if listener is not None:
                self._close_socket(listener)
            if (
                self._accept_thread is not None
                and self._accept_thread is not threading.current_thread()
            ):
                self._accept_thread.join()

            deadline = time.monotonic() + self.drain_timeout
            while True:
                with self._lock:
                    workers = tuple(worker for worker in self._workers if worker.is_alive())
                if not workers or time.monotonic() >= deadline:
                    break
                for worker in workers:
                    worker.join(timeout=min(0.05, max(0.0, deadline - time.monotonic())))

            with self._lock:
                sockets = tuple(self._clients | self._upstreams)
                workers = tuple(self._workers)
            for active_socket in sockets:
                self._close_socket(active_socket)
            # DNS waits poll _stop and all other socket operations are bounded;
            # joining here is the quiescence barrier before evidence is frozen.
            for worker in workers:
                if worker is not threading.current_thread():
                    worker.join()

            resolver_deadline = time.monotonic() + self.drain_timeout
            while True:
                with self._lock:
                    resolvers = tuple(
                        resolver for resolver in self._resolvers if resolver.is_alive()
                    )
                if not resolvers or time.monotonic() >= resolver_deadline:
                    break
                for resolver in resolvers:
                    resolver.join(timeout=min(0.05, max(0.0, resolver_deadline - time.monotonic())))

            with self._lock:
                if self._resolvers:
                    raise RuntimeError("proxy resolver threads remain live; evidence not sealed")
                if self._workers or self._clients or self._upstreams:
                    raise RuntimeError("proxy failed to become quiescent")
                self._record_locked("sealed")
                self._final_evidence = self._evidence_locked(active=False, sealed=True)
                return dict(self._final_evidence)

    def close(self) -> None:
        """Idempotently release all resources."""

        self.seal()

    def abort(self) -> None:
        """Immediately revoke admission and every live tunnel without sealing.

        Emergency trial cleanup uses this before any guest RPC.  Unlike
        :meth:`seal`, it never waits for a drain deadline; a later ``close``
        still performs the quiescence barrier and freezes final evidence.
        """

        with self._seal_lock:
            with self._lock:
                if self._final_evidence is not None:
                    return
                self._sealed = True
                self._stop.set()
                listener = self._listener
                self._listener = None
                sockets = tuple(self._clients | self._upstreams)
            if listener is not None:
                self._close_socket(listener)
            for active_socket in sockets:
                self._close_socket(active_socket)

    def __enter__(self) -> "ProviderConnectProxy":
        self.start()
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.close()

    def _accept_loop(self) -> None:
        while not self._stop.is_set():
            with self._lock:
                listener = self._listener
            if listener is None:
                return
            try:
                client, peer = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            client.settimeout(self.idle_timeout)
            try:
                parsed_peer_ip = ipaddress.ip_address(peer[0])
                if isinstance(parsed_peer_ip, ipaddress.IPv6Address) and parsed_peer_ip.ipv4_mapped:
                    raise ValueError("mapped peer")
                peer_ip = str(parsed_peer_ip)
            except ValueError:
                peer_ip = ""
            with self._lock:
                wrong_client = peer_ip != self.allowed_client_ip
                over_limit = self._live_connections >= self.max_connections
                if not wrong_client and not over_limit:
                    self._clients.add(client)
                    self._live_connections += 1
            if wrong_client or over_limit:
                self._reject(client, "client_binding" if wrong_client else "connection_limit")
                self._close_socket(client)
                continue
            worker = threading.Thread(
                target=self._serve, args=(client,), name="provider-connect-relay", daemon=True
            )
            with self._lock:
                self._workers.add(worker)
            try:
                worker.start()
            except BaseException:
                with self._lock:
                    self._workers.discard(worker)
                    self._clients.discard(client)
                    self._live_connections -= 1
                self._reject(client, "worker")
                self._close_socket(client)

    def _serve(self, client: socket.socket) -> None:
        upstream: socket.socket | None = None
        established = False
        tunnel_announced = False
        hello_reserved = 0
        try:
            request = self._read_request(client)
            authority = self._parse_request(request)
            if authority not in self._parsed_authorities:
                raise ValueError("authority")
            hostname, port = self._parsed_authorities[authority]
            client.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            tunnel_announced = True
            client_hello = self._read_tls_client_hello(client, hostname)
            with self._lock:
                total = self._bytes_guest_to_provider + self._bytes_provider_to_guest
                if (
                    len(client_hello) > self.max_bytes_per_connection
                    or len(client_hello) > self.max_total_bytes - total - self._bytes_reserved
                ):
                    raise ValueError("client hello exceeds byte bound")
                hello_reserved = len(client_hello)
                self._bytes_reserved += hello_reserved
            upstream = self._connect_host(hostname, port)
            upstream.sendall(client_hello)
            with self._lock:
                self._bytes_reserved -= hello_reserved
                hello_reserved = 0
                self._bytes_guest_to_provider += len(client_hello)
                self._accepted += 1
                self._record_locked("accepted")
            established = True
            self._relay(client, upstream, initial_bytes=len(client_hello))
        except (ValueError, UnicodeError):
            if not established:
                self._reject(client, "request", send_response=not tunnel_announced)
        except (OSError, TimeoutError):
            if not established:
                self._reject(client, "upstream", send_response=not tunnel_announced)
        finally:
            if hello_reserved:
                with self._lock:
                    self._bytes_reserved -= hello_reserved
            if upstream is not None:
                self._close_socket(upstream)
            self._close_socket(client)
            with self._lock:
                self._clients.discard(client)
                if upstream is not None:
                    self._upstreams.discard(upstream)
                self._live_connections -= 1
                self._workers.discard(threading.current_thread())

    def _read_request(self, client: socket.socket) -> bytes:
        data = bytearray()
        while b"\r\n\r\n" not in data:
            if len(data) >= self.max_header_bytes:
                raise ValueError("header bound")
            chunk = client.recv(min(1024, self.max_header_bytes - len(data)))
            if not chunk:
                raise ValueError("incomplete header")
            data.extend(chunk)
        end = data.find(b"\r\n\r\n") + 4
        if end != len(data):
            # Pipelined/tunnel bytes before authorization create ambiguous parsing.
            raise ValueError("early payload")
        return bytes(data)

    @staticmethod
    def _parse_request(request: bytes) -> str:
        if b"\x00" in request or b"\n" in request.replace(b"\r\n", b""):
            raise ValueError("invalid framing")
        text = request.decode("ascii")
        lines = text[:-4].split("\r\n")
        if not lines or len(lines[0].split(" ")) != 3:
            raise ValueError("request line")
        method, authority, version = lines[0].split(" ")
        if method != "CONNECT" or version != "HTTP/1.1":
            raise ValueError("method or version")
        canonical_authority = _canonical_request_authority(authority)
        headers: dict[str, str] = {}
        for line in lines[1:]:
            if not line or line[0].isspace() or ":" not in line:
                raise ValueError("header")
            name, value = line.split(":", 1)
            lowered = name.lower()
            if lowered in headers:
                raise ValueError("duplicate header")
            if lowered == "host":
                if not value.startswith(" ") or value != " " + value[1:].strip(" "):
                    raise ValueError("host header")
                if _canonical_request_authority(value[1:]) != canonical_authority:
                    raise ValueError("host header")
            elif lowered == "user-agent":
                user_agent = value[1:] if value.startswith(" ") else ""
                if (
                    not user_agent
                    or user_agent != user_agent.strip(" ")
                    or len(user_agent) > 256
                    or any(ord(char) < 0x20 or ord(char) > 0x7E for char in user_agent)
                ):
                    raise ValueError("user-agent header")
            elif lowered in ("proxy-connection", "connection"):
                if not value.startswith(" ") or value[1:].lower() not in ("keep-alive", "close"):
                    raise ValueError("connection header")
            else:
                # This includes all body framing, transfer coding, and
                # authorization headers. CONNECT needs none of them here.
                raise ValueError("unsupported header")
            headers[lowered] = value
        if "host" not in headers:
            raise ValueError("host header")
        hostname, port = canonical_authority
        return f"{hostname}:{port}"

    def _read_tls_client_hello(self, client: socket.socket, expected_hostname: str) -> bytes:
        wire = bytearray()
        handshake = bytearray()
        needed: int | None = None
        while needed is None or len(handshake) < needed:
            header = self._recv_exact(client, 5)
            content_type = header[0]
            record_length = int.from_bytes(header[3:5], "big")
            if content_type != 22 or record_length == 0:
                raise ValueError("first TLS flight is not a handshake")
            if len(wire) + 5 + record_length > self.max_client_hello_bytes:
                raise ValueError("client hello bound")
            payload = self._recv_exact(client, record_length)
            wire.extend(header)
            wire.extend(payload)
            handshake.extend(payload)
            if needed is None and len(handshake) >= 4:
                if handshake[0] != 1:
                    raise ValueError("first TLS handshake is not ClientHello")
                needed = 4 + int.from_bytes(handshake[1:4], "big")
                if needed > self.max_client_hello_bytes:
                    raise ValueError("client hello bound")
        if needed is None or len(handshake) != needed:
            raise ValueError("ambiguous ClientHello framing")
        sni = self._parse_client_hello_sni(bytes(handshake[4:]))
        if sni != expected_hostname:
            raise ValueError("ClientHello SNI does not match CONNECT hostname")
        return bytes(wire)

    @staticmethod
    def _parse_client_hello_sni(body: bytes) -> str:
        def take(offset: int, size: int) -> tuple[bytes, int]:
            end = offset + size
            if size < 0 or end > len(body):
                raise ValueError("truncated ClientHello")
            return body[offset:end], end

        _, offset = take(0, 34)  # legacy_version and random
        session_size_raw, offset = take(offset, 1)
        _, offset = take(offset, session_size_raw[0])
        cipher_size_raw, offset = take(offset, 2)
        cipher_size = int.from_bytes(cipher_size_raw, "big")
        if cipher_size < 2 or cipher_size % 2:
            raise ValueError("invalid cipher suites")
        _, offset = take(offset, cipher_size)
        compression_size_raw, offset = take(offset, 1)
        if not compression_size_raw[0]:
            raise ValueError("invalid compression methods")
        _, offset = take(offset, compression_size_raw[0])
        extensions_size_raw, offset = take(offset, 2)
        extensions_size = int.from_bytes(extensions_size_raw, "big")
        extensions, offset = take(offset, extensions_size)
        if offset != len(body):
            raise ValueError("trailing ClientHello bytes")

        names: list[str] = []
        extension_offset = 0
        seen_extensions: set[int] = set()
        while extension_offset < len(extensions):
            if extension_offset + 4 > len(extensions):
                raise ValueError("truncated extension")
            extension_type = int.from_bytes(
                extensions[extension_offset : extension_offset + 2], "big"
            )
            extension_size = int.from_bytes(
                extensions[extension_offset + 2 : extension_offset + 4], "big"
            )
            extension_offset += 4
            extension_end = extension_offset + extension_size
            if extension_end > len(extensions) or extension_type in seen_extensions:
                raise ValueError("invalid extension")
            seen_extensions.add(extension_type)
            extension = extensions[extension_offset:extension_end]
            extension_offset = extension_end
            if extension_type in _ECH_EXTENSION_TYPES:
                raise ValueError("ECH is not permitted")
            if extension_type == 0:
                if len(extension) < 2 or int.from_bytes(extension[:2], "big") != len(extension) - 2:
                    raise ValueError("invalid server_name extension")
                name_offset = 2
                while name_offset < len(extension):
                    if name_offset + 3 > len(extension):
                        raise ValueError("truncated server name")
                    name_type = extension[name_offset]
                    name_size = int.from_bytes(extension[name_offset + 1 : name_offset + 3], "big")
                    name_offset += 3
                    name_end = name_offset + name_size
                    if name_type != 0 or name_end > len(extension):
                        raise ValueError("invalid server name")
                    names.append(extension[name_offset:name_end].decode("ascii"))
                    name_offset = name_end
        if len(names) != 1:
            raise ValueError("exactly one plaintext SNI hostname is required")
        return names[0]

    @staticmethod
    def _recv_exact(client: socket.socket, size: int) -> bytes:
        chunks = bytearray()
        while len(chunks) < size:
            chunk = client.recv(size - len(chunks))
            if not chunk:
                raise ValueError("truncated TLS flight")
            chunks.extend(chunk)
        return bytes(chunks)

    def _connect_host(self, hostname: str, port: int) -> socket.socket:
        last_error: OSError | None = None
        deadline = time.monotonic() + self.connect_timeout
        result: Queue[tuple[list[tuple[Any, ...]] | None, BaseException | None]] = Queue(maxsize=1)

        def resolve() -> None:
            try:
                addresses = socket.getaddrinfo(hostname, port, type=socket.SOCK_STREAM)
                result.put((addresses, None))
            except BaseException as error:  # handed back to the bounded caller
                result.put((None, error))
            finally:
                with self._lock:
                    self._resolvers.discard(threading.current_thread())

        resolver = threading.Thread(target=resolve, name="provider-connect-dns", daemon=True)
        with self._lock:
            if self._sealed:
                raise OSError("proxy is sealing")
            self._resolvers.add(resolver)
        try:
            resolver.start()
        except BaseException:
            with self._lock:
                self._resolvers.discard(resolver)
            raise
        while True:
            if self._stop.is_set():
                raise OSError("proxy is sealing")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("host resolution timed out")
            try:
                addresses, resolution_error = result.get(timeout=min(0.05, remaining))
                break
            except Empty:
                continue
        if resolution_error is not None:
            if isinstance(resolution_error, OSError):
                raise resolution_error
            raise OSError("host resolution failed") from resolution_error
        assert addresses is not None
        if not addresses:
            raise OSError("hostname resolved to no stream addresses")
        validated: list[tuple[Any, ...]] = []
        for address in addresses:
            family, socktype, proto, _, sockaddr = address
            if family not in (socket.AF_INET, socket.AF_INET6) or socktype != socket.SOCK_STREAM:
                raise OSError("hostname resolved to an invalid address family")
            try:
                resolved_ip = ipaddress.ip_address(sockaddr[0])
            except ValueError as error:
                raise OSError("hostname resolved to an invalid IP literal") from error
            if (
                not _is_global_upstream_ip(resolved_ip)
                or (isinstance(resolved_ip, ipaddress.IPv6Address) and resolved_ip.ipv4_mapped)
                or (family == socket.AF_INET) != isinstance(resolved_ip, ipaddress.IPv4Address)
                or (family == socket.AF_INET6 and len(sockaddr) >= 4 and sockaddr[3] != 0)
            ):
                raise OSError("hostname resolved to a non-global IP address")
            validated.append(address)
        for family, socktype, proto, _, sockaddr in validated:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or self._stop.is_set():
                raise TimeoutError("upstream connection timed out")
            upstream = socket.socket(family, socktype, proto)
            upstream.settimeout(remaining)
            with self._lock:
                if self._sealed:
                    upstream.close()
                    raise OSError("proxy is sealing")
                self._upstreams.add(upstream)
            try:
                upstream.connect(sockaddr)
                upstream.settimeout(self.idle_timeout)
                return upstream
            except OSError as error:
                last_error = error
                with self._lock:
                    self._upstreams.discard(upstream)
                upstream.close()
        if last_error is not None:
            raise last_error
        raise OSError("hostname resolved to no stream addresses")

    def _relay(
        self, client: socket.socket, upstream: socket.socket, *, initial_bytes: int = 0
    ) -> None:
        sockets = (client, upstream)
        per_connection = initial_bytes
        while True:
            readable, _, _ = select.select(sockets, (), (), self.idle_timeout)
            if not readable:
                return
            for source in readable:
                destination = upstream if source is client else client
                with self._lock:
                    total = self._bytes_guest_to_provider + self._bytes_provider_to_guest
                    allowance = min(
                        65536,
                        self.max_bytes_per_connection - per_connection,
                        self.max_total_bytes - total - self._bytes_reserved,
                    )
                    if allowance > 0:
                        self._bytes_reserved += allowance
                if allowance <= 0:
                    return
                try:
                    data = source.recv(allowance)
                    if not data:
                        return
                    destination.sendall(data)
                    amount = len(data)
                    per_connection += amount
                    with self._lock:
                        if source is client:
                            self._bytes_guest_to_provider += amount
                        else:
                            self._bytes_provider_to_guest += amount
                finally:
                    with self._lock:
                        self._bytes_reserved -= allowance

    def _reject(self, client: socket.socket, category: str, *, send_response: bool = True) -> None:
        with self._lock:
            self._rejected += 1
            self._record_locked("rejected:" + category)
        if send_response:
            try:
                client.sendall(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n")
            except OSError:
                pass

    def _record_locked(self, event: str) -> None:
        # Events contain only fixed classifications and monotonic counters.
        body = canonical_json({"event": event, "sequence": self._accepted + self._rejected})
        self._chain = hashlib.sha256(self._chain + b"\n" + body).digest()

    def _evidence_locked(self, *, active: bool, sealed: bool) -> dict[str, Any]:
        endpoint_host = f"[{self.listen_host}]" if ":" in self.listen_host else self.listen_host
        endpoint = f"http://{endpoint_host}:{self.port}"
        return {
            "schema_version": 1,
            "trial_id": self.trial_id,
            "endpoint": endpoint,
            "allowed_client_ip": self.allowed_client_ip,
            "client_binding_digest": digest_json(
                {
                    "allowed_authorities": list(self.allowed_authorities),
                    "allowed_client_ip": self.allowed_client_ip,
                    "endpoint": endpoint,
                }
            ),
            "allowed_authorities_digest": digest_json(list(self.allowed_authorities)),
            "implementation_identity": _IMPLEMENTATION_IDENTITY,
            "implementation_digest": self._implementation_digest,
            "accepted_connections": self._accepted,
            "rejected_connections": self._rejected,
            "bytes_guest_to_provider": self._bytes_guest_to_provider,
            "bytes_provider_to_guest": self._bytes_provider_to_guest,
            "transcript_chain_digest": "sha256:" + self._chain.hex(),
            "active": active,
            "sealed": sealed,
        }

    @staticmethod
    def _close_socket(sock: socket.socket) -> None:
        try:
            sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            sock.close()
        except OSError:
            pass
