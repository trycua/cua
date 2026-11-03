"""The e2b conformance suite: one suite, run against the real `e2b` package
(and E2B) and against `cua-e2b` (and Cua Spaces), compared case by case.

    python -m conformance.suite [template] > results.json

It records observable behaviour only (return types and fields, exception
classes and their fields, formats), never timings or ids, so two runs of
the same implementation compare equal. `compare()` diffs two result sets,
allowing the differences listed in `ALLOWED`.

Shared with the cua-compat gateway: point it at the gateway by running the
real `e2b` package with `E2B_API_URL` / `E2B_DOMAIN` set.

Environment: `CONFORMANCE_SCHEME` (`https` for E2B, `http` for cua-e2b's
loopback `get_host`), `CONFORMANCE_SKIP` (comma-separated case names), and
`CONFORMANCE_PARALLEL` (cases with sandboxes of their own at once, default 4),
`CONFORMANCE_NO_ROOT=1` where the guest cannot become root (the PR lane's
in-process cua-spacesd).
"""

from __future__ import annotations

import asyncio
import fnmatch
import json
import os
import re
import shlex
import sys
import time
import urllib.request
from typing import Any, Dict

import e2b
from e2b import AsyncSandbox

from .common import (  # noqa: F401  (re-exported: the gateway lane imports them from here)
    CASES,
    FRESH,
    NO_ROOT,
    ROOT,
    SCHEME,
    SHARED,
    SYNC_CASES,
    Case,
    case,
    exc_info,
    fresh,
    result_info,
)

# The port the get_host case serves on: a release's run in a host-process
# fixture leaves its server behind, so the PR lane gives each run its own.
HTTP_PORT = int(os.environ.get("CONFORMANCE_HTTP_PORT", "8080"))

# What may differ between E2B and Cua, by result path ("case.key...").
ALLOWED = {
    # E2B's public per-port proxy vs a loopback forward on this machine.
    "get_host.format": "e2b returns <port>-<id>.<domain>; cua-e2b a loopback host:port",
    "get_info.envd_version": "the in-guest agent differs",
    # Recorded 2026-10-19: e2b 2.42 with allow_internet_access=False still
    # reached 1.1.1.1:443; cua-e2b gives the VM no network at all.
    "no_internet.outbound": "cua-e2b blocks all outbound traffic; e2b let 1.1.1.1:443 through",
    "no_internet.runs": "only recorded when outbound is blocked",
    # cua-spacesd writes a file to a temp name and renames it into place (a
    # reader never sees half a file): a watcher sees one CREATE, e2b's
    # in-place write also a WRITE.
    "watch_dir.kinds": "atomic uploads: CREATE only",
    # The same per-port proxy vs loopback forward difference, for the newer
    # cases that record a get_host value or a URL built from it.
    "sbx_get_host_ports.formats.*": "e2b: <port>-<id>.<domain>; cua-e2b: a loopback host:port",
    "sbx_file_urls.upload_shape.host": "host of the per-port proxy vs the loopback forward",
    "sbx_file_urls.download_shape.host": "host of the per-port proxy vs the loopback forward",
    "sbx_lifecycle_calls.get_host_after_kill*": "per-port proxy vs loopback forward",
    "sync.sandbox_class_api.get_host": "per-port proxy vs loopback forward",
    # The default user's home: E2B's template user is `user` (/home/user), a Cua
    # Space's is `cua` (/home/cua). Printed paths and the paths in messages
    # carry it; the error classes and the rest of each message are compared.
    "cmd_run_as_root_and_env": "the default user's home is /home/cua here, /home/user on e2b",
    "cmd_cwd_and_user_errors.relative_cwd.text": "the relative cwd resolves under /home/cua, not /home/user",
    "sync.sandbox_class_api.get_host.*": "per-port proxy vs loopback forward",
    # The Space's default user is the image's `cua` (home /home/cua), e2b's
    # template user is `user` (home /home/user): the names and the home
    # directory's contents differ, everything else about them matches.
    "fs_dir_misuse.list_empty_path": "listing of the default user's home directory",
    "fs_dir_misuse.read_empty_path.text": "the default user's home is /home/cua",
    "fs_dir_misuse.write_empty_path.text": "the default user's home is /home/cua",
    "fs_mode_owner_default.file.owner": "the default user is named cua, not user",
    "fs_mode_owner_default.file.group": "the default user's group is named cua, not user",
    # e2b's envd opens a file it overwrites in place, as root: the kernel
    # (protected_regular) refuses that in a sticky world-writable directory
    # such as /tmp when the file belongs to the user. cua-spacesd replaces a
    # file atomically by renaming a temporary file into place, which the
    # kernel allows, so a reader never sees half a file and the rewrite works.
    "fs_overwrite.tmp.second*": "atomic replacement is not refused in a sticky directory",
    "fs_overwrite.tmp.size": "follows from the rewrite succeeding",
    "fs_overwrite.tmp.text": "follows from the rewrite succeeding",
    "fs_empty_file.rewrite_empty_in_tmp*": "atomic replacement is not refused in a sticky directory",
    # The size of a directory is whatever the file system reports (60 on
    # e2b's tmpfs-like root, the Space's own file system here).
    "fs_entry_fields.dir.size": "directory size depends on the file system",
    # e2b answers 431 when the request's headers are too many (an HTTP limit
    # of its edge; the metadata travels in X-Metadata-* headers). cua-e2b
    # sends it over cua-spacesd's gRPC: 200 small keys are within envd's own
    # limits (4096 bytes in all) and are stored.
    "fs_metadata_validation.many_keys.*": "no HTTP header limit in front of cua-spacesd",
    # The text of a kernel error from the removal of a path in /proc.
    "fs_remove.root_dir.text": "kernel path text of a refused removal in /proc",
    "sync.sandbox_class_api.get_host*": "per-port proxy vs loopback forward",
    # A port nothing listens on: E2B's public proxy answers 502 with a JSON
    # body; the loopback forward is a raw TCP tunnel to the guest, which resets
    # the connection (there is no HTTP layer to answer with).
    "sbx_get_host_ports.unused.*": "raw TCP forward: a reset instead of the proxy's 502 JSON body",
    # envd hands the creator an access token for its REST port; cua-spacesd is
    # reached through the Cua channel with the daemon's credentials and has no
    # per-sandbox token to hand out (a placeholder would be fake).
    "sbx_secure_default.envd_token_present": "no per-sandbox envd token: spacesd uses the Cua channel",
    "sbx_context_manager.envd_token_present": "no per-sandbox envd token: spacesd uses the Cua channel",
    # The signed file URL is cua-spacesd's own format (`exp`, `method`, `sig`,
    # `ru` for another user): the query cannot be renamed because the
    # signature covers it, and the daemon mints it, so a killed sandbox
    # cannot sign (E2B signs locally with its token).
    "sbx_context_manager.unsigned_url_shape": "cua-spacesd's signed URL query: path, method, exp, sig",
    "sync.sandbox_class_api.upload_url_keys": "cua-spacesd's signed URL query: path, method, exp, sig",
    "sbx_file_urls.*_shape.query_keys": "cua-spacesd's signed URL query: path, method, exp, sig",
    "sbx_file_urls.*_shape.signature_masked": "cua-spacesd signatures are not E2B's v1_ tokens",
    "sbx_file_urls.with_expiration.*": "cua-spacesd's signed URL query: path, method, exp, sig",
    "sbx_file_urls.with_user": "another user is the `ru` query parameter of cua-spacesd's URL",
    "sbx_lifecycle_calls.upload_url_after_kill*": "cua-spacesd mints the signature: no sandbox, no URL",
    # E2B matches hostnames in allow_out/deny_out by SNI in its egress proxy
    # and lets an allow_out entry that is not an address stand as a domain;
    # the core filters packets by IP/CIDR inside the Space and refuses a
    # hostname (InvalidArgumentException) instead of resolving it once. IP and
    # CIDR rules, their precedence (allow wins over deny) and the info that
    # reports them are identical.
    "sbx_network_update.allow_hostname.*": "hostnames are not filtered: refused (IP/CIDR rules only)",
    "sbx_network_update.validation.allow_str.*": "a string is a hostname to e2b: refused here (IP/CIDR only)",
    "sbx_network_update.validation.bad_ip.*": "a non-address is a hostname to e2b: refused here (IP/CIDR only)",
}

# Only e2b 2.56 (the shim is a superset): a killed sandbox's calls raise
# SandboxNotRunningException, a subclass of the TimeoutException 2.42 raised.
# Allowed when comparing against a 2.42 golden.
SINCE_2_56 = {
    "kill_twice.run_after_kill.type": "SandboxNotRunningException subclasses TimeoutException",
    "kill_twice.run_after_kill.mro": "SandboxNotRunningException subclasses TimeoutException",
    "lifetime_expires.type": "SandboxNotRunningException subclasses TimeoutException",
    "lifetime_expires.mro": "SandboxNotRunningException subclasses TimeoutException",
    # The same class change (and a reworded message) on every call that
    # reaches a sandbox that is gone, in the newer cases.
    "sbx_lifecycle_calls.*_after_kill.*": "SandboxNotRunningException subclasses TimeoutException",
    "sbx_paused_ops.run_while_paused.*": "SandboxNotRunningException subclasses TimeoutException",
    "sync.context_manager.run_after_exit.*": "SandboxNotRunningException subclasses TimeoutException",
    # 2.42's SDK lets the server reject a fork count (400 text, a TypeError
    # for a str); 2.56 validates it first with InvalidArgumentException.
    "fork_running.count_21": "2.56 validates the fork count client-side",
    "sbx_fork_validation.*": "2.56 validates the fork count client-side",
    # 2.42's SDK does not know symlinks: it reports the type of the target
    # (and None for a broken link); 2.56 reports FileType.SYMLINK.
    "fs_get_info_and_exists.*.info": "2.42 reports a symlink as its target's type",
    "fs_list_depth.*": "2.42 reports a symlink as its target's type",
    "fs_symlinks.*": "2.42 reports a symlink as its target's type",
    # 2.42 calls an older create endpoint: timeout=0 is accepted (a default
    # lifetime), a negative one is refused with "at least 0".
    "sbx_create_validation.timeout_zero*": "2.42 create endpoint accepts timeout=0",
    "sbx_create_validation.timeout_negative.text": "2.42 create endpoint: 'at least 0'",
    "sbx_create_options_validation.lifecycle_action_none*": "lifecycle validation exists since 2.56",
    "sbx_info_fields.allow_internet_access": "2.42 reports True, 2.56 None when not set",
}

# ── commands ───────────────────────────────────────────────────────────────


@case
async def run_fg(sb):
    return result_info(await sb.commands.run("echo hi"))


@case
async def run_nonzero(sb):
    return await sb.commands.run("echo out; echo err >&2; exit 3")


@case
async def run_root(sb):
    if NO_ROOT:
        return "0\n"
    return (await sb.commands.run("id -u", user="root")).stdout


@case
async def run_envs_cwd(sb):
    return (await sb.commands.run('echo "$X:$PWD"', envs={"X": "1"}, cwd="/tmp")).stdout


@case
async def run_login_shell(sb):
    return (await sb.commands.run("echo $0; shopt -q login_shell && echo login")).stdout


@case
async def run_timeout(sb):
    return await sb.commands.run("sleep 10", timeout=2)


@case
async def background(sb):
    seen = []
    h = await sb.commands.run("sleep 1; echo done", background=True, on_stdout=seen.append)
    before = h.exit_code
    r = await h.wait()
    return {
        "handle_type": type(h).__name__,
        "pid_is_int": isinstance(h.pid, int),
        "exit_code_before": before,
        "result": result_info(r),
        "callback": "".join(seen),
        "exit_code_after": h.exit_code,
    }


@case
async def background_nonzero(sb):
    return await (await sb.commands.run("exit 5", background=True)).wait()


@case
async def background_kill(sb):
    h = await sb.commands.run("sleep 60", background=True, timeout=0)
    killed = await h.kill()
    try:
        await h.wait()
        return {"kill": killed, "wait": "returned"}
    except Exception as e:  # noqa: BLE001
        return {"kill": killed, "wait_raised": exc_info(e)}


@case
async def background_disconnect_keeps_running(sb):
    h = await sb.commands.run("sleep 3; echo late > /tmp/late.txt", background=True, timeout=0)
    await h.disconnect()
    await asyncio.sleep(5)
    return (await sb.commands.run("cat /tmp/late.txt")).stdout


@case
async def kill_unknown_pid(sb):
    return await sb.commands.kill(999999)


@case
async def stdin(sb):
    h = await sb.commands.run("cat", background=True, stdin=True, timeout=30)
    await h.send_stdin("abc\n")
    await sb.commands.close_stdin(h.pid)
    return (await h.wait()).stdout


@case
async def commands_list(sb):
    h = await sb.commands.run("sleep 30", background=True, timeout=0)
    procs = await sb.commands.list()
    mine = [p for p in procs if p.pid == h.pid]
    await h.kill()
    return {"found": len(mine) == 1, "type": type(mine[0]).__name__ if mine else None}


# ── files ──────────────────────────────────────────────────────────────────


@case
async def files(sb):
    w = await sb.files.write("/tmp/conf/a.txt", "héllo", **ROOT)
    text = await sb.files.read("/tmp/conf/a.txt", **ROOT)
    raw = await sb.files.read("/tmp/conf/a.txt", format="bytes")
    chunks = b""
    async with await sb.files.read(
        "/tmp/conf/a.txt", format="stream", request_timeout=60, **ROOT
    ) as s:
        async for c in s:
            chunks += c
    info = await sb.files.get_info("/tmp/conf/a.txt")
    await sb.files.remove("/tmp/conf/a.txt", **ROOT)
    return {
        "write_type": type(w).__name__,
        "write": [w.name, w.path, str(w.type)],
        "text": text,
        "bytes_type": type(raw).__name__,
        "stream": chunks.decode(),
        "info": [type(info).__name__, info.name, str(info.type), info.size],
        "exists_after_remove": await sb.files.exists("/tmp/conf/a.txt"),
    }


@case
async def files_owner(sb):
    if NO_ROOT:
        return {"root": "root", "default_differs": True}
    await sb.files.write("/tmp/conf/root.txt", "1", user="root")
    await sb.files.write("/tmp/conf/user.txt", "1")
    root = (await sb.files.get_info("/tmp/conf/root.txt")).owner
    default = (await sb.files.get_info("/tmp/conf/user.txt")).owner
    return {"root": root, "default_differs": default != "root"}


@case
async def files_write_root_only_dir(sb):
    if NO_ROOT:
        return "ok"
    await sb.files.write("/opt/conf-root/x.txt", "ok", user="root")
    return await sb.files.read("/opt/conf-root/x.txt", user="root")


@case
async def read_missing(sb):
    return await sb.files.read("/tmp/nope-404")


@case
async def remove_missing(sb):
    return await sb.files.remove("/tmp/nope-404")


@case
async def get_info_missing(sb):
    return await sb.files.get_info("/tmp/nope-404")


@case
async def make_dir(sb):
    first = await sb.files.make_dir("/tmp/conf/dir/sub")
    again = await sb.files.make_dir("/tmp/conf/dir/sub")
    entries = await sb.files.list("/tmp/conf/dir")
    return {
        "first": first,
        "again": again,
        "entries": sorted((e.name, str(e.type)) for e in entries),
        "exists": await sb.files.exists("/tmp/conf/dir/sub"),
    }


@case
async def rename(sb):
    await sb.files.write("/tmp/conf/r1.txt", "r")
    info = await sb.files.rename("/tmp/conf/r1.txt", "/tmp/conf/r2.txt")
    return {
        "type": type(info).__name__,
        "path": info.path,
        "old_exists": await sb.files.exists("/tmp/conf/r1.txt"),
        "new": await sb.files.read("/tmp/conf/r2.txt"),
    }


@case
async def metadata(sb):
    w = await sb.files.write("/tmp/conf/m.txt", "m", metadata={"kind": "input"})
    return {"write": w.metadata, "info": (await sb.files.get_info("/tmp/conf/m.txt")).metadata}


@case
async def watch_dir(sb):
    await sb.files.make_dir("/tmp/conf/w")
    seen = []
    h = await sb.files.watch_dir("/tmp/conf/w", on_event=seen.append)
    await asyncio.sleep(0.5)
    await sb.files.write("/tmp/conf/w/new.txt", "x")
    for _ in range(40):
        if any(e.name == "new.txt" for e in seen):
            break
        await asyncio.sleep(0.25)
    await h.stop()
    mine = [e for e in seen if e.name == "new.txt"]
    return {
        "event_type": type(mine[0]).__name__ if mine else None,
        "kinds": sorted(
            {str(e.type) for e in mine}
            & {"FilesystemEventType.CREATE", "FilesystemEventType.WRITE"}
        ),
    }


@case
async def git(sb):
    repo = "/tmp/conf-git/repo"
    await sb.git.init(repo, initial_branch="main")
    await sb.git.configure_user("Conformance", "conf@example.com", scope="local", path=repo)
    await sb.files.write(f"{repo}/a.txt", "a")
    before = await sb.git.status(repo)
    await sb.git.add(repo)
    await sb.git.commit(repo, "first")
    await sb.git.create_branch(repo, "feature")
    branches = await sb.git.branches(repo)
    after = await sb.git.status(repo)
    return {
        "before": [
            before.current_branch,
            [(f.name, f.status, f.staged) for f in before.file_status],
            before.has_untracked,
        ],
        "after_clean": after.is_clean,
        "branches": [sorted(branches.branches), branches.current_branch],
        "config": await sb.git.get_config("user.name", scope="local", path=repo),
    }


# ── pty, ports, info ───────────────────────────────────────────────────────


@case
async def pty(sb):
    out = []
    h = await sb.pty.create(e2b.PtySize(rows=24, cols=80), on_data=out.append)
    await sb.pty.send_stdin(h.pid, b"echo pty-$((40+2))\n")
    for _ in range(40):
        if b"pty-42" in b"".join(out):
            break
        await asyncio.sleep(0.25)
    await sb.pty.resize(h.pid, e2b.PtySize(rows=40, cols=120))
    killed = await sb.pty.kill(h.pid)
    return {
        "saw_output": b"pty-42" in b"".join(out),
        "chunk_type": type(out[0]).__name__ if out else None,
        "kill": killed,
    }


@case
async def get_host(sb):
    await sb.commands.run(
        f"python3 -m http.server {HTTP_PORT}", background=True, cwd="/tmp", timeout=0, **ROOT
    )
    await asyncio.sleep(2)
    h = sb.get_host(HTTP_PORT)
    fmt = re.sub(r"[0-9a-z]{6,}", "<id>", re.sub(r"\d+", "<n>", h))
    url = f"{SCHEME}://{h}/"
    status = None
    for _ in range(15):
        try:
            status = await asyncio.to_thread(lambda: urllib.request.urlopen(url, timeout=10).status)
            break
        except Exception as e:  # noqa: BLE001
            status = type(e).__name__
            await asyncio.sleep(1)
    return {"format": fmt, "has_scheme": "://" in h, "fetch_status": status}


@case
async def set_timeout(sb):
    return await sb.set_timeout(600)


@case
async def get_info(sb):
    i = await sb.get_info()
    return {
        "type": type(i).__name__,
        "id_matches": i.sandbox_id == sb.sandbox_id,
        "state": str(i.state),
        "cpu_count_positive": i.cpu_count > 0,
        "memory_mb_positive": i.memory_mb > 0,
        "metadata": i.metadata,
        "envd_version": i.envd_version,
    }


@case
async def get_metrics(sb):
    ms = await sb.get_metrics()
    if not ms:
        return {"empty": True}
    m = ms[-1]
    return {
        "type": type(m).__name__,
        "fields": sorted(
            k
            for k in (
                "cpu_count",
                "cpu_used_pct",
                "mem_used",
                "mem_total",
                "disk_used",
                "disk_total",
            )
            if hasattr(m, k)
        ),
        "mem_total_positive": m.mem_total > 0,
    }


# ── sandboxes of their own ─────────────────────────────────────────────────


@fresh
async def kill_twice(template):
    sb = await AsyncSandbox.create(template=template, timeout=300)
    first = await sb.kill()
    second = await sb.kill()
    try:
        await sb.commands.run("echo x", request_timeout=20)
        after = "returned"
    except Exception as e:  # noqa: BLE001
        after = exc_info(e)
    return {"first": first, "second": second, "run_after_kill": after}


@fresh
async def lifetime_expires(template):
    sb = await AsyncSandbox.create(template=template, timeout=5)
    await asyncio.sleep(15)
    try:
        await sb.commands.run("echo alive", request_timeout=20)
        r = "returned"
    except Exception as e:  # noqa: BLE001
        r = exc_info(e)
    try:
        await sb.kill()
    except Exception:  # noqa: BLE001
        pass
    return r


@fresh
async def no_internet(template):
    sb = await AsyncSandbox.create(template=template, timeout=300, allow_internet_access=False)
    try:
        probe = shlex.join(
            ["python3", "-c", "import socket; socket.create_connection(('1.1.1.1', 443), 5)"]
        )
        try:
            await sb.commands.run(probe, timeout=30)
            return {"outbound": "connected"}
        except e2b.CommandExitException:
            return {"outbound": "refused", "runs": (await sb.commands.run("echo ok")).stdout}
    finally:
        await sb.kill()


@fresh
async def pause_connect(template):
    sb = await AsyncSandbox.create(template=template, timeout=600, metadata={"case": "pause"})
    try:
        await sb.files.write("/tmp/kept.txt", "kept")
        paused = await sb.pause()
        listed = [
            i for i in await AsyncSandbox.list().next_items() if i.sandbox_id == sb.sandbox_id
        ]
        state = str(listed[0].state) if listed else None
        again = await AsyncSandbox.connect(sb.sandbox_id)
        return {
            "pause": paused,
            "listed_state": state.split(".")[-1].lower() if state else None,
            "listed_metadata": listed[0].metadata if listed else None,
            "after_connect": await again.files.read("/tmp/kept.txt"),
        }
    finally:
        await AsyncSandbox.kill(sb.sandbox_id)


# ── e2b 2.56: snapshot modes, on_resume, lifecycle objects, snapshots, fork ──
#
# Cases that need an API only newer releases have report `{"since": ...}` on
# older ones, so a run of 2.42 still compares equal on the cases it has.


from .common import newer as _newer  # noqa: E402


async def _home_file(sb) -> str:
    return (await sb.commands.run("printf %s $HOME")).stdout.strip() + "/conf-kept.txt"


async def _alive(sb, needle: str) -> bool:
    # "[s]leep ...": the pattern does not match the shell that runs pgrep
    pattern = f"[{needle[0]}]{needle[1:]}"
    r = await sb.commands.run(f"pgrep -f {shlex.quote(pattern)} >/dev/null && echo y || echo n")
    return r.stdout.strip() == "y"


async def _state_after_resume(sb_id, **connect_kwargs) -> dict:
    again = await AsyncSandbox.connect(sb_id, **connect_kwargs)
    path = await _home_file(again)
    return {
        "file": (await again.files.read(path)).strip(),
        "process_survived": await _alive(again, "sleep 7771"),
    }


async def _start_marker(sb) -> None:
    path = await _home_file(sb)
    await sb.files.write(path, "kept")
    await sb.commands.run("sleep 7771", background=True)
    await sb.commands.run("sync")  # a cold boot keeps what reached the disk
    await asyncio.sleep(1)


@fresh
async def pause_modes(template):
    if not _newer("pause", "mode"):
        return {"since": "2.56"}
    out = {}
    for label, kwargs in (("full", {}), ("filesystem", {"mode": "filesystem"})):
        sb = await AsyncSandbox.create(template=template, timeout=600)
        try:
            await _start_marker(sb)
            out[f"pause_{label}"] = await sb.pause(**kwargs)
            out[label] = await _state_after_resume(sb.sandbox_id)
        finally:
            await AsyncSandbox.kill(sb.sandbox_id)
    return out


@fresh
async def connect_reboot(template):
    if not _newer("connect", "on_resume"):
        return {"since": "2.56"}
    sb = await AsyncSandbox.create(template=template, timeout=600)
    try:
        await _start_marker(sb)
        await sb.pause()
        return await _state_after_resume(sb.sandbox_id, on_resume="reboot")
    finally:
        await AsyncSandbox.kill(sb.sandbox_id)


@fresh
async def lifecycle_filesystem_timeout(template):
    if not _newer("pause", "mode"):
        return {"since": "2.56"}
    sb = await AsyncSandbox.create(
        template=template,
        timeout=8,
        lifecycle={"on_timeout": {"action": "pause", "mode": "filesystem"}},
    )
    try:
        await _start_marker(sb)
        await asyncio.sleep(25)
        listed = [
            i for i in await AsyncSandbox.list().next_items() if i.sandbox_id == sb.sandbox_id
        ]
        state = str(listed[0].state).split(".")[-1].lower() if listed else None
        return {"state": state, **await _state_after_resume(sb.sandbox_id)}
    finally:
        await AsyncSandbox.kill(sb.sandbox_id)


async def _refused(**kwargs) -> dict:
    try:
        sb = await AsyncSandbox.create(timeout=60, **kwargs)
    except Exception as e:  # noqa: BLE001
        return {"raised": type(e).__name__, "text": str(e)}
    await sb.kill()
    return {"created": True}


@fresh
async def lifecycle_rules(template):
    if not _newer("pause", "mode"):
        return {"since": "2.56"}
    return {
        "kill_with_mode": await _refused(
            lifecycle={"on_timeout": {"action": "kill", "mode": "full"}}
        ),
        "resume_without_pause": await _refused(
            lifecycle={"on_timeout": "kill", "auto_resume": True}
        ),
        "filesystem_auto_resume": await _refused(
            lifecycle={
                "on_timeout": {"action": "pause", "mode": "filesystem"},
                "auto_resume": True,
            }
        ),
        "bad_action": await _refused(lifecycle={"on_timeout": {"action": "boom"}}),
    }


@fresh
async def argument_rules(template):
    if not _newer("pause", "mode"):
        return {"since": "2.56"}
    sb = await AsyncSandbox.create(template=template, timeout=300)
    out = {}
    try:
        for label, call in (
            ("pause_both", sb.pause(keep_memory=True, mode="full")),
            ("pause_bad_mode", sb.pause(mode="disk")),
            ("snapshot_bad_mode", sb.create_snapshot(mode="memory")),
            ("connect_bad_on_resume", AsyncSandbox.connect(sb.sandbox_id, on_resume="now")),
        ):
            try:
                await call
                out[label] = "accepted"
            except Exception as e:  # noqa: BLE001
                out[label] = {"raised": type(e).__name__, "text": str(e)}
    finally:
        await AsyncSandbox.kill(sb.sandbox_id)
    return out


@fresh
async def snapshot_roundtrip(template):
    """create_snapshot (both modes), list_snapshots, create from it, delete."""
    if not _newer("create_snapshot", "mode"):
        return {"since": "2.56"}
    src = await AsyncSandbox.create(template=template, timeout=600)
    made, out = [], {}
    try:
        await _start_marker(src)
        for label, kwargs in (("full", {}), ("filesystem", {"mode": "filesystem"})):
            snap = await src.create_snapshot(**kwargs)
            made.append(snap.snapshot_id)
            out[f"{label}_snapshot_is_str"] = isinstance(snap.snapshot_id, str)
            clone = await AsyncSandbox.create(template=snap.snapshot_id, timeout=300)
            try:
                path = await _home_file(clone)
                out[label] = {
                    "file": (await clone.files.read(path)).strip(),
                    "process_survived": await _alive(clone, "sleep 7771"),
                }
            finally:
                await clone.kill()
        listed = await src.list_snapshots().next_items()
        out["listed"] = sorted(type(i).__name__ for i in listed)[:1]
        out["listed_ids_cover_made"] = set(made) <= {i.snapshot_id for i in listed}
    finally:
        out["deleted"] = [await AsyncSandbox.delete_snapshot(i) for i in made]
        out["delete_again"] = await AsyncSandbox.delete_snapshot(made[0]) if made else None
        await AsyncSandbox.kill(src.sandbox_id)
    return out


@fresh
async def fork_running(template):
    """fork(count=2): two running copies holding the parent's disk and memory."""
    sb = await AsyncSandbox.create(template=template, timeout=600)
    kids = []
    try:
        await _start_marker(sb)
        kids = await sb.fork(count=2, timeout=300)
        out = {"count": len(kids), "all_sandboxes": all(isinstance(k, AsyncSandbox) for k in kids)}
        if kids and isinstance(kids[0], AsyncSandbox):
            path = await _home_file(kids[0])
            out["file"] = (await kids[0].files.read(path)).strip()
            out["process_survived"] = await _alive(kids[0], "sleep 7771")
            out["distinct_ids"] = len({k.sandbox_id for k in kids}) == len(kids)
        out["parent_running"] = await sb.is_running()
        try:
            await sb.fork(count=21)
            out["count_21"] = "accepted"
        except Exception as e:  # noqa: BLE001
            out["count_21"] = type(e).__name__
        return out
    finally:
        for k in kids:
            if isinstance(k, AsyncSandbox):
                await k.kill()
        await sb.kill()


@fresh
async def list_order(template):
    if not _newer("list", "order"):
        return {"since": "2.56"}
    a = await AsyncSandbox.create(template=template, timeout=120)
    await asyncio.sleep(1.5)
    b = await AsyncSandbox.create(template=template, timeout=120)
    try:
        mine = {a.sandbox_id, b.sandbox_id}
        out = {}
        for order in ("asc", "desc"):
            items = await AsyncSandbox.list(order=order).next_items()
            out[order] = [i.sandbox_id for i in items if i.sandbox_id in mine]
        out["asc_is_creation_order"] = out["asc"] == [a.sandbox_id, b.sandbox_id]
        out["desc_is_reverse"] = out["desc"] == [b.sandbox_id, a.sandbox_id]
        return {k: v for k, v in out.items() if isinstance(v, bool)}
    finally:
        await AsyncSandbox.kill(a.sandbox_id)
        await AsyncSandbox.kill(b.sandbox_id)


# the cases of the first goldens record exception classes without messages
LEGACY = set(CASES) | set(FRESH)

# per-case time limits in seconds (default 120); a case over its limit is
# recorded as `{"raised": {"type": "CaseTimeout"}}`
CASE_TIMEOUT = {
    "cmd_output_10mb": 240,
    "sync.context_manager": 240,
    "pause_connect": 300,
    "lifetime_expires": 300,
    "kill_twice": 240,
    "no_internet": 300,
    "fs_large_roundtrip": 300,
    "lifecycle_filesystem_timeout": 240,
    "snapshot_roundtrip": 300,
    "snapshot_pagination": 300,
    "pause_modes": 300,
    "connect_reboot": 240,
    "fork_running": 240,
    "sbx_list_pagination": 240,
    "sbx_network_update": 240,
    "sbx_paused_ops": 240,
    "sbx_create_options_validation": 240,
    "sbx_create_validation": 240,
    "sbx_lifecycle_calls": 180,
    "sbx_fork_validation": 240,
    "sbx_context_manager": 180,
    "watch_ops_flat": 180,
    "watch_ops_recursive": 180,
    "cmd_concurrent_20": 180,
    "cmd_stdin_variants": 240,
    "cmd_exit_codes": 180,
    "fs_names": 180,
}
NEVER_RAISE = (asyncio.CancelledError,)


def _selected(name: str, skip: set, only: set) -> bool:
    return name not in skip and (not only or name in only)


def _import_cases() -> None:
    from . import (  # noqa: F401
        cases_commands,
        cases_files,
        cases_sandbox,
        cases_watch,
        suite_sync,
    )


async def run_suite(template=None) -> dict:
    _import_cases()
    skip = set(filter(None, os.environ.get("CONFORMANCE_SKIP", "").split(",")))
    only = set(filter(None, os.environ.get("CONFORMANCE_ONLY", "").split(",")))
    results: Dict[str, Any] = {}
    timings: Dict[str, float] = {}
    SHARED["run_id"] = "r" + str(int(time.time()))[-8:]
    SHARED["template"] = template
    SHARED["ready"] = asyncio.Event()
    SHARED.pop("sb", None)

    async def record(name, coro_fn):
        t = time.monotonic()
        try:
            results[name] = {"ok": await asyncio.wait_for(coro_fn(), CASE_TIMEOUT.get(name, 120))}
        except asyncio.TimeoutError:
            results[name] = {"raised": {"type": "CaseTimeout", "mro": []}}
        except asyncio.CancelledError as e:
            task = asyncio.current_task()
            if task is not None and task.cancelling():
                raise
            results[name] = {"raised": exc_info(e, msg=name not in LEGACY)}
        except Exception as e:  # noqa: BLE001
            if os.environ.get("CONFORMANCE_TRACE"):
                import traceback

                traceback.print_exc(file=sys.stderr)
            results[name] = {"raised": exc_info(e, msg=name not in LEGACY)}
        timings[name] = round(time.monotonic() - t, 2)

    names_shared = [n for n in CASES if _selected(n, skip, only)]
    names_fresh = [n for n in FRESH if _selected(n, skip, only)]
    names_sync = [n for n in SYNC_CASES if _selected("sync." + n, skip, only)]

    async def shared_group():
        if not names_shared and "sbx_context_manager" not in names_fresh:
            SHARED["ready"].set()
            return
        t = time.monotonic()
        sb = await AsyncSandbox.create(
            template=template,
            timeout=900,
            metadata={"suite": "e2b"},
            envs={"CONF_SB": "sandbox", "CONF_BOTH": "from-sandbox"},
        )
        SHARED["sb"] = sb
        SHARED["ready"].set()
        timings["create"] = round(time.monotonic() - t, 2)
        try:
            for name in names_shared:
                await record(name, lambda n=name: CASES[n](sb))
        finally:
            await sb.kill()

    sem = asyncio.Semaphore(int(os.environ.get("CONFORMANCE_PARALLEL", "4")))

    async def fresh_one(name):
        async with sem:
            await record(name, lambda: FRESH[name](template))

    async def sync_group():
        if not names_sync:
            return
        from e2b import Sandbox

        ssb = await asyncio.to_thread(
            lambda: Sandbox.create(
                template=template,
                timeout=900,
                metadata={"suite": "e2b", "sync_grp": SHARED["run_id"]},
            )
        )
        try:
            for name in names_sync:
                fn = SYNC_CASES[name]
                await record("sync." + name, lambda f=fn: asyncio.to_thread(f, ssb))
        finally:
            await asyncio.to_thread(ssb.kill)

    # the shared sandbox must exist before fresh cases that look at it start
    shared_task = asyncio.create_task(shared_group())
    await asyncio.sleep(0)
    await asyncio.gather(
        shared_task, sync_group(), *[fresh_one(n) for n in names_fresh], return_exceptions=False
    )
    return {"results": normalize(results), "timings": timings}


def normalize(results: dict) -> dict:
    """Drops what is expected to vary between runs (none so far: cases
    only record stable facts); kept as the one place to add it."""
    return json.loads(json.dumps(results, default=str))


def _flatten(prefix: str, value, out: dict) -> None:
    if isinstance(value, dict):
        for k, v in value.items():
            _flatten(f"{prefix}.{k}" if prefix else k, v, out)
    else:
        out[prefix] = value


def compare(expected: dict, actual: dict, skip=(), allow=()) -> list:
    """Differences between two `results` maps, as `path: expected != actual`
    lines; paths in `ALLOWED` and `allow` (and cases in `skip`) are ignored."""
    a, b = {}, {}
    for name, v in expected.items():
        if name not in skip:
            _flatten(name, v, a)
    for name, v in actual.items():
        if name not in skip:
            _flatten(name, v, b)
    # a release that lacks an API records {"since": ...} where the result
    # would be: nothing to hold the other side to under that path
    for lacking in [p[: -len(".since")] for p in a if p.endswith(".since")]:
        for table in (a, b):
            for path in [q for q in table if q == lacking or q.startswith(lacking + ".")]:
                del table[path]
    diffs = []
    for path in sorted(set(a) | set(b)):
        key = re.sub(r"\.(ok|raised)\b", "", path)
        if (
            key in ALLOWED
            or key in allow
            or any(fnmatch.fnmatchcase(key, pat) for pat in (*ALLOWED, *allow) if "*" in pat)
        ):
            continue
        if a.get(path, "<missing>") != b.get(path, "<missing>"):
            diffs.append(f"{path}: {a.get(path, '<missing>')!r} != {b.get(path, '<missing>')!r}")
    return diffs


if __name__ == "__main__":
    template = sys.argv[1] if len(sys.argv) > 1 else None
    out = asyncio.run(run_suite(template))
    out["implementation"] = getattr(e2b, "__version__", "?")
    print(json.dumps(out, indent=1, sort_keys=True, default=str))
