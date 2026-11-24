"""share-a-link: ``cua.share`` makes a link to a file and to a port that a
real daemon owns and serves: private by default (only the owner opens it),
public with a TTL, listed, checked, stopped, and refused for bad input.

hermetic: a real ``cua daemon`` that is a relay host; the relay and the
host's cua-spacesd are fixtures/share_relay.py.
"""

from __future__ import annotations

import http.server
import threading

import e2e
import pytest

import cua


@pytest.fixture
def app_port():
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            body = f"app says {self.path}".encode()
            self.send_response(200)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_address[1]
    srv.shutdown()


async def _flow(d: e2e.ShareDaemon, tmp_path, app_port: int) -> None:
    c = d.client()
    report = tmp_path / "report.html"
    report.write_text("<h1>report</h1>")

    # A file, private by default.
    f = await c.share(str(report))
    assert f.access == cua.ShareAccess.PRIVATE and f.kind == "file"
    assert f.url.startswith(f"{d.relay_url}/m/{e2e.SHARE_MACHINE}/share/"), f.url
    assert "localhost" not in f.url and "127.0.0.1:" + str(app_port) not in f.url
    assert d.get(f.url, "owner") == (200, b"<h1>report</h1>")
    assert d.get(f.url)[0] == 401  # nobody signed in
    assert d.get(f.url, "other")[0] == 403  # someone else's account
    check = await c.check_share(f.id)
    assert check.ok and check.status == 200 and check.via == "link", check

    # A port, shared by name, then stopped.
    p = await c.share(f"localhost:{app_port}", name="dash")
    assert d.get(p.url + "x?y=1", "owner") == (200, b"app says /x?y=1")
    assert {s.id for s in await c.shares()} == {f.id, p.id}
    assert await c.stop_share("dash") == [p.id]
    assert d.get(p.url, "owner")[0] == 404

    # A public share needs no sign-in, and a folder lists read-only.
    pub = await c.share(str(tmp_path), access=cua.ShareAccess.PUBLIC, ttl_seconds=60)
    assert "/share/pub/" in pub.url and pub.expires_at_unix is not None
    code, body = d.get(pub.url)
    assert code == 200 and b"report.html" in body
    assert d.get(pub.url + "../../etc/passwd")[0] in (400, 403, 404)

    # The daemon owns shares: another client sees them, a stop ends them.
    again = d.client()
    assert {s.id for s in await again.shares()} == {f.id, pub.id}
    assert set(await again.stop_all_shares()) == {f.id, pub.id}
    assert d.get(pub.url)[0] == 404
    assert await c.shares() == []

    # Bad input.
    with pytest.raises(cua.CuaError.InvalidArgument):
        await c.share(str(tmp_path / "missing"))
    with pytest.raises(cua.CuaError.InvalidArgument):
        await c.share("0")
    with pytest.raises(cua.CuaError.InvalidArgument):
        await c.share(str(report), access=cua.ShareAccess.PUBLIC, ttl_seconds=5)
    with pytest.raises(cua.CuaError.NotFound):
        await c.stop_share("share-nope")

    # A port nothing listens on: the self-check says so.
    dead = await c.share("1")
    bad = await c.check_share(dead.id)
    assert not bad.ok and bad.status == 502, bad
    await c.stop_all_shares()


@pytest.mark.e2e("share-a-link", "hermetic")
def test_share_a_link(tmp_path, app_port):
    with e2e.share_daemon() as d:
        e2e.run_async(_flow(d, tmp_path, app_port), timeout=120)
