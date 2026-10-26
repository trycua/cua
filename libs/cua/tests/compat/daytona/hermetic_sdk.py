"""The unmodified `daytona` SDK against the gateway over mock Spaces.

Hermetic like libs/cua/tests/compat/e2b/hermetic_sdk.py: simulated guest processes
(`echo`, `fail N`, `sleep MS`), in-memory files, a local stand-in for a
guest server on port 8080. Session commands need a real shell, so they are
covered by the Space conformance run (conformance.py), not here.

    libs/cua/tests/compat/daytona/run_hermetic.sh
"""

from __future__ import annotations

import json
import os
import ssl
import sys
import time
import traceback
import urllib.request

os.environ.update(json.loads(open(sys.argv[1]).read()))

from daytona import (  # noqa: E402
    CreateSandboxFromSnapshotParams,
    Daytona,
    DaytonaConfig,
)

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


def fetch(url, headers=None):
    ctx = ssl.create_default_context(cafile=os.environ["SSL_CERT_FILE"])
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=10, context=ctx) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def main():
    daytona = Daytona(DaytonaConfig())
    sb = daytona.create(
        CreateSandboxFromSnapshotParams(labels={"suite": "hermetic"}, auto_stop_interval=0)
    )
    try:
        run(daytona, sb)
    finally:
        try:
            daytona.delete(sb)
        except Exception:  # noqa: BLE001
            pass
    print(f"{passed} passed, {len(failures)} failed: {failures}")
    sys.exit(1 if failures else 0)


def run(daytona: Daytona, sb):
    def created():
        assert str(sb.state).lower().endswith("started"), sb.state
        assert sb.labels.get("suite") == "hermetic", sb.labels
        got = daytona.get(sb.id)
        assert got.id == sb.id
        from daytona import ListSandboxesQuery

        listed = [s.id for s in daytona.list(ListSandboxesQuery(labels={"suite": "hermetic"}))]
        assert sb.id in listed, listed

    check("create_get_list", created)

    def events_connected():
        disp = getattr(daytona, "_event_dispatcher", None)
        for _ in range(50):
            if disp is not None and getattr(disp, "_connected", False):
                return
            time.sleep(0.1)
        raise AssertionError(f"socket.io not connected: {getattr(disp, '_fail_error', None)}")

    check("event_stream_connects", events_connected)

    def exec_ok():
        r = sb.process.exec("echo hi there")
        assert (r.exit_code, r.result) == (0, "hi there\n"), (r.exit_code, r.result)
        r = sb.process.exec("fail 3")
        assert r.exit_code == 3, r.exit_code

    check("process_exec", exec_ok)

    def files():
        sb.fs.upload_file(b"payload", "/tmp/a.txt")
        assert sb.fs.download_file("/tmp/a.txt") == b"payload"
        sb.fs.create_folder("/tmp/dir", "755")
        info = sb.fs.get_file_info("/tmp/a.txt")
        assert (info.name, info.size, info.is_dir) == ("a.txt", 7, False), info
        names = [f.name for f in sb.fs.list_files("/tmp")]
        assert "a.txt" in names, names
        sb.fs.move_files("/tmp/a.txt", "/tmp/b.txt")
        assert sb.fs.download_file("/tmp/b.txt") == b"payload"
        sb.fs.delete_file("/tmp/b.txt")
        try:
            sb.fs.download_file("/tmp/b.txt")
        except Exception:  # noqa: BLE001
            return
        raise AssertionError("deleted file still downloads")

    check("files", files)

    def many_files():
        from daytona import FileUpload

        sb.fs.upload_files(
            [
                FileUpload(source=b"1", destination="/tmp/m/1"),
                FileUpload(source=b"22", destination="/tmp/m/2"),
            ]
        )
        assert sb.fs.download_file("/tmp/m/2") == b"22"

    check("bulk_upload", many_files)

    def signed_file_urls():
        sb.fs.upload_file(b"signed", "/tmp/s.txt")
        url = sb.download_url("/tmp/s.txt", ttl_seconds=60)
        with urllib.request.urlopen(url, timeout=10) as r:
            assert r.read() == b"signed"

    check("signed_download_url", signed_file_urls)

    def preview():
        link = sb.get_preview_link(8080)
        status, body = fetch(link.url, {"x-daytona-preview-token": link.token})
        assert (status, body) == (200, "hello-from-port-8080"), (status, body)
        status, _ = fetch(link.url)
        assert status == 401, status
        signed = sb.create_signed_preview_url(8080, expires_in_seconds=60)
        status, body = fetch(signed.url)
        assert (status, body) == (200, "hello-from-port-8080"), (status, body)

    check("preview_links", preview)

    def labels_and_intervals():
        sb.set_labels({"suite": "hermetic", "x": "1"})
        assert daytona.get(sb.id).labels.get("x") == "1"
        sb.set_autostop_interval(30)
        assert daytona.get(sb.id).auto_stop_interval == 30

    check("labels_and_intervals", labels_and_intervals)

    def stop_start():
        sb.stop()
        assert str(daytona.get(sb.id).state).lower().endswith("stopped")
        sb.start()
        assert str(daytona.get(sb.id).state).lower().endswith("started")
        assert sb.process.exec("echo back").result == "back\n"

    check("stop_start", stop_start)

    def snapshot_fork():
        sb.fs.upload_file(b"world", "/tmp/world.txt")
        name = f"world-{os.getpid()}-{int(time.time() * 1000)}"
        sb.create_snapshot(name)
        child = daytona.create(CreateSandboxFromSnapshotParams(snapshot=name))
        try:
            assert child.fs.download_file("/tmp/world.txt") == b"world"
        finally:
            daytona.delete(child)
        kid = sb.fork()
        try:
            assert kid.fs.download_file("/tmp/world.txt") == b"world"
        finally:
            daytona.delete(kid)

    check("snapshot_and_fork", snapshot_fork)

    def unsupported():
        try:
            daytona.create(CreateSandboxFromSnapshotParams(network_allow_list="10.0.0.0/8"))
        except Exception as e:  # noqa: BLE001
            # The allow list is enforced in the guest; a mock guest cannot, and
            # the create is refused saying so (never silently accepted).
            assert "network policy" in str(e), str(e)
            return
        raise AssertionError("allow-list accepted")

    check("unsupported_field_is_refused", unsupported)

    def delete_other():
        other = daytona.create(CreateSandboxFromSnapshotParams())
        daytona.delete(other)
        try:
            daytona.get(other.id)
        except Exception:  # noqa: BLE001
            return
        raise AssertionError("deleted sandbox still found")

    check("delete", delete_other)


main()
