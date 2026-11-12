"""Preview links and signed preview URLs (a guest server on port 8080 is
stood in for by the hermetic harness), and sandbox metrics.

Derived from daytona 0.225.0 `_sync/sandbox.py` get_preview_link /
create_signed_preview_url / expire_signed_preview_url."""

import os
import ssl
import time
import urllib.error
import urllib.request

from harness import case


def fetch(url, headers=None):
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=15, context=ctx) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


async def serve(c, sb):
    """On a real guest (COMPAT_REAL_GUEST=1) start the server the hermetic
    harness otherwise stands in for."""
    if not os.environ.get("COMPAT_REAL_GUEST"):
        return
    cmd = (
        "mkdir -p /tmp/www && printf hello-from-port-8080 >/tmp/www/index.html && "
        "(setsid nohup python3 -m http.server 8080 --directory /tmp/www >/dev/null 2>&1 &) ; "
        "for i in $(seq 50); do python3 -c 'import urllib.request;urllib.request.urlopen(\"http://127.0.0.1:8080/\")' 2>/dev/null && exit 0; sleep 0.2; done; exit 1"
    )
    r = await c.w(sb.process.exec(cmd))
    assert r.exit_code == 0, r.result


@case(
    "preview.private_link_needs_its_token",
    needs="mock",
    ref="_sync/sandbox.py get_preview_link ('401 Unauthorized' without credentials)",
)
async def private(c):
    sb = await c.new(auto_stop_interval=0)
    await serve(c, sb)
    link = await c.w(sb.get_preview_link(8080))
    assert link.sandbox_id == sb.id and link.token and link.url.startswith("https://8080-"), link
    ok = fetch(link.url, {"X-Daytona-Preview-Token": link.token})
    assert ok == (200, "hello-from-port-8080"), ok
    assert fetch(link.url)[0] == 401
    assert fetch(link.url, {"X-Daytona-Preview-Token": "wrong"})[0] == 401
    key = os.environ["DAYTONA_API_KEY"]
    assert fetch(link.url, {"Authorization": f"Bearer {key}"}) == (200, "hello-from-port-8080")
    return {"withToken": 200, "without": 401, "wrong": 401, "withApiKey": 200}


@case(
    "preview.public_link_needs_nothing",
    needs="mock",
    ref="_sync/sandbox.py get_preview_link ('Public sandboxes need no credentials')",
)
async def public(c):
    sb = await c.new(public=True, auto_stop_interval=0)
    await serve(c, sb)
    link = await c.w(sb.get_preview_link(8080))
    assert fetch(link.url) == (200, "hello-from-port-8080")
    await c.w(sb.set_public(False))
    assert fetch(link.url)[0] == 401
    return {"public": 200, "afterPrivate": 401}


@case(
    "preview.signed_url",
    needs="mock",
    ref="_sync/sandbox.py create_signed_preview_url/expire_signed_preview_url",
)
async def signed(c):
    sb = await c.new(auto_stop_interval=0)
    await serve(c, sb)
    s = await c.w(sb.create_signed_preview_url(8080, expires_in_seconds=60))
    assert s.port == 8080 and s.sandbox_id == sb.id and s.token in s.url, s
    assert fetch(s.url) == (200, "hello-from-port-8080")
    await c.w(sb.expire_signed_preview_url(8080, s.token))
    assert fetch(s.url)[0] in (401, 403, 404), "an expired URL stops working"
    short = await c.w(sb.create_signed_preview_url(8080, expires_in_seconds=1))
    assert fetch(short.url)[0] == 200
    time.sleep(2.2) if not c.is_async else await __import__("asyncio").sleep(2.2)
    assert fetch(short.url)[0] in (401, 403, 404)
    default = await c.w(sb.create_signed_preview_url(8080))
    assert fetch(default.url)[0] == 200, "valid for 60 s by default"
    return {"works": 200, "afterExpire": "refused", "afterTimeout": "refused", "defaultValid": True}


@case("preview.port_not_open", needs="mock", ref="_sync/sandbox.py get_preview_link")
async def closed(c):
    sb = await c.new(auto_stop_interval=0)
    link = await c.w(sb.get_preview_link(9999))
    st, body = fetch(link.url, {"X-Daytona-Preview-Token": link.token})
    assert st == 502, (st, body)
    return {"closedPort": 502}


@case("sandbox.metrics", needs="mock", ref="_sync/sandbox.py get_metrics/get_metrics_latest")
async def metrics(c):
    sb = await c.main()
    m = await c.w(sb.get_metrics_latest())
    assert m.cpu_count >= 0 and m.mem_total > 0 and m.disk_total >= 0, m
    series = await c.w(sb.get_metrics())
    assert len(series) >= 1
    return {"hasCpu": True, "hasMemory": True}


@case(
    "sandbox.home_and_work_dirs",
    needs="mock",
    ref="_sync/sandbox.py get_user_home_dir/get_work_dir/get_user_root_dir",
)
async def dirs(c):
    sb = await c.main()
    home = await c.w(sb.get_user_home_dir())
    work = await c.w(sb.get_work_dir())
    assert home.startswith("/") and work.startswith("/"), (home, work)
    assert await c.w(sb.get_user_root_dir()) == home
    return {"absolute": True}
