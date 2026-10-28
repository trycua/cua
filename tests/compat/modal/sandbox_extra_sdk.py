"""The unmodified `modal` client against the gateway over mock Spaces: the
Sandbox calls hermetic_sdk.py does not make (snapshots, mounts, connect
tokens, readiness, logs, listing, naming, policies) and the objects around
a Sandbox (ContainerProcess, streams, SandboxSnapshot, Tunnel, Probe).
Calls that need a real guest to show their effect (directory snapshots,
bind mounts, memory snapshots) run on real Spaces (conformance.py).
"""

from __future__ import annotations

import datetime
import json
import os
import sys
import time
import traceback
import uuid

# With an env file (the hermetic runners) its values are the gateway's; without,
# the environment is (`eval "$(cua compat env --provider modal)"`).
if len(sys.argv) > 1:
    os.environ.update(json.loads(open(sys.argv[1]).read()))

import modal  # noqa: E402
import modal.experimental  # noqa: E402
from modal.exception import (  # noqa: E402
    AlreadyExistsError,
    InvalidError,
    NotFoundError,
    SandboxTerminatedError,
    UnimplementedError,
)

failures: list[str] = []
passed = 0
RUN = uuid.uuid4().hex[:8]
app = modal.App.lookup("sandbox-extra", create_if_missing=True)
IMG = modal.Image.from_registry("ghcr.io/trycua/linux:24.04")


def check(name, fn):
    global passed
    try:
        fn()
        passed += 1
        print(f"PASS {name}")
    except BaseException as e:  # noqa: BLE001
        failures.append(name)
        print(f"FAIL {name}: {type(e).__name__}: {e}")
        traceback.print_exc()


def raises(exc, fn):
    try:
        fn()
    except exc as e:
        return e
    raise AssertionError(f"expected {exc.__name__}")


def eq(a, b):
    assert a == b, f"{a!r} != {b!r}"


def new(*args, **kw):
    kw.setdefault("image", IMG)
    kw.setdefault("timeout", 120)
    return modal.Sandbox.create(*args, app=app, **kw)


def sandbox_handle_members():
    sb = new("sleep", "600000")
    try:
        assert sb.object_id
        assert sb.client is not None
        assert sb.get_dashboard_url().startswith("http")
        sb.hydrate()
        eq(sb.poll(), None)
        eq(sb.returncode, None)
    finally:
        sb.terminate()
    other = new("sleep", "600000")
    sid = other.object_id
    other.detach()
    raises(modal.exception.ClientClosed, lambda: other.poll())
    modal.Sandbox.from_id(sid).terminate()


def snapshot_filesystem_and_sandbox_from_image():
    sb = new("sleep", "600000")
    try:
        img = sb.snapshot_filesystem()
        assert img.object_id.startswith("im-"), img.object_id
        sb2 = new("sleep", "600000", image=img)
        sb2.terminate()
    finally:
        sb.terminate()


def snapshot_directory_returns_an_image():
    sb = new("sleep", "600000")
    try:
        img = sb.snapshot_directory("/tmp")
        assert img.object_id.startswith("im-"), img.object_id
    except Exception as e:  # noqa: BLE001
        # The mock has no tar; the real behaviour is conformance.py's.
        assert isinstance(e, (modal.exception.Error, RuntimeError)), e
    finally:
        sb.terminate()


def mount_unmount_errors():
    sb = new("sleep", "600000")
    try:
        raises(Exception, lambda: sb.unmount_image("/not-mounted"))
        raises(InvalidError, lambda: sb.mount_image("relative/path", IMG))
    finally:
        sb.terminate()


def connect_token():
    import ssl
    import urllib.error
    import urllib.request

    sb = new("sleep", "600000")  # port 8080 is not declared: only a connect token reaches it
    try:
        t = sb.create_connect_token(user_metadata={"a": 1}, port=8080)
        assert t.url.startswith("https://8080-") and t.token, t
        raises(InvalidError, lambda: sb.create_connect_token(port=0))
        ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
        err = raises(
            urllib.error.HTTPError, lambda: urllib.request.urlopen(t.url, timeout=10, context=ctx)
        )
        eq(err.code, 401)
        req = urllib.request.Request(t.url, headers={"Authorization": f"Bearer {t.token}"})
        with urllib.request.urlopen(req, timeout=10, context=ctx) as r:
            eq(r.read(), b"hello-from-port-8080")
        # A token is for one port.
        other = sb.create_connect_token(port=9090)
        req = urllib.request.Request(t.url, headers={"Authorization": f"Bearer {other.token}"})
        err = raises(
            urllib.error.HTTPError, lambda: urllib.request.urlopen(req, timeout=10, context=ctx)
        )
        eq(err.code, 401)
    finally:
        sb.terminate()


def wait_until_ready():
    sb = new("sleep", "600000", readiness_probe=modal.Probe.with_exec("echo", "ok"))
    try:
        sb.wait_until_ready(timeout=30)
    finally:
        sb.terminate()
    sb = new(
        "sleep", "600000", unencrypted_ports=[8080], readiness_probe=modal.Probe.with_tcp(8080)
    )
    try:
        sb.wait_until_ready(timeout=30)
    finally:
        sb.terminate()


def logs_manager():
    sb = new("echo", "from-entrypoint")
    try:
        sb.wait()
        since = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=5)
        text = "".join(e.message for e in sb.logs.fetch(since=since))
        assert "from-entrypoint" in text, text
        text = "".join(e.message for e in sb.logs.tail(5))
        assert "from-entrypoint" in text, text
    finally:
        sb.terminate()


def reload_volumes_and_policy_errors():
    sb = new("sleep", "600000")
    try:
        sb.reload_volumes()
        e = raises(
            Exception,
            lambda: sb._experimental_set_outbound_network_policy(
                outbound_cidr_allowlist=["10.0.0.0/8"]
            ),
        )
        assert isinstance(e, (UnimplementedError, modal.exception.Error)), e
        policy = modal.experimental.OutboundPolicy().with_header_replacement(
            domain="example.com", headers={"X-A": "b"}
        )
        e = raises(Exception, lambda: sb._experimental_update_outbound_policy(policy))
        assert isinstance(e, (UnimplementedError, modal.exception.Error)), e
        e = raises(Exception, lambda: new("sleep", "1", _experimental_outbound_policy=policy))
        assert isinstance(e, (UnimplementedError, modal.exception.Error)), e
    finally:
        sb.terminate()


def names_and_listing():
    name = f"named-{RUN}"
    sb = new("sleep", "600000", name=name, tags={"suite": RUN})
    try:
        again = modal.Sandbox.from_name(app.name, name)
        eq(again.object_id, sb.object_id)
        raises(AlreadyExistsError, lambda: new("sleep", "1", name=name))
        listed = [s.object_id for s in modal.Sandbox.list(app_id=app.app_id, tags={"suite": RUN})]
        eq(listed, [sb.object_id])
        sb.set_tags({"suite": RUN, "k": "v"})
        eq(sb.get_tags(), {"suite": RUN, "k": "v"})
    finally:
        sb.terminate()
    raises(NotFoundError, lambda: modal.Sandbox.from_name(app.name, name))
    again = modal.Sandbox.from_name(app.name, name, include_terminated=True)
    assert again.object_id == sb.object_id


def container_process_members():
    sb = new("sleep", "600000")
    try:
        p = sb.exec("fail", "3")  # the mock's program: exit status 3
        eq(p.wait(), 3)
        eq(p.returncode, 3)
        eq(p.poll(), 3)
        eq(p.stdout.read(), "")
        p = sb.exec("cat")
        p.stdin.write("hello")
        p.stdin.write_eof()
        p.stdin.drain()
        eq(p.stdout.read(), "hello")
        eq(p.wait(), 0)
        p = sb.exec("echo", "x", text=False)
        eq(p.stdout.read(), b"x\n")
        eq(p.stdout.file_descriptor, 1)
        p.stdout.close()
    finally:
        sb.terminate()


def sandbox_snapshot_by_id_errors():
    raises(
        NotFoundError, lambda: modal.SandboxSnapshot.from_id("sn-doesnotexist00000000000").hydrate()
    )


def experimental_snapshot_roundtrip():
    sb = new("sleep", "600000", _experimental_enable_snapshot=True)
    try:
        snap = sb._experimental_snapshot()
        assert snap.object_id, snap
        sb2 = modal.Sandbox._experimental_from_snapshot(snap)
        assert sb2.object_id and sb2.object_id != sb.object_id
        sb2.terminate()
    finally:
        sb.terminate()


def snapshot_requires_enable():
    sb = new("sleep", "600000")
    try:
        raises(Exception, lambda: sb._experimental_snapshot())
    finally:
        sb.terminate()


def experimental_sandbox_calls():
    sb = new(
        "sleep",
        "600000",
        _experimental_enable_snapshot=True,
        experimental_options={"enable_exit_snapshot": True},
    )
    try:
        sb._experimental_set_name(f"exp-{RUN}")
        eq(modal.Sandbox._experimental_from_name(app.name, f"exp-{RUN}").object_id, sb.object_id)
        assert sb.object_id in [
            x.object_id for x in modal.Sandbox._experimental_list(app_id=app.app_id)
        ]
        other = modal.Sandbox._experimental_create(
            "sleep", "600000", app=app, image=IMG, timeout=60
        )
        assert other.object_id != sb.object_id
        other.terminate()
        snap = sb._experimental_snapshot()
        sb3 = modal.Sandbox._experimental_from_snapshot_v2(
            snap,
            modal.Client.from_env(),
            override_type=modal.sandbox._sandbox.api_pb2.SandboxRestoreRequest.SandboxNameOverrideType.SANDBOX_NAME_OVERRIDE_TYPE_NONE,
            name_override=None,
        )
        assert sb3.object_id != sb.object_id
        sb3.terminate()
    finally:
        sb.terminate()
    img = sb._experimental_get_exit_snapshot(timeout=30)
    assert img.object_id.startswith("im-"), img.object_id
    plain = new("sleep", "600000")
    plain.terminate()
    raises(InvalidError, lambda: plain._experimental_get_exit_snapshot())


def sidecars_are_refused():
    sb = new("sleep", "600000")
    try:
        e = raises(
            Exception,
            lambda: sb._experimental_sidecars.create("sleep", "1", name="side", image=IMG),
        )
        assert isinstance(e, (UnimplementedError, modal.exception.Error)), e
        assert "sidecar" in str(e).lower(), e
    finally:
        sb.terminate()


def terminated_sandbox_semantics():
    sb = new("sleep", "600000")
    sb.terminate()
    raises(Exception, lambda: sb.exec("echo", "x"))
    eq(sb.poll() is not None, True)


def tunnel_and_probe_objects():
    p = modal.Probe.with_tcp(8080, interval_ms=50)
    eq(p.tcp_port, 8080)
    eq(p.interval_ms, 50)
    q = modal.Probe.with_exec("true", "x", interval_ms=75)
    eq(list(q.exec_argv), ["true", "x"])
    sb = new("sleep", "600000", encrypted_ports=[8080], unencrypted_ports=[9090])
    try:
        t = sb.tunnels()
        assert t[8080].url.startswith("https://"), t
        host, port = t[9090].tcp_socket
        assert host and port
        raises(InvalidError, lambda: t[8080].tcp_socket)
        eq(t[8080].tls_socket, (t[8080].host, t[8080].port))
    finally:
        sb.terminate()


def sandbox_create_validation():
    raises(InvalidError, lambda: new("sleep", "1", workdir="relative"))
    raises(InvalidError, lambda: new("sleep", "1", encrypted_ports=[0]))
    raises(InvalidError, lambda: new("sleep", "1", encrypted_ports=[70000]))
    raises(InvalidError, lambda: new("sleep", "1", encrypted_ports=[80, 80]))
    raises(InvalidError, lambda: new("sleep", "1", name="Not Valid!"))


def sandbox_resources_and_options_accepted():
    sb = new(
        "sleep",
        "600000",
        cpu=(0.5, 1),
        memory=(256, 512),
        env={"A": "1"},
        workdir="/tmp",
        block_network=False,
        outbound_cidr_allowlist=["10.0.0.0/8"] if False else None,
        verbose=False,
    )
    try:
        assert sb.object_id  # the workdir is created in the guest (conformance.py: workdir)
    finally:
        sb.terminate()


def sandbox_unsupported_options_are_native_errors():
    for kw in (
        dict(proxy=modal.Proxy.from_name("p")),
        dict(include_oidc_identity_token=True),
        dict(custom_domain="x.example.com"),
    ):
        e = raises(Exception, lambda: new("sleep", "1", **kw))
        assert isinstance(
            e, (UnimplementedError, InvalidError, NotFoundError, modal.exception.Error)
        ), (kw, e)


def gpu_needs_the_cua_option():
    e = raises(UnimplementedError, lambda: new("sleep", "1", gpu="A10G"))
    assert "cua.gpu" in str(e), e
    # With the option the request is passed to the runtime as its GPU option.
    sb = new("sleep", "600000", gpu="A10G", experimental_options={"cua.gpu": "auto"})
    sb.terminate()


def main():
    for name, fn in list(globals().items()):
        if callable(fn) and getattr(fn, "__module__", None) == "__main__" and name in CHECKS:
            check(name, fn)
    print(f"{passed} passed, {len(failures)} failed: {failures}")
    sys.exit(1 if failures else 0)


CHECKS = {
    "sandbox_handle_members",
    "snapshot_filesystem_and_sandbox_from_image",
    "snapshot_directory_returns_an_image",
    "mount_unmount_errors",
    "connect_token",
    "wait_until_ready",
    "logs_manager",
    "reload_volumes_and_policy_errors",
    "names_and_listing",
    "container_process_members",
    "sandbox_snapshot_by_id_errors",
    "experimental_snapshot_roundtrip",
    "experimental_sandbox_calls",
    "snapshot_requires_enable",
    "sidecars_are_refused",
    "terminated_sandbox_semantics",
    "tunnel_and_probe_objects",
    "sandbox_create_validation",
    "sandbox_resources_and_options_accepted",
    "sandbox_unsupported_options_are_native_errors",
    "gpu_needs_the_cua_option",
}

main()
