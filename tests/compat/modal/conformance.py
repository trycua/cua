"""The unmodified `modal` client against `cua compat serve --provider modal`
on real Spaces (a Linux guest running as root): SDK-visible behaviour
recorded as observations and compared with the derived golden
(fixtures/modal-<version>-derived.json).

    conformance.py > run.json
    compare.py fixtures/modal-1.6.2-derived.json run.json

Each case returns a dict of observations (facts the client can see: output,
exit codes, exception classes, ordering); volatile values (ids, timings) are
reduced to booleans before they are recorded. Cases are tagged with the lanes
they run in: every case runs on real Spaces; `hermetic` ones also run over
mock Spaces (see hermetic_sdk.py for those).
"""

from __future__ import annotations

import hashlib
import json
import os
import socket
import ssl
import sys
import tempfile
import threading
import time
import traceback
import urllib.request

import modal
from modal.exception import (
    AlreadyExistsError,
    NotFoundError,
    SandboxFilesystemNotFoundError,
    SandboxTerminatedError,
    SandboxTimeoutError,
)
from modal.types import FileType, FileWatchEventType

RESULTS: dict[str, dict] = {}
LANE = os.environ.get("CONFORMANCE_LANE", "space")
ONLY = set(filter(None, os.environ.get("ONLY", "").split(",")))

app = modal.App.lookup("conformance", create_if_missing=True)
BASE = os.environ.get("CONFORMANCE_IMAGE", "ghcr.io/trycua/linux:24.04")
IMG = modal.Image.from_registry(BASE)
CREATED: list = []


def new(*args, **kw):
    kw.setdefault("image", IMG)
    kw.setdefault("timeout", 600)
    sb = modal.Sandbox.create(*args, app=app, **kw)
    CREATED.append(sb)
    return sb


class Skip(Exception):
    """The Space cannot show this case (the reason is recorded)."""


def case(name):
    def deco(fn):
        if ONLY and name not in ONLY:
            return fn
        t0 = time.monotonic()
        try:
            RESULTS[name] = {"ok": True, "observed": fn()}
            print(f"PASS {name} ({time.monotonic() - t0:.1f}s)", file=sys.stderr, flush=True)
        except Skip as e:
            RESULTS[name] = {"ok": True, "skipped": str(e)}
            print(f"SKIP {name}: {e}", file=sys.stderr, flush=True)
        except BaseException as e:  # noqa: BLE001
            traceback.print_exc(file=sys.stderr)
            RESULTS[name] = {"ok": False, "error": f"{type(e).__name__}: {e}"}
            print(f"FAIL {name}: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
        return fn

    return deco


def exc(fn):
    """The exception class name `fn` raises, or None."""
    try:
        fn()
    except BaseException as e:  # noqa: BLE001
        return type(e).__name__
    return None


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


sb = new("sleep", "100000")


@case("exec_streams_and_exit_code")
def _():
    p = sb.exec("sh", "-c", "echo out; echo err >&2; exit 3")
    return {"stdout": p.stdout.read(), "stderr": p.stderr.read(), "returncode": p.wait()}


@case("exec_default_user_cwd_env")
def _():
    p = sb.exec(
        "sh",
        "-c",
        "id -u; pwd; echo $HOME; echo $MODAL_SANDBOX_ID | cut -c1-3; echo ${MODAL_TASK_ID:+task}",
    )
    return {"stdout": p.stdout.read(), "returncode": p.wait()}


@case("sandbox_env_secrets_and_exec_env")
def _():
    s = new("sleep", "100000", env={"A": "1"}, secrets=[modal.Secret.from_dict({"B": "2"})])
    a = s.exec("sh", "-c", "echo $A $B").stdout.read()
    b = s.exec("sh", "-c", "echo $A $B $C", env={"A": "x", "C": "3"}).stdout.read()
    s.terminate()
    return {"default": a, "override": b}


@case("workdir")
def _():
    s = new("sleep", "100000", workdir="/tmp/wd")
    a = s.exec("pwd").stdout.read()
    b = s.exec("pwd", workdir="/etc").stdout.read()
    s.terminate()
    return {"sandbox_workdir": a, "exec_workdir": b}


@case("stdin_binary_roundtrip")
def _():
    data = os.urandom(2 * 1024 * 1024)
    p = sb.exec("sha256sum", text=False)
    p.stdin.write(data)
    p.stdin.write_eof()
    p.stdin.drain()
    out = p.stdout.read().decode().split()[0]
    return {"sha_matches": out == sha(data), "returncode": p.wait()}


@case("stdout_binary_large")
def _():
    p = sb.exec(
        "sh",
        "-c",
        "head -c 3000000 /dev/urandom > /tmp/r.bin; sha256sum /tmp/r.bin | cut -d' ' -f1 >&2; cat /tmp/r.bin",
        text=False,
    )
    data = p.stdout.read()
    want = p.stderr.read().decode().strip()
    return {"length": len(data), "sha_matches": sha(data) == want}


@case("invalid_utf8_in_text_mode")
def _():
    p = sb.exec("sh", "-c", "printf '\\377\\n'")
    return {"exception": exc(lambda: p.stdout.read())}


@case("killed_by_signal_is_128_plus")
def _():
    p = sb.exec("sh", "-c", "kill -9 $$")
    return {"returncode": p.wait()}


@case("exec_timeout_kills_the_process")
def _():
    p = sb.exec("sleep", "31", timeout=2)
    t = time.monotonic()
    rc = p.wait()
    took = time.monotonic() - t
    time.sleep(6)
    alive = sb.exec(
        "sh", "-c", "pgrep -x sleep -a | grep -q ' 31$' && echo alive || echo gone"
    ).stdout.read()
    return {"returncode": rc, "within_5s": took < 5, "process": alive}


@case("pty_exec")
def _():
    p = sb.exec("sh", "-c", "tty; echo hi", pty=True)
    out = p.stdout.read()
    return {
        "has_pts": "/dev/pts/" in out,
        "has_hi": "hi" in out,
        "stderr": p.stderr.read(),
        "returncode": p.wait(),
    }


@case("parallel_execs")
def _():
    t = time.monotonic()
    ps = [sb.exec("sh", "-c", "sleep 0.5; echo $$") for _ in range(6)]
    outs = [p.stdout.read() for p in ps]
    return {"distinct": len(set(outs)) == 6, "parallel": time.monotonic() - t < 3.0}


@case("filesystem_stat_symlink_list")
def _():
    sb.exec(
        "sh",
        "-c",
        "mkdir -p /tmp/fs && install -m 640 /dev/null /tmp/fs/f && echo -n abc > /tmp/fs/f && ln -sf /tmp/fs/f /tmp/fs/l && mkdir -p /tmp/fs/d",
    ).wait()
    f = sb.filesystem.stat("/tmp/fs/f")
    lnk = sb.filesystem.stat("/tmp/fs/l")
    d = sb.filesystem.stat("/tmp/fs/d")
    names = sorted(e.name for e in sb.filesystem.list_files("/tmp/fs"))
    return {
        "file": [f.type.value, f.size, f.permissions, f.owner, f.group, f.mode],
        "link": [lnk.type.value, lnk.symlink_target],
        "dir": [d.type.value, d.permissions],
        "list": names,
    }


@case("filesystem_large_file")
def _():
    data = os.urandom(25 * 1024 * 1024)
    sb.filesystem.write_bytes(data, "/tmp/fs/big.bin")
    back = sb.filesystem.read_bytes("/tmp/fs/big.bin")
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "x", "big.bin")
        sb.filesystem.copy_to_local("/tmp/fs/big.bin", out)
        same = open(out, "rb").read() == data
    return {"roundtrip": back == data, "copy_to_local": same}


@case("filesystem_watch")
def _():
    sb.exec("mkdir", "-p", "/tmp/w").wait()
    events: list = []

    def watch():
        for ev in sb.filesystem.watch("/tmp/w", timeout=15):
            events.append((ev.type.value, ev.paths))

    th = threading.Thread(target=watch)
    th.start()
    time.sleep(2)
    sb.exec(
        "sh",
        "-c",
        "echo x > /tmp/w/new.txt; sleep 0.5; echo y >> /tmp/w/new.txt; sleep 0.5; rm /tmp/w/new.txt",
    ).wait()
    th.join(timeout=30)
    kinds = sorted({k for k, p in events if p == ["/tmp/w/new.txt"]})
    return {"kinds": kinds}


@case("tunnels_encrypted_and_raw")
def _():
    s = new(
        "python3",
        "-m",
        "http.server",
        "8080",
        "--directory",
        "/etc",
        encrypted_ports=[8080],
        unencrypted_ports=[8080],
        readiness_probe=modal.Probe.with_tcp(8080),
    )
    s.wait_until_ready()
    t = s.tunnels()[8080]
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
    body = urllib.request.urlopen(t.url, timeout=20, context=ctx).read().decode()
    host, port = t.tcp_socket
    c = socket.create_connection((host, port), timeout=10)
    c.sendall(b"GET / HTTP/1.0\r\n\r\n")
    raw = b""
    while chunk := c.recv(65536):
        raw += chunk
    s.terminate()
    return {"tls": "Directory listing" in body, "raw": b"Directory listing" in raw}


@case("snapshot_filesystem_boots_fresh")
def _():
    s = new("sleep", "100000")
    s.exec("sh", "-c", "echo v1 > /root/snap.txt; sync").wait()
    img = s.snapshot_filesystem()
    s2 = new("sleep", "100000", image=img)
    content = s2.filesystem.read_text("/root/snap.txt")
    s.terminate()
    s2.terminate()
    return {"content": content}


@case("images_python_and_pip")
def _():
    img = (
        modal.Image.debian_slim(python_version="3.11")
        .pip_install("six==1.16.0")
        .env({"IMG_ENV": "set"})
        .workdir("/work")
    )
    s = new("sleep", "100000", image=img)
    out = s.exec(
        "python",
        "-c",
        "import six, sys, os; print(six.__version__, sys.version_info[:2], os.environ['IMG_ENV'], os.getcwd())",
    ).stdout.read()
    s.terminate()
    return {"out": out}


@case("image_console_scripts_are_on_path")
def _():
    # pip's console scripts land next to the interpreter, on PATH (a python:* image's /usr/local/bin).
    img = modal.Image.debian_slim(python_version="3.12").uv_pip_install("pyflakes")
    s = new("sleep", "100000", image=img)
    out = s.exec("sh", "-c", "command -v pyflakes && pyflakes --version").stdout.read()
    s.terminate()
    return {"on_path": "/usr/local/bin/pyflakes" in out}


@case("volume_mount_roundtrip")
def _():
    vol = modal.Volume.from_name("conformance-vol", create_if_missing=True)
    with vol.batch_upload(force=True) as b:
        b.put_file(__file__, "/up/self.py")
    s = new("sleep", "100000", volumes={"/data": vol})
    listing = s.exec("ls", "/data/up").stdout.read()
    s.exec("sh", "-c", "echo from-sandbox > /data/up/out.txt").wait()
    s.terminate()
    names = sorted(e.path for e in vol.listdir("up"))
    got = b"".join(vol.read_file("up/out.txt")).decode()
    vol.remove_file("up", recursive=True)
    modal.Volume.objects.delete("conformance-vol")
    return {"mounted_listing": listing, "listdir": names, "read": got}


@case("dict_and_queue")
def _():
    d = modal.Dict.from_name("conformance-dict", create_if_missing=True)
    d["a"] = 1
    d.put("b", {"x": [1, 2]})
    q = modal.Queue.from_name("conformance-queue", create_if_missing=True)
    q.put_many([1, 2, 3])
    r = {
        "dict": [d["a"], d["b"], d.len(), d.get("zz", "none")],
        "queue": [q.get(), q.get_many(5), q.len()],
    }
    modal.Dict.objects.delete("conformance-dict")
    modal.Queue.objects.delete("conformance-queue")
    return r


@case("entrypoint_stdout_streams_in_order")
def _():
    s = new("sh", "-c", "for i in 1 2 3; do echo $i; sleep 0.5; done")
    t = time.monotonic()
    lines = []
    first_at = None
    for line in s.stdout:
        if first_at is None:
            first_at = time.monotonic() - t
        lines.append(line)
    s.wait()
    return {
        "lines": lines,
        "streamed": first_at is not None and first_at < 1.4,
        "returncode": s.returncode,
    }


@case("sandbox_timeout_is_124_and_space_ends")
def _():
    s = new("sleep", "100000", timeout=4)
    cls = exc(s.wait)
    return {"exception": cls, "returncode": s.returncode}


@case("exec_after_terminate")
def _():
    s = new("sleep", "100000")
    s.terminate()
    return {"exec": exc(lambda: s.exec("echo", "x")), "poll": s.poll()}


@case("snapshot_directory_and_mount_image")
def _():
    s = new("sleep", "100000")
    s.exec("sh", "-c", "mkdir -p /work/data && echo snap > /work/data/a.txt").wait()
    img = s.snapshot_directory("/work/data")
    s2 = new("sleep", "100000")
    s2.mount_image("/mnt/snap", img)
    mounted = s2.exec("cat", "/mnt/snap/a.txt").stdout.read()
    s2.unmount_image("/mnt/snap")
    after = s2.exec("sh", "-c", "ls /mnt/snap 2>/dev/null | wc -l").stdout.read().strip()
    s.terminate()
    s2.terminate()
    return {"mounted": mounted, "after_unmount": after}


@case("memory_snapshot_keeps_processes")
def _():
    s = new("sleep", "100000", _experimental_enable_snapshot=True)
    s.exec("sh", "-c", "echo mem > /tmp/mem.txt; (sleep 888 &) ; sync").wait()
    snap = s._experimental_snapshot()
    s2 = modal.Sandbox._experimental_from_snapshot(snap)
    content = s2.exec("cat", "/tmp/mem.txt").stdout.read()
    procs = s2.exec(
        "sh", "-c", "pgrep -f 'sleep 888' >/dev/null && echo carried || echo fresh"
    ).stdout.read()
    s.terminate()
    s2.terminate()
    return {"content": content, "processes": procs}


@case("volume_read_only_and_sub_path")
def _():
    vol = modal.Volume.from_name("conformance-vol2", create_if_missing=True)
    vol_b = modal.Volume.from_name("conformance-vol3", create_if_missing=True)
    with vol.batch_upload(force=True) as b:
        b.put_file(__file__, "/sub/self.py")
        b.put_file(__file__, "/other.py")
    with vol_b.batch_upload(force=True) as b:
        b.put_file(__file__, "/ro.py")
    s = new(
        "sleep",
        "100000",
        volumes={"/ro": vol_b.read_only(), "/sub": vol.with_mount_options(sub_path="sub")},
    )
    sub = s.exec("ls", "/sub").stdout.read()
    write = s.exec("sh", "-c", "echo x > /ro/new.txt; echo rc=$?").stdout.read()
    s.exec("sh", "-c", "echo from-sandbox > /sub/out.txt").wait()
    s.terminate()  # commits what the Sandbox wrote
    committed = b"".join(vol.read_file("sub/out.txt")).decode()
    vol.remove_file("sub", recursive=True)
    vol.remove_file("other.py")
    modal.Volume.objects.delete("conformance-vol2")
    modal.Volume.objects.delete("conformance-vol3")
    return {"sub": sub, "read_only_write": "rc=0" not in write, "committed_at_exit": committed}


@case("cloud_bucket_mount_public_bucket")
def _():
    # AWS Open Data: a public bucket read without credentials.
    mount = modal.CloudBucketMount("noaa-ghcn-pds", read_only=True)
    try:
        s = new("sleep", "100000", volumes={"/bucket": mount})
    except modal.exception.ConflictError as e:
        if "fuse" in str(e).lower() or "bucket mount needs" in str(e):
            raise Skip(
                "this Space has no /dev/fuse (a container Space); bucket mounts need a VM Space"
            )
        raise
    listing = s.exec("sh", "-c", "ls /bucket | head -3").stdout.read()
    s.terminate()
    return {"listed": bool(listing.strip())}


@case("readiness_probe_and_connect_token")
def _():
    s = new(
        "python3",
        "-m",
        "http.server",
        "8080",
        "--directory",
        "/etc",
        readiness_probe=modal.Probe.with_tcp(8080),
    )
    s.wait_until_ready(timeout=60)
    t = s.create_connect_token(user_metadata={"u": 1}, port=8080)
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
    req = urllib.request.Request(t.url, headers={"Authorization": f"Bearer {t.token}"})
    body = urllib.request.urlopen(req, timeout=20, context=ctx).read().decode()
    try:
        urllib.request.urlopen(t.url, timeout=20, context=ctx)
        unauth = "reachable"
    except Exception as e:  # noqa: BLE001
        unauth = getattr(e, "code", type(e).__name__)
    s.terminate()
    return {"with_token": "Directory listing" in body, "without_token": unauth}


@case("image_run_function")
def _():
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import fn_app

    img = modal.Image.debian_slim(python_version="3.12").run_function(
        fn_app.write_marker, args=("/opt/built.txt", "built"), kwargs={"suffix": "!"}
    )
    with fn_app.app.run():
        s = new("sleep", "100000", image=img)
        content = s.exec("cat", "/opt/built.txt").stdout.read()
        s.terminate()
    return {"content": content}


@case("idle_timeout")
def _():
    s = new("sleep", "100000", idle_timeout=3)
    time.sleep(8)
    return {"poll_is_set": s.poll() is not None}


@case("function_remote_and_map_in_a_space")
def _():
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import fn_app

    with fn_app.app.run():
        out = {
            "remote": fn_app.square.remote(7),
            "map": list(fn_app.square.map([1, 2, 3])),
            "secret": fn_app.secret_env.remote(),
            "gen": list(fn_app.gen.remote_gen(3)),
        }
        vol = fn_app.writes_volume.remote("from-function")
        out["volume"] = vol
    return out


def main():
    for s in CREATED:
        try:
            s.terminate()
        except Exception:  # noqa: BLE001
            pass
    print(
        json.dumps(
            {"lane": LANE, "modal": modal.__version__, "cases": RESULTS}, indent=1, sort_keys=True
        )
    )
    sys.exit(0 if all(r["ok"] for r in RESULTS.values()) else 1)


main()
