#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
"""Cloud conformance suite: Spaces and sandboxes in your own cloud account
(`--on aws|gcp|modal`), end to end, against the real cloud and relay.cua.ai.

    cloud_suite.py --cua PATH --on aws,gcp,modal [--runs 3]
        [--scenarios space,bench] [--home ~/projects/.cua-work/cloud-providers/home]
        [--inventory ~/projects/.cua-work/cloud-providers/inventory.sh]
        [--work ~/projects/.cua-work/cloud-conformance] [--image linux]

Why a sibling of suite.py (whose helpers it imports) and not more scenarios
in it: suite.py builds a throwaway CUA_HOME per run and cleans up local VMs
and containers; a cloud run needs the opposite. It uses one signed-in home (the relay
account and device enrollment live there, and a new home would be a new
device) that it never deletes, the cloud's own inventory before and after
every run, and a cost per run.

Per provider and run, each step timed:

  space:  create (`cua spaces create --on P`), ready (spacesd and the guest
          doctor), screenshot, exec, files (a round trip, sha256), desktop
          stream over the relay (frames), window stream, presence (the
          probe joins and reads the roster), Cua Volume (only with the Cua
          Spaces `cua`; n/a otherwise), stop and start (n/a on Modal),
          delete, orphan check (the cloud shows nothing Cua tagged for this
          run; `cua cloud status` records nothing for it; every resource
          the account had before is unchanged).
  bench:  one cua-bench-basic task through Python cua-sandbox on="P"
          (`cb run ... --on P`), then the same orphan check.

Results: <work>/results/<stamp>.json and a Markdown matrix (stdout and
<stamp>.md), with timings and the estimated cost (uptime times the
provider's `usd_per_hour` for the image).

Safety (hard rules):
- One cloud machine per provider at a time (at most two relay machines of
  this suite at once, well under the account's 32); providers run one after
  another.
- It deletes only what it created, by the names it chose
  (`<prefix>-<provider>-<n>`), through `cua`; it never runs `cloud sweep
  --delete` (it reports what a dry-run sweep still sees).
- It never deletes or rewrites the home; it never touches `~/.cua`.
- It stops only the daemon of the home it runs in, and only at the end
  (the persistent home keeps no daemon between suites).
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import sys
import time
import traceback
import uuid

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from suite import REAL_CUA_HOME, Run, StepFailed, need, sh, wait_until  # noqa: E402

HOME = pathlib.Path.home()
DEFAULT_HOME = HOME / "projects/.cua-work/cloud-providers/home"
DEFAULT_INVENTORY = HOME / "projects/.cua-work/cloud-providers/inventory.sh"
PROVIDERS = ("aws", "gcp", "modal")
CAN_STOP = {"aws": True, "gcp": True, "modal": False}
# CUA_* variables passed through to the cua under test.
# Doctor checks a provider cannot pass with the published images, by id
# prefix, and why (see the "Use your own cloud" guide).
KNOWN_LIMITS = {
    # Modal runs its own PID 1 (dumb-init under gVisor, its agent in the
    # VM runtime): the image's init and systemd-unit checks do not apply.
    # The next linux image and driver judge PID 1 by its program instead.
    "modal": ("init.", "compat.unit."),
}

PASS_ENV = ("CUA_MODAL_HELPER", "CUA_MODAL_APP", "CUA_RELAY_URL", "CUA_CLOUD_CUA_URL")
# Inventory files the suite compares; each line is one resource.
TERMINAL = ("terminated", "shutting-down", "TERMINATED", "STOPPING", "deleted")


class CloudHome:
    """The persistent signed-in home, as the suite uses it (never deleted)."""

    def __init__(self, path: pathlib.Path, cua: pathlib.Path):
        self.path = path.expanduser().resolve()
        if self.path == REAL_CUA_HOME.resolve():
            raise SystemExit("refusing to run against the real ~/.cua")
        if not self.path.is_dir():
            raise SystemExit(f"{self.path}: no signed-in test home (sign it in with `cua auth login --remote`)")
        self.cua = cua

    def env(self, **over):
        # The caller's CUA_* settings never leak in, except the ones that
        # say where a provider's helper or the relay is.
        e = {k: v for k, v in os.environ.items() if not k.startswith("CUA_") or k in PASS_ENV}
        e.update({
            "CUA_HOME": str(self.path),
            "CUA_CREDENTIAL_STORE": "file",
            "CUA_NO_BROWSER": "1",
            "CUA_TELEMETRY": "0",
            "CUA_TELEMETRY_FORBID_NETWORK": "1",
            "DO_NOT_TRACK": "1",
            "CUA_SPACES_TELEPORT_HOME": str(self.path / "teleport"),
        })
        for k, v in over.items():
            if v is None:
                e.pop(k, None)
            else:
                e[k] = v
        return e

    def run(self, args, timeout, **env):
        return sh([str(self.cua), *args], timeout, env=self.env(**env))


def jsonl(res):
    """The JSON a `--json` command printed (its last JSON line)."""
    for line in reversed((res.out or "").strip().splitlines()):
        line = line.strip()
        if line.startswith("{") or line.startswith("["):
            try:
                return json.loads(line)
            except Exception:
                continue
    return res.json()


class Suite:
    def __init__(self, a):
        self.cua = pathlib.Path(a.cua).expanduser()
        self.home = CloudHome(pathlib.Path(a.home), self.cua)
        self.inventory = pathlib.Path(a.inventory).expanduser()
        # The inventory taken when the sandbox account, project and
        # environment were set up: what "pre-existing" means.
        base = sorted(self.inventory.parent.glob("inventory/baseline-*"))
        self.baseline = base[0] if base else None
        self.work = pathlib.Path(a.work).expanduser()
        self.tmp = self.work / "tmp"
        self.tmp.mkdir(parents=True, exist_ok=True)
        (self.work / "results").mkdir(parents=True, exist_ok=True)
        self.providers = [p.strip() for p in a.on.split(",") if p.strip()]
        for p in self.providers:
            if p not in PROVIDERS:
                raise SystemExit(f"--on {p}: one of {', '.join(PROVIDERS)}")
        self.runs = a.runs
        self.scenarios = [s.strip() for s in a.scenarios.split(",") if s.strip()]
        self.image = a.image
        self.bench_task = a.bench_task
        self.bench_dir = pathlib.Path(a.bench_dir).expanduser() if a.bench_dir else None
        self.prefix = "cc" + uuid.uuid4().hex[:4]
        self.results = []
        self.spaces_build = "volume" in self.home.run(["--help"], 30).out

    # -- inventory

    def snapshot(self, label):
        if not self.inventory.exists():
            return None
        r = sh(["bash", str(self.inventory), label], 600)
        path = (r.out or "").strip().splitlines()[-1:] or [""]
        d = pathlib.Path(path[0])
        if not d.is_absolute():
            d = self.inventory.parent / d
        return d if d.is_dir() else None

    # Sets of environments the suite never writes to (the payer account,
    # Modal's "main" environment): other teams' workloads change them all
    # the time, so they are reported, never a failure. Writing there at all
    # is impossible for the suite: every cloud call goes to the sandbox
    # account, project and environment.
    FOREIGN_SETS = ("payer-", "modal-main-")

    @staticmethod
    def diff(before, after, baseline=None):
        """Pre-existing resources that changed or vanished, and resources
        left behind (new lines not in a terminal state). With `baseline`
        (the inventory taken when the sandbox account, project and
        environment were set up), "pre-existing" is what was there then:
        the default network, groups, roles, service accounts; resources of
        earlier runs that are still terminating are not pre-existing."""
        changed, leftover = [], []
        if not before or not after:
            return ["no inventory"], []

        def lines(d, name):
            f = d / name
            return set(f.read_text().splitlines()) if f.exists() else set()

        # Changed: what the account had at setup (or before this run) and no
        # longer has.
        for f in sorted((baseline or before).glob("*.txt")):
            if f.name.startswith(Suite.FOREIGN_SETS):
                continue
            a = lines(after, f.name)
            ids_after = {line.split()[0] for line in a if line.split()}
            for line in sorted(set(f.read_text().splitlines()) - a):
                rid = line.split()[0] if line.split() else line
                if rid not in ids_after:
                    changed.append(f"{f.name}: {line} (gone)")
        # Left behind: what appeared during this run and is not ending.
        for f in sorted(after.glob("*.txt")):
            if f.name.startswith(Suite.FOREIGN_SETS):
                continue
            for line in sorted(lines(after, f.name) - lines(before, f.name)):
                if not any(t in line for t in TERMINAL):
                    leftover.append(f"{f.name}: {line}")
        return changed, leftover

    # -- helpers

    def status(self, provider):
        r = need(self.home.run(["--json", "cloud", "status", provider], 120), "cloud status")
        j = jsonl(r) or {}
        rows = [p for p in j.get("providers", []) if p.get("name") == provider]
        if not rows or not rows[0].get("connected"):
            raise StepFailed(f"{provider} is not connected in {self.home.path} (`cua cloud connect {provider}`)")
        return rows[0], j.get("resources", [])

    def price(self, row, family="linux"):
        for k in row.get("kinds", []):
            if k.get("image") == family and k.get("supported"):
                return float(k.get("usd_per_hour") or 0), k.get("machine_type", "")
        return 0.0, ""

    def relay_ids(self):
        r = self.home.run(["--json", "spaces", "ls"], 60)
        j = jsonl(r)
        rows = j.get("spaces", j) if isinstance(j, dict) else (j or [])
        return {s.get("id") for s in rows if str(s.get("id", "")).startswith("relay:")}

    def orphan_check(self, provider, name, before, t_start):
        after = self.snapshot(f"after-{self.prefix}-{provider}-{name}")
        _, resources = self.status(provider)
        mine = [r for r in resources if str(r.get("sandbox", "")).endswith(f":{name}")]
        if mine:
            raise StepFailed(f"cloud status still records {mine}")
        sweep = jsonl(self.home.run(["--json", "cloud", "sweep", provider], 300)) or {}
        seen = [r for r in sweep.get("resources", [])
                if name in json.dumps(r) and r.get("action") not in ("keep",)]
        changed, leftover = self.diff(before, after, self.baseline)
        # Other Cua runs in the same account (another suite, a developer)
        # create and delete their own tagged resources meanwhile: a change
        # to one of those is theirs. Everything else the account had must be
        # exactly as it was.
        changed = [c for c in changed if "cua-" not in c or name in c or self.prefix in c]
        # Shared resources Cua keeps and reuses (a security group, a
        # network) are recorded with no sandbox; they are not leftovers.
        shared = {r.get("id") for r in resources if not r.get("sandbox") and r.get("id")}
        leftover = [l for l in leftover if (name in l or self.prefix in l or "cua-" in l)
                    and not any(i in l for i in shared)]
        problems = []
        if changed:
            problems.append(f"pre-existing resources changed: {changed[:5]}")
        if leftover:
            problems.append(f"left behind: {leftover[:5]}")
        if seen:
            problems.append(f"sweep still sees: {seen[:3]}")
        if problems:
            raise StepFailed("; ".join(problems))
        return f"nothing left; {sum(1 for _ in before.glob('*.txt')) if before else 0} inventory sets unchanged"

    # -- scenarios

    def s_space(self, run, provider, n):
        row, _ = self.status(provider)
        usd, machine_type = self.price(row)
        run.notes.append(f"{row.get('label')} {machine_type} ~${usd:.4f}/h")
        name = f"{self.prefix}-{provider[:3]}-{n}"
        ref = f"{provider}:{name}"
        before = run.step("inventory before", lambda: str(self.snapshot(f"before-{self.prefix}-{provider}-{n}")), 600)
        before = pathlib.Path(before) if before and before != "None" else None
        relay_before = self.relay_ids()
        if len(relay_before) > 24:
            raise StepFailed(f"{len(relay_before)} relay machines already on the account (cap 32); not creating more")
        t0 = time.monotonic()
        sid = {}

        def create():
            r = self.home.run(["--json", "spaces", "create", self.image, "--on", provider, "--name", name], 1800)
            need(r, "spaces create")
            j = jsonl(r) or {}
            rows = j.get("spaces", [j]) if isinstance(j, dict) else j
            if not rows or "error" in rows[0]:
                raise StepFailed(f"create: {rows}")
            sid["id"] = rows[0]["id"]
            if not sid["id"].startswith("relay:"):
                raise StepFailed(f"a cloud Space is a relay machine, got {sid['id']}")
            return sid["id"]

        try:
            run.step("create", create, 1800)

            def ready():
                t = time.monotonic()
                while True:
                    r = self.home.run(["--json", "doctor", ref, "--no-host"], 180)
                    rep = r.json()
                    checks = (rep or {}).get("checks", [])
                    fails = [c for c in checks if c.get("status") in ("fail", "error")
                             and not str(c.get("id", "")).startswith("time.")]
                    # Known platform limits (documented in the guide): the
                    # published image's driver checks for its own init and
                    # systemd units, which a Modal sandbox replaces with
                    # Modal's PID 1. Recorded, not a failure.
                    limits = [c for c in fails
                              if str(c.get("id", "")).startswith(KNOWN_LIMITS.get(provider, ()))]
                    fails = [c for c in fails if c not in limits]
                    if limits and not any(n.startswith("known limits") for n in run.notes):
                        run.notes.append("known limits: " + ", ".join(c.get("id") for c in limits))
                    # A doctor that reached the guest runs its guest checks;
                    # one or two host-side lines mean it did not.
                    if rep is not None and not fails and len(checks) >= 5:
                        return f"{len(rep.get('checks', []))} doctor checks, settled in {time.monotonic() - t:.0f}s"
                    if time.monotonic() - t > 300:
                        raise StepFailed(f"doctor: {[c.get('id') for c in fails][:6] or r.text()[-300:]}")
                    time.sleep(5)
            run.step("ready (spacesd + doctor)", ready, 330)

            def screenshot():
                out = self.tmp / f"{name}.png"
                need(self.home.run(["sb", "screenshot", ref, "-o", str(out)], 120), "screenshot")
                data = out.read_bytes() if out.exists() else b""
                if not data.startswith(b"\x89PNG") or len(data) < 4096:
                    raise StepFailed(f"not a real PNG ({len(data)} bytes)")
                return f"{len(data)} bytes"
            run.step("screenshot", screenshot, 120)

            def exec_():
                r = need(self.home.run(["sb", "exec", ref, "echo", "cua-ok"], 90), "exec")
                if "cua-ok" not in r.out:
                    raise StepFailed(f"exec printed {r.out[-200:]!r}")
                return "cua-ok"
            run.step("exec", exec_, 90)

            def files():
                src = self.tmp / f"{name}.bin"
                src.write_bytes(os.urandom(256 * 1024))
                back = self.tmp / f"{name}.back"
                need(self.home.run(["sb", "cp", str(src), f"{ref}:/tmp/cua-conf.bin"], 180), "cp up")
                need(self.home.run(["sb", "cp", f"{ref}:/tmp/cua-conf.bin", str(back)], 180), "cp down")
                h1 = hashlib.sha256(src.read_bytes()).hexdigest()
                h2 = hashlib.sha256(back.read_bytes()).hexdigest() if back.exists() else ""
                if h1 != h2:
                    raise StepFailed("the file came back different")
                return f"256 KiB round trip, sha256 {h1[:12]}"
            run.step("files", files, 360)

            def stream(extra, what):
                r = need(self.home.run(["sb", "stream-probe", ref, "--seconds", "5", *extra], 180), what)
                j = jsonl(r) or {}
                if int(j.get("frames") or 0) < 1:
                    raise StepFailed(f"{what}: no frames: {r.text()[-300:]}")
                return j
            run.step("desktop stream over the relay",
                     lambda: "{frames} frames, {fps} fps, first frame {first_frame_ms} ms, {width}x{height}".format(
                         **stream([], "desktop stream")), 180)

            def window():
                need(self.home.run(["sb", "exec", ref,
                                    "DISPLAY=:1 nohup xfce4-terminal >/dev/null 2>&1 &"], 60),
                     "open a terminal")
                time.sleep(5)
                j = stream(["--window-app", "terminal"], "window stream")
                return f"{j['target']}: {j['frames']} frames"
            run.step("window stream", window, 240)

            def presence():
                j = stream(["--presence-check"], "presence")
                p = j.get("presence")
                if p is None:
                    raise StepFailed("the probe reported no presence roster")
                return f"presence: {json.dumps(p)[:120]}"
            run.step("presence", presence, 180)

            # The published linux:24.04 still runs cua-guestd (from before the
            # cua-spacesd rename), which has no Cua Volume guest mount: n/a
            # there, measured on images whose driver has it.
            legacy = "legacy" in self.home.run(
                ["sb", "exec", ref, "test -x /usr/local/bin/cua-spacesd && echo current || echo legacy"],
                60).out
            if self.spaces_build and legacy:
                run.notes.append("Cua Volume: n/a (the image's driver is the pre-rename cua-guestd, "
                                 "which has no guest volume mount; the next linux image has it)")
            elif self.spaces_build:
                def volume():
                    st = jsonl(self.home.run(["--json", "volume", "status"], 60)) or {}
                    v = next((v for v in st.get("volumes") or [] if v.get("space") == sid["id"]), None)
                    err = next((e for e in st.get("volume_errors") or [] if e.get("space") == sid["id"]), None)
                    if v:
                        return f"mounted at {v.get('mount_path')}"
                    if err:
                        raise StepFailed(f"volume error: {err.get('error')}")
                    raise StepFailed("no volume verdict for the Space")
                # Recorded, and the run goes on: stop, start and delete are
                # measured whatever the Volume did.
                try:
                    run.step("Cua Volume", volume, 120)
                except StepFailed:
                    # The run stays failed; the steps after it still run.
                    run.notes.append("Cua Volume failed (see its step)")
            else:
                run.notes.append("Cua Volume: n/a (needs the Cua Spaces `cua`)")

            if CAN_STOP[provider]:
                run.step("stop", lambda: need(self.home.run(["spaces", "stop", sid["id"]], 600), "stop").out.strip()[-120:], 600)
                run.step("start", lambda: need(self.home.run(["spaces", "start", sid["id"]], 1200), "start").out.strip()[-120:], 1200)
                run.step("exec after start", lambda: wait_until(
                    lambda: "cua-ok" in self.home.run(["sb", "exec", ref, "echo", "cua-ok"], 60).out,
                    300, 5, "exec after start") and "cua-ok", 300)
            else:
                run.notes.append("stop/start: n/a (a Modal sandbox cannot stop)")

            def delete():
                need(self.home.run(["spaces", "delete", sid["id"], "--force"], 900), "delete")
                if sid["id"] in self.relay_ids():
                    raise StepFailed(f"{sid['id']} still on the relay")
                sid.pop("id")
                return "deleted"
            run.step("delete", delete, 900)
        finally:
            uptime = time.monotonic() - t0
            run.notes.append(f"uptime {uptime / 60:.1f} min, est. ${usd * uptime / 3600:.4f}")
            run.cost = usd * uptime / 3600
            if sid.get("id"):
                d = self.home.run(["spaces", "delete", sid["id"], "--force"], 900)
                run.notes.append(f"cleanup delete {sid['id']}: exit {d.rc} {d.text()[-160:]}")
            leftover = self.home.run(["sb", "rm", ref, "--force"], 600)
            if leftover.rc == 0 and "not found" not in leftover.text().lower():
                run.notes.append(f"cleanup rm {ref}: {leftover.text()[-120:]}")
            # The orphan check runs after a failure too: a failed run must
            # still leave nothing behind.
            try:
                run.step("orphan check", lambda: self.orphan_check(provider, name, before, t0), 900)
            except StepFailed as e:
                run.error = run.error or str(e)

    def s_bench(self, run, provider, n):
        row, _ = self.status(provider)
        usd, _ = self.price(row)
        before = self.snapshot(f"before-{self.prefix}-bench-{provider}-{n}")
        bench = self.bench_dir or (pathlib.Path(__file__).resolve().parents[4] / "libs" / "cua-bench")
        t0 = time.monotonic()

        def run_task():
            r = sh(["uv", "run", "--frozen", "cb", "run", "cua-bench-basic", "--on", provider,
                    "--task-filter", self.bench_task, "--max-variants", "1", "-j", "1"],
                   3600, env=self.home.env(), cwd=str(bench))
            need(r, "cb run")
            tail = r.text()[-400:]
            if "error" in tail.lower() and "0 error" not in tail.lower():
                raise StepFailed(tail)
            return tail.strip().splitlines()[-1] if tail.strip() else "ran"
        try:
            run.step(f"cua-bench-basic {self.bench_task} on {provider}", run_task, 3600)
        finally:
            uptime = time.monotonic() - t0
            run.cost = usd * uptime / 3600
            run.notes.append(f"uptime {uptime / 60:.1f} min, est. ${run.cost:.4f}")
        run.step("orphan check", lambda: self.bench_orphans(provider, before), 900)

    def bench_orphans(self, provider, before):
        _, resources = self.status(provider)
        eph = [r for r in resources if ":cua-eph-" in str(r.get("sandbox", "")) or not r.get("sandbox")
               and r.get("type") in ("instance", "sandbox")]
        if eph:
            raise StepFailed(f"cloud status still records {eph}")
        changed, leftover = self.diff(before, self.snapshot(f"after-{self.prefix}-bench-{provider}"),
                                      self.baseline)
        changed = [c for c in changed if "cua-" not in c]
        shared = {r.get("id") for r in resources if not r.get("sandbox") and r.get("id")}
        leftover = [l for l in leftover if "cua-" in l and not any(i in l for i in shared)]
        if changed or leftover:
            raise StepFailed(f"changed {changed[:3]} left {leftover[:3]}")
        return "nothing left"

    # -- driver

    def main(self):
        print(f"cloud suite {self.prefix}: {self.cua} in {self.home.path}; "
              f"{', '.join(self.providers)} x {self.runs}; {', '.join(self.scenarios)}", flush=True)
        for provider in self.providers:
            for scen in self.scenarios:
                fn = {"space": self.s_space, "bench": self.s_bench}[scen]
                for n in range(1, self.runs + 1):
                    run = Run(f"{scen}:{provider}", n)
                    run.cost = 0.0
                    print(f"== {scen} on {provider}, run {n}/{self.runs}", flush=True)
                    try:
                        fn(run, provider, n)
                    except StepFailed as e:
                        run.ok = False
                        run.error = run.error or str(e)
                    except Exception as e:  # a bug in the suite is a failure too
                        run.ok = False
                        run.error = f"{type(e).__name__}: {e}"
                        traceback.print_exc()
                    run.secs = round(time.monotonic() - run.t0, 1)
                    self.results.append(run)
                    print(f"   -> {'PASS' if run.ok else 'FAIL'} ({run.secs}s, ~${run.cost:.4f})"
                          f"{' ' + run.error[:300] if run.error else ''}", flush=True)
        self.home.run(["daemon", "stop"], 30)
        return self.report()

    def report(self):
        stamp = dt.datetime.now().strftime("%Y%m%dT%H%M%S")
        rows = [{"scenario": r.scenario, "run": r.n, "ok": r.ok, "secs": r.secs,
                 "cost_usd": round(getattr(r, "cost", 0.0), 4), "error": r.error,
                 "notes": r.notes, "steps": r.steps} for r in self.results]
        out = self.work / "results" / f"{stamp}.json"
        out.write_text(json.dumps({"prefix": self.prefix, "cua": str(self.cua), "runs": rows}, indent=2))
        lines = ["| Scenario | Run | Result | Time | Est. cost | Steps (s) |", "|---|---|---|---|---|---|"]
        for r in self.results:
            steps = ", ".join(f"{s['step']} {s['secs']}" + ("" if s["ok"] else " FAIL") for s in r.steps)
            lines.append(f"| {r.scenario} | {r.n} | {'pass' if r.ok else 'FAIL'} | {r.secs}s | "
                         f"${getattr(r, 'cost', 0.0):.4f} | {steps} |")
        total = sum(getattr(r, "cost", 0.0) for r in self.results)
        lines.append(f"\nEstimated compute cost, all runs: ${total:.4f}")
        md = "\n".join(lines)
        (self.work / "results" / f"{stamp}.md").write_text(md + "\n")
        print(md)
        print(f"results: {out}")
        return 0 if all(r.ok for r in self.results) else 1


def main():
    a = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    a.add_argument("--cua", required=True, help="the cua to test (MIT `cua` or the Cua Spaces build)")
    a.add_argument("--on", required=True, help="aws, gcp, modal (comma list)")
    a.add_argument("--runs", type=int, default=3)
    a.add_argument("--scenarios", default="space,bench")
    a.add_argument("--image", default="linux")
    a.add_argument("--home", default=str(DEFAULT_HOME))
    a.add_argument("--inventory", default=str(DEFAULT_INVENTORY))
    a.add_argument("--work", default=str(HOME / "projects/.cua-work/cloud-conformance"))
    a.add_argument("--bench-task", default="click-button")
    a.add_argument("--bench-dir", default=None, help="libs/cua-bench (default: this repo's)")
    return Suite(a.parse_args()).main()


if __name__ == "__main__":
    sys.exit(main())
