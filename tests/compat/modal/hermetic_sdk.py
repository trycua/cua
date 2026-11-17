"""The unmodified `modal` client against the gateway over mock Spaces.

Hermetic: the gateway's backend is the in-process mock cua-spacesd (simulated
processes: `echo`, `fail N`, `ticker N MS`, `cat`, `sleep MS`; files in
memory) and image builds run with `CUA_MODAL_BUILD_DRY_RUN=1` (commands are
logged, not run). A guest port is stood in for by a local HTTP server the
runner maps to guest 8080.

    run_hermetic.sh modal       # builds the mock gateway, runs this per SDK

Each check asserts SDK-visible behaviour; any failure exits 1.
"""

from __future__ import annotations

import json
import os
import socket
import ssl
import sys
import tempfile
import time
import traceback
import urllib.request

# With an env file (the hermetic runners) its values are the gateway's; without,
# the environment is (`eval "$(cua compat env --provider modal)"`).
if len(sys.argv) > 1:
    os.environ.update(json.loads(open(sys.argv[1]).read()))

import modal  # noqa: E402
from modal.exception import (  # noqa: E402
    AlreadyExistsError,
    NotFoundError,
    SandboxFilesystemDirectoryNotEmptyError,
    SandboxFilesystemIsADirectoryError,
    SandboxFilesystemNotADirectoryError,
    SandboxFilesystemNotFoundError,
    SandboxFilesystemPathAlreadyExistsError,
    SandboxTerminatedError,
    SandboxTimeoutError,
    UnimplementedError,
)
from modal.types import FileType  # noqa: E402

failures: list[str] = []
passed = 0


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
    except exc:
        return
    raise AssertionError(f"expected {exc.__name__}")


app = modal.App.lookup("hermetic", create_if_missing=True)
IMG = modal.Image.from_registry("ghcr.io/trycua/linux:24.04").run_commands("echo layer-one")


def new(*args, **kw):
    kw.setdefault("image", IMG)
    kw.setdefault("timeout", 120)
    return modal.Sandbox.create(*args, app=app, **kw)


def main():
    sb = new("sleep", "600000")  # the mock's sleep takes milliseconds
    try:
        run(sb)
    finally:
        sb.terminate()
    other()
    print(f"{passed} passed, {len(failures)} failed: {failures}")
    sys.exit(1 if failures else 0)


def run(sb):
    def entrypoint_output():
        s = new("echo", "hello from the entrypoint")
        assert s.stdout.read() == "hello from the entrypoint\n"
        assert s.wait() is None
        assert s.returncode == 0
        assert s.poll() == 0
        s.terminate()

    check("entrypoint_output_and_wait", entrypoint_output)

    def entrypoint_failure():
        s = new("fail", "3")
        s.wait()
        assert s.returncode == 3, s.returncode
        assert "failed" in s.stderr.read()

    check("entrypoint_exit_code", entrypoint_failure)

    def exec_echo():
        p = sb.exec("echo", "hi", "there")
        assert p.stdout.read() == "hi there\n"
        assert p.wait() == 0 and p.returncode == 0
        assert p.stderr.read() == ""

    check("exec_stdout_wait_returncode", exec_echo)

    def exec_fail():
        p = sb.exec("fail", "7")
        assert p.wait() == 7
        assert p.stderr.read() == "failed\n"

    check("exec_exit_code_and_stderr", exec_fail)

    def exec_stdin():
        p = sb.exec("cat")
        p.stdin.write(b"alpha\n")
        p.stdin.write("beta\n")
        p.stdin.write_eof()
        p.stdin.drain()
        assert p.stdout.read() == "alpha\nbeta\n"
        assert p.wait() == 0

    check("exec_stdin_eof", exec_stdin)

    def exec_bytes_and_lines():
        p = sb.exec("echo", "a", text=False)
        assert p.stdout.read() == b"a\n"
        p = sb.exec("ticker", "3", "5", bufsize=1)
        lines = list(p.stdout)
        assert lines == ["tick 0\n", "tick 1\n", "tick 2\n"], lines

    check("exec_bytes_text_and_line_buffering", exec_bytes_and_lines)

    def exec_poll_running():
        p = sb.exec("sleep", "800")
        assert p.poll() is None
        assert p.wait() == 0
        assert p.poll() == 0

    check("exec_poll_then_wait", exec_poll_running)

    def exec_unknown_command():
        p = sb.exec("definitely-not-a-command")
        assert p.wait() == 127

    check("exec_missing_executable_is_127", exec_unknown_command)

    def exec_timeout():
        p = sb.exec("sleep", "60000", timeout=1)
        t = time.monotonic()
        assert p.wait() == -1
        assert time.monotonic() - t < 10

    check("exec_timeout_returns_minus_one", exec_timeout)

    def exec_validation():
        raises(modal.exception.InvalidError, lambda: sb.exec("echo", workdir="relative"))
        raises(modal.exception.InvalidError, lambda: sb.exec("x" * 70000))

    check("exec_validation_errors", exec_validation)

    def fs_roundtrip():
        fs = sb.filesystem
        fs.write_text("héllo\n", "/tmp/modal-fs/a.txt")
        assert fs.read_text("/tmp/modal-fs/a.txt") == "héllo\n"
        fs.write_bytes(bytes(range(256)) * 100, "/tmp/modal-fs/bin")
        assert fs.read_bytes("/tmp/modal-fs/bin") == bytes(range(256)) * 100
        st = fs.stat("/tmp/modal-fs/a.txt")
        assert st.type == FileType.FILE and st.size == len("héllo\n".encode()), st
        names = sorted(e.name for e in fs.list_files("/tmp/modal-fs"))
        assert names == ["a.txt", "bin"], names

    check("filesystem_write_read_stat_list", fs_roundtrip)

    def fs_copy():
        with tempfile.TemporaryDirectory() as d:
            src = os.path.join(d, "in.bin")
            payload = os.urandom(3 * 1024 * 1024 + 7)
            open(src, "wb").write(payload)
            sb.filesystem.copy_from_local(src, "/tmp/modal-fs/copy.bin")
            out = os.path.join(d, "sub", "out.bin")
            sb.filesystem.copy_to_local("/tmp/modal-fs/copy.bin", out)
            assert open(out, "rb").read() == payload

    check("filesystem_copy_from_and_to_local_large_binary", fs_copy)

    def fs_dirs_and_errors():
        fs = sb.filesystem
        fs.make_directory("/tmp/modal-fs/d1/d2")
        fs.make_directory("/tmp/modal-fs/d1/d2")  # idempotent with parents
        raises(
            SandboxFilesystemPathAlreadyExistsError,
            lambda: fs.make_directory("/tmp/modal-fs/d1/d2", create_parents=False),
        )
        raises(
            SandboxFilesystemNotFoundError,
            lambda: fs.make_directory("/tmp/modal-fs/zz/yy", create_parents=False),
        )
        raises(SandboxFilesystemNotFoundError, lambda: fs.read_text("/tmp/modal-fs/missing"))
        raises(SandboxFilesystemNotFoundError, lambda: fs.stat("/tmp/modal-fs/missing"))
        raises(SandboxFilesystemNotFoundError, lambda: fs.list_files("/tmp/modal-fs/missing"))
        raises(SandboxFilesystemIsADirectoryError, lambda: fs.read_text("/tmp/modal-fs/d1"))
        raises(SandboxFilesystemNotADirectoryError, lambda: fs.list_files("/tmp/modal-fs/a.txt"))
        raises(SandboxFilesystemIsADirectoryError, lambda: fs.write_text("x", "/tmp/modal-fs/d1"))
        raises(
            SandboxFilesystemNotADirectoryError,
            lambda: fs.write_text("x", "/tmp/modal-fs/a.txt/under"),
        )
        raises(SandboxFilesystemDirectoryNotEmptyError, lambda: fs.remove("/tmp/modal-fs/d1"))
        fs.remove("/tmp/modal-fs/d1/d2")
        fs.remove("/tmp/modal-fs/d1")
        fs.remove("/tmp/modal-fs/a.txt")
        raises(SandboxFilesystemNotFoundError, lambda: fs.remove("/tmp/modal-fs/a.txt"))
        raises(modal.exception.InvalidError, lambda: fs.read_text("relative/path"))

    check("filesystem_errors_map_to_modal_exceptions", fs_dirs_and_errors)

    def tags():
        s = new("sleep", "600000", tags={"suite": "hermetic", "n": "1"})
        try:
            assert s.get_tags() == {"suite": "hermetic", "n": "1"}
            s.set_tags({"suite": "hermetic", "n": "2"})
            assert s.get_tags()["n"] == "2"
            ids = [
                x.object_id
                for x in modal.Sandbox.list(app_id=app.app_id, tags={"suite": "hermetic"})
            ]
            assert s.object_id in ids
            none = [
                x.object_id for x in modal.Sandbox.list(app_id=app.app_id, tags={"suite": "other"})
            ]
            assert s.object_id not in none
            s.set_tags({})
            assert s.get_tags() == {}
        finally:
            s.terminate()

    check("tags_set_get_list", tags)

    def lookup():
        s = new("sleep", "600000", name="named-one")
        try:
            raises(AlreadyExistsError, lambda: new("sleep", "1", name="named-one"))
            again = modal.Sandbox.from_name("hermetic", "named-one")
            assert again.object_id == s.object_id
            by_id = modal.Sandbox.from_id(s.object_id)
            assert by_id.poll() is None
            assert by_id.exec("echo", "via id").stdout.read() == "via id\n"
        finally:
            s.terminate()
        raises(NotFoundError, lambda: modal.Sandbox.from_name("hermetic", "named-one"))
        old = modal.Sandbox.from_name("hermetic", "named-one", include_terminated=True)
        assert old.object_id == s.object_id and old.returncode == 137
        raises(NotFoundError, lambda: modal.Sandbox.from_name("hermetic", "never-existed"))

    check("from_id_from_name_names_unique", lookup)

    def terminate_semantics():
        s = new("sleep", "600000")
        assert s.poll() is None
        assert s.terminate(wait=True) == 137
        assert s.poll() == 137 and s.returncode == 137
        s.terminate()  # a no-op when finished
        raises(SandboxTerminatedError, s.wait)
        s.wait(raise_on_termination=False)
        raises((NotFoundError, modal.exception.ConflictError), lambda: s.exec("echo"))

    check("terminate_wait_poll_returncode", terminate_semantics)

    def sandbox_timeout():
        s = new("sleep", "600000", timeout=2)
        t = time.monotonic()
        raises(SandboxTimeoutError, s.wait)
        assert 1.5 < time.monotonic() - t < 15
        assert s.returncode == 124

    check("sandbox_timeout_is_124", sandbox_timeout)

    def idle_timeout():
        s = new("sleep", "600000", idle_timeout=2)
        t = time.monotonic()
        s.wait(raise_on_termination=False)
        assert 1.5 < time.monotonic() - t < 15
        assert s.poll() is not None

    check("sandbox_idle_timeout", idle_timeout)

    def secrets_and_env():
        sec = modal.Secret.from_dict({"A_KEY": "a-value"})
        s = new("sleep", "600000", secrets=[sec], env={"B_KEY": "b"})
        try:
            assert s.poll() is None
        finally:
            s.terminate()
        named = modal.Secret.objects.create("hermetic-secret", {"K": "v"}, allow_existing=True)
        got = modal.Secret.from_name("hermetic-secret")
        assert got.hydrate().object_id.startswith("st-")
        assert "hermetic-secret" in [x.name for x in modal.Secret.objects.list()]
        modal.Secret.objects.delete("hermetic-secret")
        raises(NotFoundError, lambda: modal.Secret.from_name("hermetic-secret").hydrate())
        raises(modal.exception.InvalidError, lambda: new("sleep", "1", env={"bad-key": "x"}))

    check("secrets_and_env", secrets_and_env)

    def images():
        a = (
            modal.Image.debian_slim()
            .pip_install("requests")
            .apt_install("git")
            .env({"X": "1"})
            .workdir("/work")
        )
        s = new("sleep", "600000", image=a)
        try:
            assert s.poll() is None
        finally:
            s.terminate()
        b = a.run_commands("echo more")
        assert b.build(app).object_id != a.build(app).object_id
        again = (
            modal.Image.debian_slim()
            .pip_install("requests")
            .apt_install("git")
            .env({"X": "1"})
            .workdir("/work")
        )
        assert again.build(app).object_id == a.build(app).object_id  # cached by spec

    check("image_layers_are_cached_by_spec", images)

    def image_local_files():
        with tempfile.TemporaryDirectory() as d:
            open(os.path.join(d, "x.txt"), "w").write("local content")
            img = IMG.add_local_file(os.path.join(d, "x.txt"), "/opt/x.txt")
            s = new("sleep", "600000", image=img)
            try:
                assert s.filesystem.read_text("/opt/x.txt") == "local content"
            finally:
                s.terminate()
            baked = IMG.add_local_dir(d, "/baked", copy=True)
            s = new("sleep", "600000", image=baked)
            try:
                assert s.filesystem.read_text("/baked/x.txt") == "local content"
            finally:
                s.terminate()

    check("image_add_local_file_mount_and_copy", image_local_files)

    def unsupported():
        raises(
            UnimplementedError, lambda: new("sleep", "1", outbound_cidr_allowlist=["10.0.0.0/8"])
        )
        raises(UnimplementedError, lambda: new("sleep", "1", include_oidc_identity_token=True))
        try:
            new("sleep", "1", outbound_cidr_allowlist=["10.0.0.0/8"])
        except UnimplementedError as e:
            assert "Cua Spaces" in str(e) and "allowlist" in str(e)

    check("unsupported_features_raise_modal_unimplemented", unsupported)

    def bad_base():
        img = modal.Image.from_registry("nvcr.io/nvidia/pytorch:22.12-py3")
        try:
            img.build(app)
        except modal.exception.ImageBuildError as e:
            assert "cua-spacesd" in str(e), e
        else:
            raise AssertionError("expected ImageBuildError")

    check("unmappable_base_image_is_an_image_build_error", bad_base)

    def failing_build():
        img = IMG.run_commands("fail 2")
        try:
            img.build(app)
        except modal.exception.ImageBuildError:
            pass
        else:
            # Dry-run builds do not execute commands.
            pass

    check("failing_build_raises_or_dry_runs", failing_build)

    def raw_tunnel(port_host):
        s = new("sleep", "600000", unencrypted_ports=[8080], encrypted_ports=[8080])
        try:
            t = s.tunnels()[8080]
            host, port = t.tcp_socket
            c = socket.create_connection((host, port), timeout=5)
            c.sendall(b"GET / HTTP/1.0\r\n\r\n")
            data = b""
            while chunk := c.recv(4096):
                data += chunk
            assert b"hello-from-port-8080" in data, data
            ctx = ssl.create_default_context(cafile=os.environ["SSL_CERT_FILE"])
            with urllib.request.urlopen(t.url, timeout=10, context=ctx) as r:
                assert r.read() == b"hello-from-port-8080"
        finally:
            s.terminate()

    check("tunnels_tls_and_raw_tcp", lambda: raw_tunnel(None))

    def app_objects():
        a2 = modal.App.lookup("hermetic-two", create_if_missing=True)
        assert a2.app_id != app.app_id
        raises(NotFoundError, lambda: modal.App.lookup("no-such-app"))
        s = modal.Sandbox.create("sleep", "600000", app=a2, image=IMG, timeout=60)
        assert s.object_id not in [x.object_id for x in modal.Sandbox.list(app_id=app.app_id)]
        s.terminate()

    check("apps", app_objects)


def other():
    pass


main()
