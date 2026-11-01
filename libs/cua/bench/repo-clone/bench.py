#!/usr/bin/env python3
"""A fresh agent in a fresh Space clones a repository (the repo cache).

Scenario: create a Space, get an agent ready, run `git clone <url>` as the
agent's first bash call. Every trial is timed in four parts and in total:

    space_create_ms   the Space exists and its cua-spacesd answers
    agent_ready_ms    the agent process is ready (only with --agent-cmd; null
                      otherwise: this harness starts no model)
    first_bash_ms     the first no-op command's round trip
    clone_ms          `git clone` of the repository, to the end
    total_ms          from the create request to the end of the clone

Cases (what the repo cache holds when the Space clones):

    off        the cache is off
    miss       the cache is on and empty: the clone downloads (the daemon
               starts filling the cache from it)
    hit        a verified mirror is held; the helper asks the remote for its
               tip (the lease is off), so the clone is never older than it
    hit_lease  a mirror verified a moment ago: no network at all

Every trial is also checked, and a trial that fails any check is counted as
failed and is not measured: the clone's HEAD is the remote's HEAD, `git fsck`
passes, the clone has no alternate and no credential in its config, and a hit
really came from the cache. With `--canary` an object written in one clone is
never visible in another clone or in the mirror.

Modes:

    sim    no Space: a temporary directory with the git config, helper and
           mirror a container Space sees, so the clone runs exactly as in a
           Space, on this machine (what `cua bench` can run anywhere with git
           and the network).
    space  real Spaces: `cua spaces create` and `cua sb exec` (needs a Linux
           container runtime); the daemon of the same `cua` binary and its own
           CUA_HOME.

Usage:
    CUA=/path/to/cua python3 bench.py --mode sim --repos small,medium --n 5 \\
        --out raw/repo-clone.json
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from typing import Dict, List, Optional

REPOS = {
    "small": "https://github.com/octocat/Hello-World",
    "medium": "https://github.com/vitejs/vite",
    "large": "https://github.com/trycua/cua",
}
CASES = ("off", "miss", "hit", "hit_lease")
SCHEMA = "cua.repo-clone/v1"
PHASES = ("space_create_ms", "agent_ready_ms", "first_bash_ms", "clone_ms", "total_ms")


def ms(a: float, b: float) -> float:
    return round((b - a) * 1000, 1)


def pct(values: List[float], p: float) -> Optional[float]:
    v = sorted(values)
    if not v:
        return None
    k = (len(v) - 1) * p / 100
    f = int(math.floor(k))
    c = min(f + 1, len(v) - 1)
    return round(v[f] + (v[c] - v[f]) * (k - f), 1)


def stats(values: List[float]) -> Dict[str, float]:
    if not values:
        return {}
    return {"p50": pct(values, 50), "p95": pct(values, 95), "min": round(min(values), 1), "max": round(max(values), 1)}


def run(cmd, env=None, cwd=None, timeout=1800) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, env=env, cwd=cwd, capture_output=True, text=True, timeout=timeout)


class Cua:
    """The `cua` binary with a Spaces home of its own."""

    def __init__(self, binary: str, home: str):
        self.binary = binary
        self.home = home
        self.env = dict(os.environ, CUA_HOME=home, DO_NOT_TRACK="1", CUA_TELEMETRY="0")
        self.root = os.path.join(home, "job-cache", "repos")

    def __call__(self, *args, timeout=1800) -> subprocess.CompletedProcess:
        return run([self.binary, *args], env=self.env, timeout=timeout)

    def set_cache(self, on: bool) -> None:
        self("host", "config", "--repo-cache", "on" if on else "off")

    def fill(self, url: str) -> bool:
        return self("host", "cache", "fill", url).returncode == 0

    def clear(self) -> None:
        self("host", "cache", "clear")


class SimSpace:
    """What a container Space sees, without the container: the cache's git
    config as the system config, the helper on PATH, the mirrors where the
    helper looks."""

    def __init__(self, cua: Cua, case: str):
        self.cua = cua
        self.case = case
        self.dir = tempfile.mkdtemp(prefix="repo-clone-sim-")
        self.log = os.path.join(self.dir, "helper.log")

    def create(self) -> None:
        os.makedirs(os.path.join(self.dir, "work"), exist_ok=True)

    def env(self) -> dict:
        # What a Space has, not what this machine has: no token of the host's.
        keep = ("PATH", "LANG", "TERM", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "NO_PROXY", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR")
        e = {k: os.environ[k] for k in keep if k in os.environ}
        e.update(HOME=self.dir, GIT_CONFIG_GLOBAL="/dev/null", GIT_TERMINAL_PROMPT="0", CUA_REPO_CACHE_LOG=self.log)
        if self.case != "off":
            root = self.cua.root
            e.update(
                GIT_CONFIG_SYSTEM=os.path.join(root, "gitconfig"),
                CUA_REPO_CACHE_ROOT=root,
                PATH=os.path.join(root, "bin") + os.pathsep + e["PATH"],
            )
            if self.case == "hit":
                e["CUA_REPO_CACHE_LEASE"] = "0"
        else:
            e["GIT_CONFIG_SYSTEM"] = "/dev/null"
        return e

    def exec(self, command: str) -> subprocess.CompletedProcess:
        return run(["sh", "-c", command], env=self.env(), cwd=os.path.join(self.dir, "work"))

    def helper_log(self) -> str:
        try:
            return open(self.log).read()
        except OSError:
            return ""

    def delete(self) -> None:
        shutil.rmtree(self.dir, ignore_errors=True)


class RealSpace:
    """A Linux container Space through the real CLI."""

    def __init__(self, cua: Cua, case: str, image: str, kind: str):
        self.cua = cua
        self.case = case
        self.name = f"bench-clone-{uuid.uuid4().hex[:8]}"
        self.image = image
        self.kind = kind

    def create(self) -> None:
        r = self.cua("spaces", "create", self.image, "--name", self.name, "--kind", self.kind, timeout=900)
        if r.returncode != 0:
            raise RuntimeError(f"cua spaces create failed: {r.stderr.strip()[-400:]}")

    def exec(self, command: str) -> subprocess.CompletedProcess:
        return self.cua("sb", "exec", self.name, command, timeout=1800)

    def helper_log(self) -> str:
        return self.exec("cat /tmp/.cua-repo-cache.log 2>/dev/null; true").stdout

    def delete(self) -> None:
        self.cua("spaces", "delete", self.name, timeout=300)


def remote_head(url: str) -> Optional[str]:
    r = run(["git", "ls-remote", url, "HEAD"], env=dict(os.environ, GIT_TERMINAL_PROMPT="0"), timeout=120)
    return r.stdout.split()[0] if r.returncode == 0 and r.stdout.strip() else None


def prepare(cua: Cua, case: str, url: str, first: bool) -> None:
    """What the cache holds before one trial."""
    if case == "off":
        if first:
            cua.set_cache(False)
        return
    if first:
        cua.set_cache(True)
    if case == "miss":
        cua.clear()
    elif case in ("hit", "hit_lease"):
        # A hit needs a mirror; `hit_lease` also needs it verified a moment
        # ago, so it is filled (or confirmed) right before every trial.
        if case == "hit_lease" or first or not os.path.isdir(cua.root):
            if not cua.fill(url):
                raise RuntimeError(f"the cache could not hold {url} (private, too big, or unreachable)")


def trial(make_space, cua: Cua, case: str, url: str, agent_cmd: Optional[str]) -> dict:
    space = make_space()
    out: dict = {"ok": False}
    t0 = time.perf_counter()
    try:
        space.create()
        t1 = time.perf_counter()
        t2 = t1
        if agent_cmd:
            space.exec(agent_cmd)
            t2 = time.perf_counter()
        space.exec("true")
        t3 = time.perf_counter()
        where = f"/tmp/bench-clone-{uuid.uuid4().hex[:8]}"
        r = space.exec(f"git clone --quiet {url} {where} 2>&1")
        t4 = time.perf_counter()
        out.update(
            space_create_ms=ms(t0, t1),
            agent_ready_ms=ms(t1, t2) if agent_cmd else None,
            first_bash_ms=ms(t2, t3),
            clone_ms=ms(t3, t4),
            total_ms=ms(t0, t4),
        )
        if r.returncode != 0:
            out["error"] = (r.stdout + r.stderr).strip()[-300:]
            return out
        head = space.exec(f"git -C {where} rev-parse HEAD").stdout.strip()
        want = remote_head(url)
        log = space.helper_log()
        checks = {
            "head_matches_remote": bool(want) and head == want,
            "fsck_ok": space.exec(f"git -C {where} fsck --connectivity-only >/dev/null 2>&1").returncode == 0,
            "no_alternate": space.exec(f"test ! -e {where}/.git/objects/info/alternates").returncode == 0,
            "no_credentials_in_config": space.exec(f"! grep -qE '://[^/]*@' {where}/.git/config").returncode == 0,
            "no_token_in_env": space.exec("env | grep -qiE '(token|secret|password|api_key)=' && exit 1 || exit 0").returncode == 0,
        }
        if case in ("hit", "hit_lease"):
            checks["served_from_cache"] = "hit_" in log
        if case in ("off", "miss"):
            checks["not_served_from_cache"] = "hit_" not in log
        out["checks"] = checks
        out["ok"] = all(checks.values())
        return out
    except Exception as e:  # a failed trial is recorded, never hidden
        out["error"] = str(e)[-300:]
        return out
    finally:
        space.exec("rm -rf /tmp/bench-clone-*")
        space.delete()


def canary(make_space, cua: Cua, url: str) -> dict:
    """An object written in one clone is not visible in another clone, and
    never reaches the mirror."""
    a, b = make_space(), make_space()
    try:
        a.create()
        b.create()
        where_a = f"/tmp/bench-canary-{uuid.uuid4().hex[:8]}"
        where_b = f"/tmp/bench-canary-{uuid.uuid4().hex[:8]}"
        for s, where in ((a, where_a), (b, where_b)):
            if s.exec(f"git clone --quiet {url} {where}").returncode != 0:
                return {"ran": False}
        h = a.exec(f"echo canary-$$-$(date +%s) | git -C {where_a} hash-object -w --stdin").stdout.strip()
        seen_b = b.exec(f"git -C {where_b} cat-file -e {h}").returncode == 0
        mirror_has = False
        if isinstance(a, SimSpace):
            for name in os.listdir(os.path.join(cua.root, "m")):
                if name.endswith(".git"):
                    d = os.path.join(cua.root, "m", name)
                    if run(["git", "-C", d, "cat-file", "-e", h]).returncode == 0:
                        mirror_has = True
        return {"ran": True, "other_clone_sees_it": seen_b, "mirror_has_it": mirror_has, "ok": not seen_b and not mirror_has}
    finally:
        for s in (a, b):
            s.exec("rm -rf /tmp/bench-canary-*")
            s.delete()


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--mode", choices=("sim", "space"), default="sim")
    ap.add_argument("--repos", default="small,medium", help="small, medium, large or URLs, comma separated")
    ap.add_argument("--cases", default=",".join(CASES))
    ap.add_argument("--n", type=int, default=5, help="trials per repository and case")
    ap.add_argument("--cua", default=os.environ.get("CUA", "cua"))
    ap.add_argument("--home", default=None, help="the Spaces home (default: a temporary one)")
    ap.add_argument("--image", default="linux")
    ap.add_argument("--kind", default="container")
    ap.add_argument("--agent-cmd", default=None, help="a command that returns once the agent is ready")
    ap.add_argument("--canary", action="store_true", help="also check isolation between two clones")
    ap.add_argument("--out", required=True, help="the native result file (JSON)")
    args = ap.parse_args(argv)

    home = args.home or tempfile.mkdtemp(prefix="repo-clone-home-")
    cua = Cua(args.cua, home)
    cases = [c for c in args.cases.split(",") if c]
    bad = [c for c in cases if c not in CASES]
    if bad:
        ap.error(f"unknown case {bad}; cases: {', '.join(CASES)}")
    repos = {}
    for r in [x for x in args.repos.split(",") if x]:
        repos[r] = REPOS.get(r, r)

    def make(case):
        if args.mode == "sim":
            return lambda: SimSpace(cua, case)
        return lambda: RealSpace(cua, case, args.image, args.kind)

    result: dict = {
        "schema": SCHEMA,
        "mode": args.mode,
        "n": args.n,
        "agent_ready": "measured" if args.agent_cmd else "not measured",
        "repos": {},
    }
    for name, url in repos.items():
        entry = {"cases": {}}
        for case in cases:
            samples = []
            for i in range(args.n):
                try:
                    prepare(cua, case, url, i == 0)
                except RuntimeError as e:
                    samples.append({"ok": False, "error": str(e)})
                    continue
                samples.append(trial(make(case), cua, case, url, args.agent_cmd))
            good = [s for s in samples if s.get("ok")]
            entry["cases"][case] = {
                "samples": samples,
                "failed": len(samples) - len(good),
                "stats": {p: stats([s[p] for s in good if s.get(p) is not None]) for p in PHASES},
            }
            print(
                f"{name} {case}: clone p50 {entry['cases'][case]['stats']['clone_ms'].get('p50')} ms, "
                f"total p50 {entry['cases'][case]['stats']['total_ms'].get('p50')} ms, failed {entry['cases'][case]['failed']}",
                file=sys.stderr,
            )
        if args.canary:
            prepare(cua, "hit", url, True)
            entry["canary"] = canary(make("hit"), cua, url)
        result["repos"][name] = {"url": url if name not in REPOS else name, **entry}
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(result, f, indent=2)
        f.write("\n")
    if not args.home:
        shutil.rmtree(home, ignore_errors=True)
    failed = sum(c["failed"] for r in result["repos"].values() for c in r["cases"].values())
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
