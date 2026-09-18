from __future__ import annotations

import json
import hashlib
import socket
import threading
import time
import unittest
from unittest.mock import patch

from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.provider_proxy import ProviderConnectProxy


class LoopbackProviderConnectProxy(ProviderConnectProxy):
    """Test-only upstream dialer; production rejects loopback DNS by design."""

    def _connect_host(self, hostname: str, port: int) -> socket.socket:
        upstream = socket.create_connection(("127.0.0.1", port), timeout=self.connect_timeout)
        upstream.settimeout(self.idle_timeout)
        with self._lock:
            if self._sealed:
                upstream.close()
                raise OSError("proxy is sealing")
            self._upstreams.add(upstream)
        return upstream


class EchoServer:
    def __init__(self) -> None:
        self.listener = socket.socket()
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.port = self.listener.getsockname()[1]
        self.stop = threading.Event()
        self.accepted = 0
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self) -> None:
        self.listener.settimeout(0.1)
        while not self.stop.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self.accepted += 1
            threading.Thread(target=self._echo, args=(client,), daemon=True).start()

    @staticmethod
    def _echo(client: socket.socket) -> None:
        with client:
            try:
                while data := client.recv(4096):
                    client.sendall(data)
            except (ConnectionAbortedError, ConnectionResetError):
                pass

    def close(self) -> None:
        self.stop.set()
        self.listener.close()
        self.thread.join(timeout=1)


def connect(proxy: ProviderConnectProxy, request: bytes) -> socket.socket:
    client = socket.create_connection((proxy.listen_host, proxy.port), timeout=2)
    client.settimeout(2)
    client.sendall(request)
    return client


def tls_client_hello(hostname: str | None, *, ech: bool = False) -> bytes:
    extensions = bytearray()
    if hostname is not None:
        encoded = hostname.encode("ascii")
        names = b"\x00" + len(encoded).to_bytes(2, "big") + encoded
        server_name = len(names).to_bytes(2, "big") + names
        extensions.extend(b"\x00\x00" + len(server_name).to_bytes(2, "big") + server_name)
    if ech:
        extensions.extend(b"\xfe\x0d\x00\x00")
    body = (
        b"\x03\x03"
        + bytes(32)
        + b"\x00"
        + b"\x00\x02\x13\x01"
        + b"\x01\x00"
        + len(extensions).to_bytes(2, "big")
        + extensions
    )
    handshake = b"\x01" + len(body).to_bytes(3, "big") + body
    return b"\x16\x03\x01" + len(handshake).to_bytes(2, "big") + handshake


def recv_exact(client: socket.socket, size: int) -> bytes:
    result = bytearray()
    while len(result) < size:
        chunk = client.recv(size - len(result))
        if not chunk:
            break
        result.extend(chunk)
    return bytes(result)


def wait_for_no_live_connections(proxy: ProviderConnectProxy) -> None:
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        with proxy._lock:
            if proxy._live_connections == 0:
                return
        time.sleep(0.01)
    raise AssertionError("proxy connection did not close")


class ProviderProxyTests(unittest.TestCase):
    def test_abort_revokes_admission_before_later_seal(self) -> None:
        proxy = ProviderConnectProxy("127.0.0.1", 0, ["localhost:443"], "trial", "127.0.0.1")
        proxy.start()
        port = proxy.port
        proxy.abort()
        try:
            with self.assertRaises(OSError):
                socket.create_connection(("127.0.0.1", port), timeout=0.2)
            evidence = proxy.seal()
        finally:
            proxy.close()
        self.assertFalse(evidence["active"])
        self.assertTrue(evidence["sealed"])

    def test_allowed_connect_relays_and_evidence_is_content_free(self) -> None:
        upstream = EchoServer()
        authority = f"localhost:{upstream.port}"
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1", 0, [authority], "trial-safe", "127.0.0.1", allow_non_443=True
        )
        initial = proxy.start()
        try:
            with connect(
                proxy,
                f"CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n".encode(),
            ) as client:
                assert client.recv(4096) == b"HTTP/1.1 200 Connection Established\r\n\r\n"
                hello = tls_client_hello("localhost")
                client.sendall(hello)
                self.assertEqual(recv_exact(client, len(hello)), hello)
                secret = b"credential prompt response secret"
                client.sendall(secret)
                assert client.recv(len(secret)) == secret
            final = proxy.seal()
        finally:
            proxy.close()
            upstream.close()

        self.assertEqual(initial["endpoint"], f"http://127.0.0.1:{proxy.port}")
        self.assertEqual(initial["allowed_authorities_digest"], digest_json([authority]))
        self.assertTrue(initial["active"] and not initial["sealed"])
        self.assertTrue(not final["active"] and final["sealed"])
        self.assertEqual(final["accepted_connections"], 1)
        self.assertEqual(final["bytes_guest_to_provider"], len(hello) + len(secret))
        self.assertEqual(final["bytes_provider_to_guest"], len(hello) + len(secret))
        serialized = json.dumps(final)
        assert authority not in serialized
        for forbidden in ("credential", "prompt", "response", "secret", "Host"):
            self.assertNotIn(forbidden, serialized)

    def test_curl_shaped_connect_headers_are_accepted_without_entering_evidence(self) -> None:
        upstream = EchoServer()
        authority = f"localhost:{upstream.port}"
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1", 0, [authority], "trial", "127.0.0.1", allow_non_443=True
        )
        proxy.start()
        try:
            request = (
                f"CONNECT LOCALHOST:{upstream.port} HTTP/1.1\r\n"
                f"hOsT: localhost:{upstream.port}\r\n"
                "user-agent: curl/8.7.1\r\n"
                "pRoXy-CoNnEcTiOn: Keep-Alive\r\n\r\n"
            ).encode()
            with connect(proxy, request) as client:
                self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 200"))
                hello = tls_client_hello("localhost")
                client.sendall(hello)
                self.assertEqual(recv_exact(client, len(hello)), hello)
            evidence = proxy.seal()
        finally:
            proxy.close()
            upstream.close()
        self.assertEqual(evidence["accepted_connections"], 1)
        serialized = json.dumps(evidence)
        self.assertNotIn("curl", serialized)
        self.assertNotIn("User-Agent", serialized)

    def test_disallowed_and_malformed_requests_fail_closed(self) -> None:
        requests = [
            b"GET localhost:443 HTTP/1.1\r\nHost: localhost:443\r\n\r\n",
            b"CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n\r\n",
            b"CONNECT user@localhost:443 HTTP/1.1\r\nHost: user@localhost:443\r\n\r\n",
            b"CONNECT other.test:443 HTTP/1.1\r\nHost: other.test:443\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\nContent-Length: 1\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\nProxy-Authorization: Basic secret\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\nHost: localhost:443\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\nUser-Agent: one\r\nUser-Agent: two\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\nProxy-Connection: upgrade\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nhost: localhost:444\r\n\r\n",
            b"CONNECT localhost:0443 HTTP/1.1\r\nhost: localhost:0443\r\n\r\n",
            b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\n\r\nsmuggled",
        ]
        for request in requests:
            with self.subTest(request=request):
                proxy = ProviderConnectProxy(
                    "127.0.0.1", 0, ["localhost:443"], "trial", "127.0.0.1"
                )
                proxy.start()
                try:
                    with connect(proxy, request) as client:
                        self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 403"))
                    self.assertEqual(proxy.seal()["rejected_connections"], 1)
                finally:
                    proxy.close()

    def test_allowlist_requires_canonical_dns_authorities(self) -> None:
        for authority in (
            "Example.com:443",
            "127.0.0.1:443",
            "[::1]:443",
            "user@example.com:443",
            "example.com:0443",
        ):
            with self.subTest(authority=authority), self.assertRaises(ValueError):
                ProviderConnectProxy("127.0.0.1", 0, [authority], "trial", "127.0.0.1")

    def test_client_ip_binding_rejects_other_loopback_peer(self) -> None:
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1", 0, ["localhost:443"], "trial", "127.0.0.2"
        )
        initial = proxy.start()
        try:
            with connect(
                proxy, b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost:443\r\n\r\n"
            ) as client:
                try:
                    response = client.recv(4096)
                except (ConnectionAbortedError, ConnectionResetError):
                    # Windows may reset a peer-rejected connection because its
                    # queued request is intentionally never consumed.
                    response = b""
                if response:
                    self.assertTrue(response.startswith(b"HTTP/1.1 403"))
            final = proxy.seal()
        finally:
            proxy.close()
        self.assertEqual(initial["allowed_client_ip"], "127.0.0.2")
        self.assertTrue(str(initial["client_binding_digest"]).startswith("sha256:"))
        self.assertEqual(final["rejected_connections"], 1)

    def test_sni_is_required_exact_and_ech_is_rejected_before_upstream(self) -> None:
        cases = (
            tls_client_hello(None),
            tls_client_hello("other.test"),
            tls_client_hello("localhost", ech=True),
        )
        for hello in cases:
            with self.subTest(hello=hello):
                upstream = EchoServer()
                authority = f"localhost:{upstream.port}"
                proxy = ProviderConnectProxy(
                    "127.0.0.1",
                    0,
                    [authority],
                    "trial",
                    "127.0.0.1",
                    allow_non_443=True,
                )
                proxy.start()
                try:
                    request = f"CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n".encode()
                    with connect(proxy, request) as client:
                        self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 200"))
                        client.sendall(hello)
                        self.assertEqual(client.recv(4096), b"")
                    evidence = proxy.seal()
                finally:
                    proxy.close()
                    upstream.close()
                self.assertEqual(upstream.accepted, 0)
                self.assertEqual(evidence["accepted_connections"], 0)
                self.assertEqual(evidence["rejected_connections"], 1)

    def test_dns_rebinding_rejects_any_non_global_ipv4_or_ipv6_result(self) -> None:
        proxy = ProviderConnectProxy("127.0.0.1", 0, ["provider.test:443"], "trial", "127.0.0.1")
        answers = (
            [
                (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("8.8.8.8", 443)),
                (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 443)),
            ],
            [
                (socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("2606:4700:4700::1111", 443, 0, 0)),
                (socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("fe80::1", 443, 0, 2)),
            ],
            [(socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("::ffff:8.8.8.8", 443, 0, 0))],
        )
        for answer in answers:
            with (
                self.subTest(answer=answer),
                patch("cua_bench_runtime.provider_proxy.socket.getaddrinfo", return_value=answer),
                self.assertRaisesRegex(OSError, "non-global"),
            ):
                proxy._connect_host("provider.test", 443)

    def test_nat64_and_ipv4_embedding_answers_are_rejected(self) -> None:
        proxy = ProviderConnectProxy("127.0.0.1", 0, ["provider.test:443"], "trial", "127.0.0.1")
        addresses = (
            "64:ff9b::808:808",  # RFC 6052 well-known prefix, global IPv4 payload
            "64:ff9b::7f00:1",  # RFC 6052 well-known prefix, loopback payload
            "64:ff9b:1::808:808",  # RFC 8215 local-use NAT64 prefix
            "64:ff9b:1::7f00:1",  # local-use translator to loopback payload
            "2002:7f00:1::",  # 6to4 with an embedded loopback IPv4 address
        )
        for address in addresses:
            answer = [(socket.AF_INET6, socket.SOCK_STREAM, 6, "", (address, 443, 0, 0))]
            with (
                self.subTest(address=address),
                patch("cua_bench_runtime.provider_proxy.socket.getaddrinfo", return_value=answer),
                self.assertRaisesRegex(OSError, "non-global"),
            ):
                proxy._connect_host("provider.test", 443)

    def test_implementation_digest_is_snapshotted_before_start(self) -> None:
        source = b"immutable implementation bytes"
        with patch("cua_bench_runtime.provider_proxy.Path.read_bytes", return_value=source):
            proxy = ProviderConnectProxy(
                "127.0.0.1", 0, ["provider.test:443"], "trial", "127.0.0.1"
            )
        with patch(
            "cua_bench_runtime.provider_proxy.Path.read_bytes", return_value=b"mutated later"
        ):
            initial = proxy.start()
            final = proxy.seal()
        expected = "sha256:" + hashlib.sha256(source).hexdigest()
        self.assertEqual(initial["implementation_digest"], expected)
        self.assertEqual(final["implementation_digest"], expected)

    def test_seal_refuses_evidence_until_blocked_resolver_is_quiescent(self) -> None:
        resolver_started = threading.Event()
        release_resolver = threading.Event()

        def blocked_resolver(*args: object, **kwargs: object) -> list[object]:
            resolver_started.set()
            release_resolver.wait(timeout=5)
            return [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("8.8.8.8", 443))]

        proxy = ProviderConnectProxy(
            "127.0.0.1",
            0,
            ["provider.test:443"],
            "trial",
            "127.0.0.1",
            connect_timeout=2,
            drain_timeout=0.2,
        )
        proxy.start()
        client = connect(
            proxy, b"CONNECT provider.test:443 HTTP/1.1\r\nHost: provider.test:443\r\n\r\n"
        )
        try:
            self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 200"))
            with patch(
                "cua_bench_runtime.provider_proxy.socket.getaddrinfo", side_effect=blocked_resolver
            ):
                client.sendall(tls_client_hello("provider.test"))
                self.assertTrue(resolver_started.wait(timeout=1))
                started = time.monotonic()
                with self.assertRaisesRegex(RuntimeError, "resolver threads remain live"):
                    proxy.seal()
                self.assertLess(time.monotonic() - started, 1.0)
                self.assertIsNone(proxy._final_evidence)
                self.assertFalse(proxy._workers)
                self.assertFalse(proxy._clients)
                self.assertFalse(proxy._upstreams)
                self.assertTrue(proxy._resolvers)
                release_resolver.set()
                final = proxy.seal()
                self.assertFalse(proxy._resolvers)
                self.assertFalse(
                    any(thread.name == "provider-connect-dns" for thread in threading.enumerate())
                )
                self.assertEqual(proxy.seal(), final)
        finally:
            release_resolver.set()
            client.close()
            proxy.close()

    def test_canonical_trial_and_ip_inputs_and_ipv6_only_listener(self) -> None:
        invalid = (
            ("localhost", "127.0.0.1", "trial"),
            ("127.0.0.1", "::1", "trial"),
            ("::ffff:127.0.0.1", "::ffff:127.0.0.1", "trial"),
            ("127.0.0.1", "127.0.0.1", "bad trial"),
            ("127.0.0.1", "127.0.0.1", "x" * 129),
        )
        for listen_host, client_ip, trial_id in invalid:
            with (
                self.subTest(values=(listen_host, client_ip, trial_id)),
                self.assertRaises(ValueError),
            ):
                ProviderConnectProxy(listen_host, 0, ["provider.test:443"], trial_id, client_ip)

        proxy = ProviderConnectProxy("::1", 0, ["provider.test:443"], "trial", "::1")
        try:
            evidence = proxy.start()
            assert proxy._listener is not None
            self.assertEqual(proxy._listener.getsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY), 1)
            self.assertEqual(evidence["endpoint"], f"http://[::1]:{proxy.port}")
        finally:
            proxy.close()

    def test_header_connection_and_byte_bounds(self) -> None:
        upstream = EchoServer()
        authority = f"localhost:{upstream.port}"
        hello = tls_client_hello("localhost")
        byte_bound = len(hello) * 2 + 5
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1",
            0,
            [authority],
            "trial",
            "127.0.0.1",
            allow_non_443=True,
            max_header_bytes=96,
            max_connections=2,
            max_bytes_per_connection=byte_bound,
            max_total_bytes=byte_bound,
        )
        proxy.start()
        try:
            with connect(proxy, b"X" * 96) as oversized:
                self.assertTrue(oversized.recv(4096).startswith(b"HTTP/1.1 403"))
            request = f"CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n".encode()
            with connect(proxy, request) as client:
                self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 200"))
                client.sendall(hello)
                self.assertEqual(recv_exact(client, len(hello)), hello)
                client.sendall(b"123456789")
                try:
                    self.assertEqual(client.recv(20), b"")
                except ConnectionResetError:
                    pass
            evidence = proxy.seal()
        finally:
            proxy.close()
            upstream.close()
        self.assertEqual(evidence["accepted_connections"], 1)
        self.assertEqual(evidence["rejected_connections"], 1)
        self.assertLessEqual(evidence["bytes_guest_to_provider"], len(hello) + 5)
        self.assertLessEqual(
            evidence["bytes_guest_to_provider"] + evidence["bytes_provider_to_guest"], byte_bound
        )

    def test_connection_limit_is_concurrent_not_lifetime(self) -> None:
        upstream = EchoServer()
        authority = f"localhost:{upstream.port}"
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1",
            0,
            [authority],
            "trial",
            "127.0.0.1",
            allow_non_443=True,
            max_connections=1,
        )
        proxy.start()
        request = f"CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n".encode()
        hello = tls_client_hello("localhost")
        try:
            for _ in range(3):
                with connect(proxy, request) as client:
                    self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 200"))
                    client.sendall(hello)
                    self.assertEqual(recv_exact(client, len(hello)), hello)
                wait_for_no_live_connections(proxy)
            evidence = proxy.seal()
        finally:
            proxy.close()
            upstream.close()
        self.assertEqual(evidence["accepted_connections"], 3)
        self.assertEqual(evidence["rejected_connections"], 0)

    def test_connection_limit_rejects_only_while_capacity_is_live(self) -> None:
        upstream = EchoServer()
        authority = f"localhost:{upstream.port}"
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1",
            0,
            [authority],
            "trial",
            "127.0.0.1",
            allow_non_443=True,
            max_connections=1,
        )
        proxy.start()
        request = f"CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n".encode()
        first = connect(proxy, request)
        try:
            self.assertTrue(first.recv(4096).startswith(b"HTTP/1.1 200"))
            with connect(proxy, request) as excess:
                try:
                    response = excess.recv(4096)
                except (ConnectionAbortedError, ConnectionResetError):
                    # Windows can reset a capacity-rejected connection because the
                    # proxy closes before consuming its queued CONNECT request.
                    # The evidence assertion below remains the rejection oracle.
                    response = b""
                if response:
                    self.assertTrue(response.startswith(b"HTTP/1.1 403"))
            first.close()
            wait_for_no_live_connections(proxy)
            with connect(proxy, request) as replacement:
                self.assertTrue(replacement.recv(4096).startswith(b"HTTP/1.1 200"))
                hello = tls_client_hello("localhost")
                replacement.sendall(hello)
                self.assertEqual(recv_exact(replacement, len(hello)), hello)
            evidence = proxy.seal()
        finally:
            first.close()
            proxy.close()
            upstream.close()
        self.assertEqual(evidence["accepted_connections"], 1)
        self.assertEqual(evidence["rejected_connections"], 2)

    def test_rejected_request_does_not_starve_connection_capacity(self) -> None:
        upstream = EchoServer()
        authority = f"localhost:{upstream.port}"
        proxy = LoopbackProviderConnectProxy(
            "127.0.0.1",
            0,
            [authority],
            "trial",
            "127.0.0.1",
            allow_non_443=True,
            max_connections=1,
        )
        proxy.start()
        try:
            with connect(proxy, b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n") as rejected:
                self.assertTrue(rejected.recv(4096).startswith(b"HTTP/1.1 403"))
            wait_for_no_live_connections(proxy)
            request = (
                f"CONNECT {authority} HTTP/1.1\r\nhost: LOCALHOST:{upstream.port}\r\n\r\n".encode()
            )
            with connect(proxy, request) as client:
                self.assertTrue(client.recv(4096).startswith(b"HTTP/1.1 200"))
                hello = tls_client_hello("localhost")
                client.sendall(hello)
                self.assertEqual(recv_exact(client, len(hello)), hello)
            evidence = proxy.seal()
        finally:
            proxy.close()
            upstream.close()
        self.assertEqual(evidence["accepted_connections"], 1)
        self.assertEqual(evidence["rejected_connections"], 1)

    def test_lifecycle_exact_bind_seal_cleanup_and_idempotency(self) -> None:
        reserved = socket.socket()
        reserved.bind(("127.0.0.1", 0))
        occupied_port = reserved.getsockname()[1]
        blocked = ProviderConnectProxy(
            "127.0.0.1", occupied_port, ["localhost:443"], "trial", "127.0.0.1"
        )
        with self.assertRaises(OSError):
            blocked.start()
        reserved.close()

        proxy = ProviderConnectProxy(
            "127.0.0.1", occupied_port, ["localhost:443"], "trial", "127.0.0.1"
        )
        first = proxy.start()
        self.assertEqual(proxy.start(), first)
        final = proxy.seal()
        self.assertEqual(proxy.seal(), final)
        proxy.close()
        proxy.close()
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", occupied_port), timeout=0.2)
        with self.assertRaises(RuntimeError):
            proxy.start()

    def test_evidence_digests_are_deterministic_for_equivalent_idle_proxies(self) -> None:
        def run_once() -> dict[str, object]:
            proxy = ProviderConnectProxy(
                "127.0.0.1", 0, ["z.test:443", "a.test:443"], "same", "127.0.0.1"
            )
            proxy.start()
            evidence = proxy.seal()
            proxy.close()
            return evidence

        left, right = run_once(), run_once()
        ignored = {"endpoint", "client_binding_digest"}
        self.assertEqual(
            {k: v for k, v in left.items() if k not in ignored},
            {k: v for k, v in right.items() if k not in ignored},
        )
        self.assertEqual(
            left["allowed_authorities_digest"], digest_json(["a.test:443", "z.test:443"])
        )
        self.assertTrue(str(left["implementation_digest"]).startswith("sha256:"))
        self.assertTrue(str(left["transcript_chain_digest"]).startswith("sha256:"))


if __name__ == "__main__":
    unittest.main()
