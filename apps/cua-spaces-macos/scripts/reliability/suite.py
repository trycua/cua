#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
"""Launch reliability suite for Cua Spaces on macOS.

Drives the runtime the SwiftUI app ships: its bundled `cua` (the Cua Spaces
build, `cua-spaces-cli`) and the `cua daemon` it runs, against throwaway
CUA_HOMEs. Every scenario runs N times (default 3) with a bounded time per
step; nothing waits forever.

    suite.py --cua "/Applications/Cua Spaces.app/Contents/MacOS/cua" \
        [--scenarios fresh,linux,volume,conflicts,restart,offline,lowdisk,macos-slim,macos]
        [--runs 3] [--work ~/projects/.cua-work/reliability] [--mit-cua PATH]
        [--other-cua PATH]

Results go to <work>/results/<stamp>.json and a Markdown matrix to stdout
(and <work>/results/<stamp>.md).

Host safety (these are hard rules, not options):
- every cua process gets its own CUA_HOME under <work>/h (never ~/.cua), the
  file credential store (never the Keychain), no browser, no telemetry and
  a throwaway teleport home;
- the suite only stops or kills processes it started (the daemon whose
  discovery file is in one of its own homes and whose executable is the
  cua under test);
- it deletes only Lume VMs its own homes recorded as created
  (`$CUA_HOME/vmm/lume/owned`, kind instance), never a VM that existed when
  the suite started, and only containers named with this run's prefix;
- at most one Space runs at a time, macOS VMs get 6 GiB, and every Space is
  deleted when its scenario run ends.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import shutil
import signal
import statistics
import subprocess
import sys
import time
import traceback
import uuid

HOME = pathlib.Path.home()
REAL_CUA_HOME = HOME / ".cua"


class StepFailed(Exception):
    pass


class Res:
    def __init__(self, rc, out, err, secs, timed_out=False):
        self.rc, self.out, self.err, self.secs, self.timed_out = rc, out, err, secs, timed_out

    def json(self):
        try:
            return json.loads(self.out)
        except Exception:
            return None

    def text(self):
        return (self.out + "\n" + self.err).strip()


def sh(cmd, timeout, env=None, cwd=None):
    """Runs `cmd` (a list) with a hard timeout; the process group is killed
    on timeout, so a hung child can never hold the suite."""
    t = time.monotonic()
    p = subprocess.Popen(cmd, env=env, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        out, err = p.communicate(timeout=timeout)
        return Res(p.returncode, out, err, time.monotonic() - t)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        out, err = p.communicate()
        return Res(-9, out or "", err or "", time.monotonic() - t, timed_out=True)


def pid_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def pid_exe(pid):
    r = sh(["ps", "-o", "comm=", "-p", str(pid)], 5)
    return r.out.strip()


class Home:
    """One throwaway CUA_HOME and the cua that runs in it."""

    def __init__(self, suite, tag, cua=None):
        self.suite = suite
        self.path = suite.work / "h" / tag
        if self.path.exists():
            suite.stop_home(self.path)
            shutil.rmtree(self.path, ignore_errors=True)
        self.path.mkdir(parents=True)
        self.path.chmod(0o700)
        assert self.path.resolve() != REAL_CUA_HOME.resolve()
        self.cua = cua or suite.cua
        self.extra = {}
        suite.homes.append(self.path)

    def env(self, **over):
        e = {k: v for k, v in os.environ.items() if not k.startswith("CUA_")}
        e.update({
            "CUA_HOME": str(self.path),
            "CUA_CREDENTIAL_STORE": "file",
            "CUA_NO_BROWSER": "1",
            "CUA_TELEMETRY": "0",
            "CUA_TELEMETRY_FORBID_NETWORK": "1",
            "DO_NOT_TRACK": "1",
            "CUA_SPACES_TELEPORT_HOME": str(self.path / "teleport"),
            "CUA_DAEMON_NO_RELAY": "1",
        })
        e.update(self.extra)
        for k, v in over.items():
            if v is None:
                e.pop(k, None)
            else:
                e[k] = v
        return e

    def run(self, args, timeout, cua=None, **env):
        return sh([str(cua or self.cua), *args], timeout, env=self.env(**env))

    def discovery(self):
        try:
            return json.loads((self.path / "daemon.json").read_text())
        except Exception:
            return None


class Run:
    """One scenario run: its steps and their timings."""

    def __init__(self, scenario, n):
        self.scenario, self.n = scenario, n
        self.steps = []
        self.ok = True
        self.error = None
        self.notes = []
        self.t0 = time.monotonic()
        self.secs = 0.0

    def step(self, name, fn, budget):
        """Runs `fn()` and fails the run when it raises or takes longer than
        `budget` seconds (fn itself must bound its own waits)."""
        t = time.monotonic()
        try:
            detail = fn()
            secs = time.monotonic() - t
            ok = secs <= budget
            if not ok:
                detail = f"took {secs:.1f}s, budget {budget}s"
        except StepFailed as e:
            secs, ok, detail = time.monotonic() - t, False, str(e)
        except Exception as e:  # a bug in the suite is a failure too
            secs, ok, detail = time.monotonic() - t, False, f"{type(e).__name__}: {e}"
            traceback.print_exc()
        detail = "" if detail is None or detail is True else str(detail)
        self.steps.append({"step": name, "ok": ok, "secs": round(secs, 2), "detail": str(detail if detail is not None else "")[:600]})
        mark = "PASS" if ok else "FAIL"
        print(f"    [{mark}] {name} {secs:.1f}s {('- ' + detail[:300]) if detail and not ok else ''}", flush=True)
        if not ok:
            self.ok = False
            raise StepFailed(f"{name}: {detail}")
        return detail


def need(res, what, ok_codes=(0,)):
    if res.timed_out:
        raise StepFailed(f"{what}: timed out after {res.secs:.0f}s; output: {res.text()[-400:]}")
    if res.rc not in ok_codes:
        raise StepFailed(f"{what}: exit {res.rc}: {res.text()[-400:]}")
    return res


def wait_until(pred, timeout, every=0.5, what="condition"):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        v = pred()
        if v:
            return v
        time.sleep(every)
    raise StepFailed(f"{what} not met within {timeout}s")


# --------------------------------------------------------------- helpers


def daemon_start(h, budget=20, **env):
    r = need(h.run(["daemon", "start"], budget, **env), "cua daemon start")
    return r.text()


def daemon_status(h):
    return h.run(["daemon", "status", "--json"], 15).json() or {}


def space_row(h, sid):
    r = h.run(["spaces", "ls", "--json"], 30)
    need(r, "cua spaces ls")
    rows = r.json()
    rows = rows.get("spaces", rows) if isinstance(rows, dict) else rows
    for s in rows or []:
        if s.get("id") == sid:
            return s
    return None


def create_space(h, image, name, budget, extra=()):
    r = h.run(["spaces", "create", image, "--name", name, "--json", *extra], budget)
    need(r, f"create {image}")
    j = r.json() or {}
    rows = j.get("spaces", [])
    if not rows or "error" in rows[0]:
        raise StepFailed(f"create {image}: {rows}")
    return rows[0]


def ready(h, sid, budget):
    """Ready: cua-spacesd connected (a version and features) and, within
    `budget`, the guest doctor reports no failed check. The create returns
    once cua-spacesd answers (by design the desktop may still be starting),
    so the doctor is polled; the time the desktop took is in the detail."""
    t = time.monotonic()
    row = space_row(h, sid)
    if not row or not row.get("spacesd_version"):
        raise StepFailed(f"{sid} has no cua-spacesd version: {row}")
    feats = row.get("features") or []
    end = t + budget
    first_fail = None
    while True:
        r = h.run(["doctor", sid, "--json", "--no-host"], max(10, min(90, end - time.monotonic())))
        rep = r.json()
        if rep is None:
            failed = [f"doctor exit {r.rc}: {r.text()[-300:]}"]
        else:
            checks = [c for c in rep.get("checks", []) if c.get("status") == "fail"]
            skew = []
            failed = [f"{c.get('id')}: {c.get('message')} {c.get('facts') or ''}"
                      for c in checks if c not in skew]
        if not failed:
            if rep is not None and skew:
                first_fail = (first_fail or []) + [f"note: {skew[0].get('message')}"]
            settle = f", desktop settled after {time.monotonic() - t:.1f}s ({first_fail[0][:80]})" if first_fail else ""
            return f"spacesd {row['spacesd_version']}, {len(feats)} features, {len(rep.get('checks', []))} doctor checks{settle}"
        first_fail = first_fail or failed
        if time.monotonic() + 3 > end:
            raise StepFailed(f"doctor {sid}: failed checks after {budget}s: {failed}")
        time.sleep(3)


def screenshot(h, sid, tmp, budget=90):
    out = tmp / f"{sid.replace(':', '_')}.png"
    need(h.run(["sb", "screenshot", sid, "-o", str(out)], budget), "screenshot")
    data = out.read_bytes() if out.exists() else b""
    if not data.startswith(b"\x89PNG") or len(data) < 4096:
        raise StepFailed(f"screenshot not a real PNG ({len(data)} bytes)")
    return f"{len(data)} bytes"


def exec_ok(h, sid, budget=60, cmd="echo cua-ok"):
    r = need(h.run(["sb", "exec", sid, cmd], budget), "exec")
    if "cua-ok" not in r.out:
        raise StepFailed(f"exec printed {r.out[-200:]!r}")
    return r.out.strip()[-80:]


def delete_space(h, sid, budget=240):
    need(h.run(["spaces", "delete", sid, "--force"], budget), "delete")
    if space_row(h, sid):
        raise StepFailed(f"{sid} still listed after delete")


# ------------------------------------------------------------- scenarios


class Suite:
    def __init__(self, args):
        self.cua = pathlib.Path(args.cua).expanduser()
        self.mit_cua = pathlib.Path(args.mit_cua).expanduser() if args.mit_cua else None
        self.other_cua = pathlib.Path(args.other_cua).expanduser() if args.other_cua else None
        self.teleport_test = pathlib.Path(args.teleport_test).expanduser() if args.teleport_test else None
        self.work = pathlib.Path(args.work).expanduser()
        self.runs = args.runs
        self.prefix = "rel" + uuid.uuid4().hex[:4]
        self.homes = []
        self.results = []
        self.tmp = self.work / "tmp"
        self.tmp.mkdir(parents=True, exist_ok=True)
        (self.work / "results").mkdir(parents=True, exist_ok=True)
        self.version = sh([str(self.cua), "--version"], 10).out.strip().split()[-1]
        self.preexisting_vms = set(self.lume_names())

    # -- host guards

    def lume_names(self):
        r = sh(["lume", "ls", "--format", "json"], 30)
        try:
            return [v["name"] for v in json.loads(r.out)]
        except Exception:
            return [line.split()[0] for line in r.out.splitlines()[1:] if line.strip()]

    def stop_home(self, path):
        """Stops the daemon of one of our homes: `cua daemon stop`, then (only
        if it is still alive and is our cua) SIGKILL by its recorded pid."""
        try:
            d = json.loads((path / "daemon.json").read_text())
        except Exception:
            d = None
        env = {k: v for k, v in os.environ.items() if not k.startswith("CUA_")}
        env["CUA_HOME"] = str(path)
        env["CUA_CREDENTIAL_STORE"] = "file"
        sh([str(self.cua), "daemon", "stop"], 20, env=env)
        if d and d.get("pid") and pid_alive(d["pid"]):
            exe = pid_exe(d["pid"])
            ours = {str(self.cua), str(self.mit_cua or ""), str(self.other_cua or "")}
            if exe in ours:
                time.sleep(2)
                if pid_alive(d["pid"]):
                    os.kill(d["pid"], signal.SIGKILL)

    def cleanup_home(self, h):
        """Deletes every Space the home registered, then any Lume VM or
        container it created that is still there."""
        r = h.run(["spaces", "ls", "--json"], 30)
        try:
            rows = r.json()
            rows = rows.get("spaces", rows) if isinstance(rows, dict) else rows
        except Exception:
            rows = []
        for s in rows or []:
            if str(s.get("id", "")).startswith("local:"):
                for attempt in (1, 2):
                    d = h.run(["spaces", "delete", s["id"], "--force"], 300)
                    print(f"    cleanup: delete {s['id']} (try {attempt}): exit {d.rc} {d.secs:.1f}s {d.text()[-200:]}", flush=True)
                    if d.rc == 0:
                        break
        owned = h.path / "vmm" / "lume" / "owned"
        live = set(self.lume_names()) or set(self.lume_names())
        for f in owned.glob("*.json") if owned.exists() else []:
            try:
                rec = json.loads(f.read_text())
            except Exception:
                continue
            name = rec.get("name", "")
            if rec.get("kind") == "instance" and name in live and name not in self.preexisting_vms:
                print(f"    cleanup: deleting leftover Lume VM {name}", flush=True)
                sh(["lume", "stop", name], 120)
                sh(["lume", "delete", name, "--force"], 180)
        r = sh(["docker", "ps", "-a", "--format", "{{.Names}}"], 30)
        for n in r.out.split():
            if n.startswith(self.prefix):
                print(f"    cleanup: removing leftover container {n}", flush=True)
                sh(["docker", "rm", "-f", n], 60)
        self.stop_home(h.path)

    # -- driver

    def scenario(self, name, fn):
        for n in range(1, self.runs + 1):
            run = Run(name, n)
            print(f"== {name} run {n}/{self.runs}", flush=True)
            h = None
            try:
                # Short: unix sockets under the home must fit SUN_LEN.
                h = Home(self, f"{ABBREV.get(name, name[:6])}{n}")
                fn(run, h, n)
            except StepFailed as e:
                run.error = str(e)
            except Exception as e:
                run.ok = False
                run.error = f"{type(e).__name__}: {e}"
                traceback.print_exc()
            finally:
                if h is not None:
                    try:
                        self.cleanup_home(h)
                    except Exception as e:
                        run.notes.append(f"cleanup: {e}")
                run.secs = round(time.monotonic() - run.t0, 1)
                self.results.append(run)
                print(f"   -> {'PASS' if run.ok else 'FAIL'} ({run.secs}s){' ' + run.error[:300] if run.error else ''}", flush=True)

    def name(self, tag, n):
        return f"{self.prefix}-{tag}-{n}"

    # 1. fresh start
    def s_fresh(self, run, h, n):
        def not_running():
            s = daemon_status(h)
            if s.get("pid"):
                raise StepFailed(f"a daemon already answers in a fresh home: {s}")
            return "no daemon"
        run.step("status (no daemon)", not_running, 15)
        if n % 2:
            run.step("daemon start", lambda: daemon_start(h), 15)
        else:
            # What an MCP client (a coding agent) does: `cua daemon mcp`
            # starts the daemon when none runs (stdin closed: it exits).
            def autostart():
                r = h.run(["daemon", "mcp"], 30)
                if r.timed_out:
                    raise StepFailed("cua daemon mcp hung with stdin closed")
                s = daemon_status(h)
                if not s.get("pid"):
                    raise StepFailed(f"cua daemon mcp did not start a daemon: {r.text()[-200:]}")
                return f"autostarted pid {s['pid']}"
            run.step("daemon autostart (cua daemon mcp)", autostart, 30)

        def version():
            s = daemon_status(h)
            if s.get("daemon_version") != self.version or s.get("mode") != "daemon":
                raise StepFailed(f"status {s}, cua --version {self.version}")
            return f"daemon {s['daemon_version']} pid {s['pid']}"
        run.step("version reported", version, 15)

        def extensions():
            out = daemon_start(h)
            if "already running" not in out:
                raise StepFailed(f"second start did not find the Cua Spaces daemon complete: {out}")
            return out
        run.step("extension set complete (second start keeps it)", extensions, 15)
        run.step("spaces ls answers", lambda: need(h.run(["spaces", "ls", "--json"], 20), "ls").out[:80], 20)

    # 2/3. lifecycle
    def lifecycle(self, run, h, n, image, tag, create_budget, extra=()):
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name(tag, n)
        sid = f"local:{name}"
        run.step("create", lambda: create_space(h, image, name, create_budget, extra)["id"], create_budget)
        run.step("ready (spacesd + doctor)", lambda: ready(h, sid, 180), 180)
        run.step("desktop screenshot", lambda: screenshot(h, sid, self.tmp), 90)
        run.step("exec", lambda: exec_ok(h, sid), 60)
        run.step("stop (suspend)", lambda: need(h.run(["sb", "suspend", sid], 180), "suspend").text()[-80:], 180)
        run.step("resume", lambda: need(h.run(["sb", "resume", sid], 600), "resume").text()[-80:], 600)
        def clock():
            # The first doctor after the resume: its clock check must pass.
            rep = h.run(["doctor", sid, "--json", "--no-host", "--only", "time"], 90).json() or {}
            skew = next((c for c in rep.get("checks", []) if c.get("id") == "time.skew"), None)
            if not skew or skew.get("status") == "fail":
                raise StepFailed(f"time.skew: {skew}")
            return skew.get("message", "")
        run.step("clock check after resume", clock, 90)
        run.step("exec after resume", lambda: wait_until(lambda: h.run(["sb", "exec", sid, "echo cua-ok"], 30).out.count("cua-ok"), 240, 3, "exec after resume") and "ok", 240)
        run.step("ready after resume", lambda: ready(h, sid, 180), 180)

        run.step("delete", lambda: delete_space(h, sid), 240)

    def s_linux(self, run, h, n):
        self.lifecycle(run, h, n, "linux", "lnx", 600)

    def s_macos_slim(self, run, h, n):
        self.lifecycle(run, h, n, "macos:26-slim", "mslim", 3600, ("--memory-mb", "6144", "--cpus", "4"))

    def s_macos(self, run, h, n):
        self.lifecycle(run, h, n, "macos:26", "mfull", 3600, ("--memory-mb", "6144", "--cpus", "4"))

    # 4. Cua Volume
    def volume(self, run, h, n, image, tag, extra=()):
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name(tag, n)
        sid = f"local:{name}"
        run.step("create", lambda: create_space(h, image, name, 3600, extra)["id"], 3600)
        run.step("ready", lambda: ready(h, sid, 300), 300)

        def vol_status():
            return need(h.run(["volume", "status", "--json"], 30), "volume status").json() or {}

        def mounted():
            def probe():
                st = vol_status()
                for e in st.get("volume_errors") or []:
                    if e.get("space") == sid:
                        raise StepFailed(f"volume error: {e.get('error')}")
                return next((v for v in st.get("volumes") or [] if v.get("space") == sid), None)
            return json.dumps(wait_until(probe, 150, 2, "volume attached"))
        info = json.loads(run.step("volume attached at start", mounted, 150))
        mnt = info.get("mount_path", "/volume")
        folder = ('"$HOME"/' + "'" + mnt[2:] + "'") if mnt.startswith("~/") else f"'{mnt}'"

        def round_trip():
            # The Space writes its own folder, spaces/<id with : as ->.
            folder_name = sid.replace(":", "-")
            space_dir = f"spaces/{folder_name}"
            gdir = f"{folder}/spaces/{folder_name}"
            token = uuid.uuid4().hex
            need(h.run(["sb", "exec", sid, f"echo {token} > {gdir}/guest.txt && sync"], 60), "guest write")
            wait_until(lambda: token in h.run(["volume", "cat", f"{space_dir}/guest.txt"], 20).out, 60, 2,
                       f"host sees the guest write in {space_dir}")
            src = self.tmp / f"{name}.txt"
            htoken = uuid.uuid4().hex
            src.write_text(htoken)
            need(h.run(["volume", "put", f"{space_dir}/host.txt", str(src)], 30), "volume put")
            wait_until(lambda: htoken in h.run(["sb", "exec", sid, f"cat {gdir}/host.txt 2>/dev/null || true"], 30).out,
                       60, 2, "guest sees the host write")
            return f"round trip {space_dir} <-> {mnt}/spaces/{folder_name}"
        run.step("round trip host <-> guest", round_trip, 180)
        run.step("delete", lambda: delete_space(h, sid), 300)

        # Failure: storage that cannot work. The Space must still be ready,
        # and the volume error must be visible.
        def break_storage():
            # S3 with no keys and nothing listening: no usable storage.
            r = h.run(["volume", "config", "set", "--backend", "s3", "--bucket", "cua-reliability-missing",
                       "--endpoint", "http://127.0.0.1:9", "--path-style"], 30)
            return f"exit {r.rc}: {r.text()[-200:]}"
        run.step("inject: unusable volume storage", break_storage, 30)
        run.step("daemon restart", lambda: (h.run(["daemon", "stop"], 20), daemon_start(h))[1], 40)
        name2 = self.name(tag + "f", n)
        sid2 = f"local:{name2}"
        run.step("create with broken volume", lambda: create_space(h, image, name2, 3600, extra)["id"], 3600)
        run.step("ready despite volume failure", lambda: ready(h, sid2, 300), 300)

        def visible_error():
            def probe():
                st = vol_status()
                err = next((e for e in st.get("volume_errors") or [] if e.get("space") == sid2), None)
                if err:
                    return f"volume_errors: {err['error'][:240]}"
                if any(v.get("space") == sid2 for v in st.get("volumes") or []):
                    return "mounted anyway (cache in front of the store)"
                return None
            return wait_until(probe, 150, 3, "a volume verdict for the Space")
        run.step("volume error is visible (or it mounts)", visible_error, 150)
        run.step("delete", lambda: delete_space(h, sid2), 300)

    def s_volume(self, run, h, n):
        self.volume(run, h, n, "linux", "vol")

    def s_volume_macos(self, run, h, n):
        self.volume(run, h, n, "macos:26-slim", "mvol", ("--memory-mb", "6144", "--cpus", "4"))

    # 5. daemon conflicts
    def s_stale_daemon(self, run, h, n):
        other = self.mit_cua if n % 2 else (self.other_cua or self.mit_cua)
        if not other:
            raise StepFailed("needs --mit-cua (and optionally --other-cua)")

        def start_other():
            env = h.env()
            p = subprocess.Popen([str(other), "daemon", "start", "--foreground"], env=env,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            wait_until(lambda: (h.discovery() or {}).get("pid") == p.pid, 15, 0.2, "other daemon up")
            return f"{other.name} daemon pid {p.pid} version {(h.discovery() or {}).get('version')}"
        run.step("another bundle's daemon on the socket", start_other, 20)
        old = (h.discovery() or {}).get("pid")

        def ls_bounded():
            r = h.run(["spaces", "ls", "--json"], 30)
            if r.timed_out:
                raise StepFailed("spaces ls hung against the other daemon")
            return f"exit {r.rc}"
        run.step("client does not hang on it", ls_bounded, 30)

        def replaced():
            out = daemon_start(h, 30)
            s = daemon_status(h)
            if s.get("pid") == old or s.get("daemon_version") != self.version:
                raise StepFailed(f"not replaced: {out} / {s}")
            if pid_alive(old):
                raise StepFailed(f"old daemon {old} still running: {out}")
            return out
        run.step("daemon start replaces it", replaced, 30)

    def s_stale_socket(self, run, h, n):
        import socket as so

        def plant():
            sock = h.path / "cua.sock"
            s = so.socket(so.AF_UNIX, so.SOCK_STREAM)
            s.bind(str(sock))
            s.close()
            dead = subprocess.Popen(["true"])
            dead.wait()
            (h.path / "daemon.json").write_text(json.dumps({
                "pid": dead.pid, "socket_path": str(sock), "loopback_url": None,
                "token": "t", "version": "0.0.1"}))
            return f"socket + discovery of dead pid {dead.pid}"
        run.step("plant stale socket", plant, 5)
        run.step("status is 'not running', bounded", lambda: need(h.run(["daemon", "status", "--json"], 15), "status", (1,)).out[:120], 15)
        run.step("spaces ls works (embedded)", lambda: need(h.run(["spaces", "ls", "--json"], 20), "ls").out[:80], 20)
        run.step("daemon start", lambda: daemon_start(h), 15)
        def version():
            s = daemon_status(h)
            if s.get("daemon_version") != self.version:
                raise StepFailed(f"status {s}")
            return f"daemon {s['daemon_version']} pid {s['pid']}"
        run.step("version", version, 15)

    def s_crash(self, run, h, n):
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name("crash", n)
        sid = f"local:{name}"
        state = {}

        def crash_mid_create():
            p = subprocess.Popen([str(self.cua), "spaces", "create", "linux", "--name", name, "--json"],
                                 env=h.env(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                 start_new_session=True)
            # Mid-create: the container exists (booting / starting services).
            wait_until(lambda: name in sh(["docker", "ps", "-a", "--format", "{{.Names}}"], 10).out, 120, 0.5, "container created")
            time.sleep(1)
            pid = (h.discovery() or {}).get("pid")
            os.kill(pid, signal.SIGKILL)
            try:
                out, _ = p.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)
                raise StepFailed("the client hung after its daemon died")
            state["client"] = (p.returncode, out[-300:])
            return f"killed daemon {pid}; client exit {p.returncode}: {out.strip()[-200:]}"
        run.step("SIGKILL the daemon mid-create; client returns", crash_mid_create, 180)
        run.step("daemon restart", lambda: daemon_start(h), 20)

        def recovered():
            def settled():
                row = space_row(h, sid)
                # Any state: a created-but-not-started container counts.
                live = name in sh(["docker", "ps", "-a", "--format", "{{.Names}}"], 10).out.split()
                return (row, live)
            end = time.monotonic() + 120
            row = live = None
            while time.monotonic() < end:
                row, live = settled()
                if row and row.get("spacesd_version"):
                    break
                if not row and not live:
                    break
                time.sleep(3)
            if live and not row:
                raise StepFailed(f"zombie: container {name} exists but no Space lists it")
            if row:
                exec_ok(h, sid)
                return f"recovered: {sid} listed and answers"
            return "reported: create failed, nothing left running"
        run.step("Space recovered or cleanly gone (no zombie)", recovered, 180)

    def s_two_clients(self, run, h, n):
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name("two", n)
        sid = f"local:{name}"
        run.step("create", lambda: create_space(h, "linux", name, 600)["id"], 600)

        def concurrent():
            procs = []
            for i in range(2):
                script = f"for i in 1 2 3 4 5; do '{self.cua}' sb exec {sid} 'echo cua-ok-{i}' || exit 1; '{self.cua}' spaces ls --json >/dev/null || exit 2; done"
                procs.append(subprocess.Popen(["bash", "-c", script], env=h.env(), stdout=subprocess.PIPE,
                                              stderr=subprocess.STDOUT, text=True, start_new_session=True))
            outs = []
            for p in procs:
                try:
                    out, _ = p.communicate(timeout=180)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
                    raise StepFailed("a client hung")
                if p.returncode != 0:
                    raise StepFailed(f"client exit {p.returncode}: {out[-300:]}")
                outs.append(out)
            if not all(o.count("cua-ok") == 5 for o in outs):
                raise StepFailed(f"missing output: {outs}")
            return "2 clients x 5 exec+ls"
        run.step("two clients at once", concurrent, 180)
        run.step("delete", lambda: delete_space(h, sid), 240)

    # 6. restart recovery
    def s_restart(self, run, h, n):
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name("rst", n)
        sid = f"local:{name}"
        run.step("create", lambda: create_space(h, "linux", name, 600)["id"], 600)
        run.step("daemon stop", lambda: need(h.run(["daemon", "stop"], 30), "stop").out.strip(), 30)
        run.step("daemon gone", lambda: wait_until(lambda: not daemon_status(h).get("pid"), 15, 0.5, "daemon gone") and "gone", 15)
        run.step("daemon start again", lambda: daemon_start(h), 20)
        def listed():
            if not space_row(h, sid):
                raise StepFailed(f"{sid} not listed after the restart")
            return "listed"
        run.step("Space still listed", listed, 30)
        run.step("reconnects (exec)", lambda: exec_ok(h, sid, 60), 60)
        run.step("ready", lambda: ready(h, sid, 180), 180)
        run.step("delete", lambda: delete_space(h, sid), 240)

    # 7. network off / relay unreachable
    def s_offline(self, run, h, n):
        bad = {"CUA_RELAY_URL": "http://127.0.0.1:9", "CUA_DAEMON_NO_RELAY": None,
               "CUA_API_URL": "http://127.0.0.1:9", "CUA_FLEET_URL": "http://127.0.0.1:9",
               "HTTPS_PROXY": "http://127.0.0.1:9", "https_proxy": "http://127.0.0.1:9",
               "NO_PROXY": "127.0.0.1,localhost,ghcr.io,pkg-containers.githubusercontent.com,*.docker.io,docker.io"}
        h.extra.update({k: v for k, v in bad.items() if v is not None})
        h.extra.pop("CUA_DAEMON_NO_RELAY", None)

        def fake_session():
            # A signed-in session (file store) whose relay and API are
            # unreachable: the daemon's relay attach and Fleet both fail.
            import base64
            b = lambda d: base64.urlsafe_b64encode(json.dumps(d).encode()).rstrip(b"=").decode()
            tok = f"{b({'alg': 'none'})}.{b({'sub': 'reliability', 'exp': int(time.time()) + 3600})}.x"
            exp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 3500))
            path = h.path / "credentials.json"
            path.write_text(json.dumps({"access_token": tok, "refresh_token": None,
                                        "expires_at": exp, "token_type": "Bearer"}))
            path.chmod(0o600)
            return "fake session"
        run.step("signed in, with nothing reachable", fake_session, 5)
        run.step("daemon start (relay unreachable)", lambda: daemon_start(h, 20, CUA_DAEMON_NO_RELAY=None), 20)
        run.step("spaces ls bounded", lambda: need(h.run(["spaces", "ls", "--json"], 30, CUA_DAEMON_NO_RELAY=None), "ls").out[:80], 30)
        name = self.name("off", n)
        sid = f"local:{name}"
        run.step("create local", lambda: create_space(h, "linux", name, 600)["id"], 600)
        run.step("exec", lambda: exec_ok(h, sid), 60)

        def cloud_error():
            r = h.run(["spaces", "create", "linux", "--on", "cloud", "--json"], 60, CUA_DAEMON_NO_RELAY=None)
            if r.timed_out:
                raise StepFailed("cloud create hung while offline")
            if r.rc == 0:
                raise StepFailed(f"cloud create succeeded offline?: {r.out[:200]}")
            return r.text()[-200:]
        run.step("cloud create fails fast, clearly", cloud_error, 60)
        run.step("local still fine", lambda: exec_ok(h, sid), 60)
        run.step("delete", lambda: delete_space(h, sid), 240)

    # 8. low disk
    def s_lowdisk(self, run, h, n):
        low = {"CUA_DISK_MIN_FREE": "100T"}
        h.extra.update(low)
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name("disk", n)
        img = f"docker.io/library/alpine:3.{17 + n}"

        def refused():
            sh(["docker", "image", "rm", img], 30)
            r = h.run(["spaces", "create", img, "--name", name, "--json"], 60)
            if r.timed_out:
                raise StepFailed("create hung under low disk")
            if r.rc == 0 and "error" not in r.out:
                raise StepFailed(f"create went ahead: {r.out[:300]}")
            txt = r.text()
            if "cache prune" not in txt and "disk" not in txt.lower():
                raise StepFailed(f"refusal does not say why: {txt[-300:]}")
            if sh(["docker", "image", "inspect", img], 20).rc == 0:
                raise StepFailed("the image was pulled despite the refusal")
            return txt[-240:]
        run.step("create refused before the pull", refused, 60)

    # 9. teleport (no secrets) into a macOS Space
    def s_teleport(self, run, h, n):
        if not self.teleport_test:
            raise StepFailed("needs --teleport-test (cargo test -p cua-spaces-ext --test e2e_teleport_app --no-run)")
        run.step("daemon start", lambda: daemon_start(h), 20)
        name = self.name("tp", n)
        sid = f"local:{name}"
        run.step("create macos:26", lambda: create_space(h, "macos:26", name, 3600, ("--memory-mb", "6144", "--cpus", "4"))["id"], 3600)
        run.step("ready", lambda: ready(h, sid, 300), 300)

        def teleport():
            info = need(h.run(["sb", "info", sid, "--json"], 30), "sb info").json() or {}
            url = (info.get("endpoints") or {}).get("env")
            token = json.loads((h.path / "spaces-credentials.json").read_text())[sid]["token"]
            env = h.env(CUA_SPACES_E2E_MACOS_URL=url, CUA_SPACES_E2E_MACOS_TOKEN=token,
                        RUST_LOG="warn")
            r = sh([str(self.teleport_test), "e2e_teleport_chrome_without_sign_ins_into_a_macos_space",
                    "--exact", "--nocapture"], 900, env=env)
            log = self.tmp / f"teleport-{name}.log"
            log.write_text(r.out + "\n" + r.err)
            if r.timed_out or r.rc != 0 or "1 passed" not in r.out or "skipped:" in (r.out + r.err):
                raise StepFailed(f"exit {r.rc}; see {log}: {(r.out + r.err)[-500:]}")
            timing = [l for l in (r.out + r.err).splitlines() if l.startswith("TIMING")]
            return "; ".join(timing)[-300:]
        run.step("teleport Chrome, signed-in state unticked", teleport, 900)
        run.step("delete", lambda: delete_space(h, sid), 300)

    # --------------------------------------------------------- report

    def report(self):
        by = {}
        for r in self.results:
            by.setdefault(r.scenario, []).append(r)
        lines = [f"cua {self.version} ({self.cua})", "",
                 "| Scenario | Runs | Pass | p50 | max | Failures |", "|---|---|---|---|---|---|"]
        for s, rs in by.items():
            secs = [r.secs for r in rs]
            fails = "; ".join(f"run {r.n}: {r.error[:160]}" for r in rs if not r.ok)
            lines.append(f"| {s} | {len(rs)} | {sum(r.ok for r in rs)}/{len(rs)} | {statistics.median(secs):.0f}s | {max(secs):.0f}s | {fails or '-'} |")
        lines += ["", "Per step (p50 / max over passing runs):", ""]
        for s, rs in by.items():
            steps = {}
            for r in rs:
                for st in r.steps:
                    if st["ok"]:
                        steps.setdefault(st["step"], []).append(st["secs"])
            if steps:
                lines.append(f"- **{s}**: " + ", ".join(f"{k} {statistics.median(v):.1f}/{max(v):.1f}s" for k, v in steps.items()))
        return "\n".join(lines)


ABBREV = {"teleport": "tp", "volume-macos": "mvol", "stale-daemon": "sdmn", "stale-socket": "ssck", "two-clients": "two", "macos-slim": "mslim", "macos": "mfull"}

SCENARIOS = {
    "fresh": Suite.s_fresh,
    "linux": Suite.s_linux,
    "volume": Suite.s_volume,
    "stale-daemon": Suite.s_stale_daemon,
    "stale-socket": Suite.s_stale_socket,
    "crash": Suite.s_crash,
    "two-clients": Suite.s_two_clients,
    "restart": Suite.s_restart,
    "offline": Suite.s_offline,
    "lowdisk": Suite.s_lowdisk,
    "macos-slim": Suite.s_macos_slim,
    "macos": Suite.s_macos,
    "teleport": Suite.s_teleport,
    "volume-macos": Suite.s_volume_macos,
}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cua", required=True, help="the app's bundled cua (Cua Spaces build)")
    ap.add_argument("--mit-cua", help="an MIT-only cua (no Cua Spaces extensions), for the stale-daemon scenario")
    ap.add_argument("--other-cua", help="another bundle's cua (for example an older app), for the stale-daemon scenario")
    ap.add_argument("--teleport-test", help="the e2e_teleport_app test binary (cargo test -p cua-spaces-ext --test e2e_teleport_app --no-run)")
    ap.add_argument("--scenarios", default=",".join(SCENARIOS))
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--work", default=str(HOME / "projects/.cua-work/reliability"))
    args = ap.parse_args()
    if os.environ.get("CUA_HOME") and pathlib.Path(os.environ["CUA_HOME"]).resolve() == REAL_CUA_HOME.resolve():
        sys.exit("refusing: CUA_HOME is the real ~/.cua")
    suite = Suite(args)
    print(f"cua {suite.version}; work {suite.work}; prefix {suite.prefix}; preexisting VMs {sorted(suite.preexisting_vms)}", flush=True)
    for name in [s.strip() for s in args.scenarios.split(",") if s.strip()]:
        if name not in SCENARIOS:
            sys.exit(f"unknown scenario {name}; one of {', '.join(SCENARIOS)}")
        df = sh(["df", "-g", str(HOME)], 10).out.splitlines()[-1].split()
        if int(df[3]) < 60:
            sys.exit(f"refusing: only {df[3]} GiB free under {HOME}")
        suite.scenario(name, lambda run, h, n, f=SCENARIOS[name]: f(suite, run, h, n))
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = suite.work / "results" / stamp
    out.with_suffix(".json").write_text(json.dumps([r.__dict__ for r in suite.results], indent=1, default=str))
    md = suite.report()
    out.with_suffix(".md").write_text(md + "\n")
    print("\n" + md)
    sys.exit(0 if all(r.ok for r in suite.results) else 1)


if __name__ == "__main__":
    main()
