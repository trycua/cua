#!/usr/bin/env python3
"""The runner behind `cua bench`: lists the Spaces benchmarks, runs one
through its own script (the scripts next to this file), and writes the run
as bench-result v1 (result.json + samples.csv, README.md) with the bench's
native output under raw/. `check` applies the bench's own perf guard to a
run against a committed baseline.

    python3 cuabench.py list [--json]
    python3 cuabench.py run NAME [--out DIR] [--n N] [--json]
    python3 cuabench.py check RESULT.json --baseline FILE [--host CLASS]
    python3 cuabench.py convert NAME RAW_DIR --out DIR   (an existing run)
    python3 cuabench.py manifest                         (files `cua` embeds)

`cua bench` runs this file with CUA set to its own binary. Exit codes: 0 ok,
1 the bench or its guard failed, 2 a usage or host problem.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import bench_result as br  # noqa: E402

IS_MAC = sys.platform == "darwin"


class Usage(Exception):
    """A problem with the request or the host (exit 2)."""


# ---------------------------------------------------------------- registry


@dataclass
class Bench:
    name: str
    measures: str
    host: str
    slug: str
    kind: str
    method: str
    files: List[str]
    needs: List[str] = field(default_factory=list)
    n_help: str = ""
    default_n: Optional[int] = None
    own_daemon: bool = False  # the script starts and stops a daemon itself
    needs_daemon: bool = False  # the script needs a running daemon
    platforms: tuple = ("darwin", "linux")
    caveats: List[str] = field(default_factory=list)
    guard: str = ""  # how `check` judges a run ("" = no guard)
    host_class: Dict[str, str] = field(default_factory=dict)  # default --host per OS


STARTUP_FILES = ["spaces-startup/bench.sh", "spaces-startup/sample_host.py", "spaces-startup/README.md"]

BENCHES: List[Bench] = [
    Bench(
        name="spaces-startup.microvm",
        measures="microVM Space start: cold (kernel boot) and warm (golden snapshot), checkpoint, fork of a checkpoint and of a running Space",
        host="Linux with KVM",
        slug="microvm-start",
        kind="microvm",
        method="`cua sb create --kind microvm` timed from the command until it returns (the Space's cua-spacesd answers with its own token), N times each for cold starts (warm snapshots off) and warm starts (restored from the golden snapshot); then N checkpoints and forks. The first start of each mode (image build, golden snapshot) is outside the samples. Serial. The value is the warm start p50.",
        files=STARTUP_FILES,
        n_help="starts per mode (N, default 20)",
        default_n=20,
        own_daemon=True,
        platforms=("linux",),
    ),
    Bench(
        name="spaces-startup.gvisor",
        measures="gVisor container Space start (image cached), checkpoint and fork of a checkpoint",
        host="Linux with gVisor (runsc)",
        slug="gvisor-start",
        kind="gvisor",
        method="`cua sb create --kind container --runtime gvisor` timed from the command until it returns (the Space's cua-spacesd answers with its own token), N times with the image cached; then N checkpoints and forks. Serial. The value is the start p50.",
        files=STARTUP_FILES,
        n_help="starts (N, default 20)",
        default_n=20,
        own_daemon=True,
        platforms=("linux",),
    ),
    Bench(
        name="spaces-startup.minimal",
        measures="A small service image (nginx:alpine) as a microVM Space, cold and warm, until its HTTP port answers; and as gVisor",
        host="Linux with KVM",
        slug="minimal-image-start",
        kind="microvm",
        method="`cua sb create nginx:alpine --kind microvm --port 80 --wait http:80/` timed until the port answers, N cold and N warm starts, then N gVisor starts. Serial. The value is the warm start p50.",
        files=STARTUP_FILES,
        n_help="starts per mode (N, default 20)",
        default_n=20,
        own_daemon=True,
        platforms=("linux",),
    ),
    Bench(
        name="spaces-startup.density",
        measures="How many microVM and gVisor Spaces run at once, and the memory each costs",
        host="Linux with KVM",
        slug="spaces-density",
        kind="microvm",
        method="Spaces started DENSITY_STEP at a time until available memory falls under DENSITY_FLOOR_MB or a step's Spaces do not all run; memory is the drop in MemAvailable. The value is the most microVM Spaces that ran at once.",
        files=STARTUP_FILES,
        n_help="most Spaces per kind (DENSITY_MAX, default 400)",
        own_daemon=True,
        platforms=("linux",),
    ),
    Bench(
        name="spaces-startup.vm-density",
        measures="How many QEMU VM Spaces one host runs at once, the memory each costs, and exec latency as they add up",
        host="macOS (HVF) or Linux with KVM",
        slug="vm-density",
        kind="vm",
        method="VM Spaces (1 vCPU, 1 GiB, the `rollout` profile) started 8 at a time through the SDK, all restoring one golden snapshot. After each step the host is sampled (private memory of every VM, shared snapshot memory once) and every VM runs 20 exec round trips. Stops at the first refused or failed create, exec p95 over 2x its one-VM value, critical memory pressure, or the most VMs asked for. The value is the most VMs that ran at once.",
        files=STARTUP_FILES
        + ["spaces-startup/baseline/macos-m2max-96g.json", "spaces-startup/baseline/linux-kvm-gha-ubuntu-24.04.json"],
        needs=["rollout_bench"],
        n_help="most VMs (VMD_MAX, default 512)",
        own_daemon=True,
        guard="spaces-startup/bench.sh guard",
    ),
    Bench(
        name="microvm-fork",
        measures="macOS microVM checkpoint, restore (a new Space from a checkpoint), fork of a running Space, 8 forks at once, and memory per fork",
        host="macOS 14+ on Apple silicon (cua-vz next to cua)",
        slug="microvm-fork",
        kind="microvm",
        method="A 2 vCPU / 2 GB microVM Space writes 256 MiB to its tmpfs; then N `cua sb checkpoint`, N `cua sb fork <checkpoint>` (restore) and N `cua sb fork <running Space>`, each timed until the command returns (the fork's cua-spacesd answers); then FANOUT forks of one checkpoint at once (wall time) and host memory per fork. Serial except the fan-out. The value is the restore p50.",
        files=["microvm-fork/bench.sh", "microvm-fork/check.py", "microvm-fork/baseline.json"],
        n_help="samples per measurement (N, default 5)",
        default_n=5,
        needs_daemon=True,
        platforms=("darwin",),
        guard="microvm-fork/check.py",
    ),
    Bench(
        name="fork-fanout",
        measures="Copy-on-write forks of one live QEMU checkpoint in waves (8, 32, 64 at once): time to a ready fork, wave wall time, private memory per fork",
        host="macOS (HVF) or Linux with KVM; a source checkout and cargo",
        slug="fork-fanout",
        kind="vm",
        method="A 2 GiB bake VM dirties 300 MB of RAM, is checkpointed and pinned, and the checkpoint is forked in waves, every fork of a wave started at once. A fork is ready when its cua-spacesd answers with its own token; the test checks every fork holds the world's state, is independent of the others and shares the world's unwritten pages. Host memory per fork is what the wave added (anonymous, wired and compressed pages). The value is the largest wave's p50 time to a ready fork.",
        files=["fork-fanout/run.sh", "fork-fanout/check.py", "fork-fanout/baseline.json"],
        needs=["source", "disk"],
        n_help="largest wave (WAVES, default 8,32,64)",
        guard="fork-fanout/check.py",
        host_class={"darwin": "macos-hvf"},
    ),
    Bench(
        name="fork-batch",
        measures="Batched forks through the SDK (`Sandboxes.fork_many`): each fork's time on its machine, first ready fork, whole batch",
        host="macOS (HVF) or Linux with KVM; the cua Python SDK",
        slug="fork-batch",
        kind="vm",
        method="A VM Space is baked into a memory checkpoint, then `fork_many` forks it COUNT times, at most 4 starting at once on this machine, streaming every event. Each fork's own time is measured on its machine. The value is the fork time p50.",
        files=["fork-batch/bench.py", "fork-batch/check.py", "fork-batch/baseline.json"],
        needs=["python-sdk", "disk"],
        n_help="forks (--count, default 8)",
        default_n=8,
        needs_daemon=True,
        guard="fork-batch/check.py",
        host_class={"darwin": "macos-hvf-m2max", "linux": "linux-kvm-gh"},
    ),
    Bench(
        name="channel",
        measures="The coordinator-to-Space channel through the SDK: a no-op call, exec round trips, file upload and download, streamed stdout",
        host="macOS (HVF) or Linux with KVM",
        slug="channel",
        kind="vm",
        method="A Linux VM Space (2 vCPU, 2 GiB) on this machine; `channel_bench` runs ITERS calls of each kind against it through the daemon, then moves files of each size both ways and streams a process's stdout. The value is the p50 of `exec` of `true` (round trip of a process run).",
        files=[
            "channel/run.sh",
            "channel/check.py",
            "channel/baselines/macos-m2max-qemu.json",
            "channel/baselines/ci-ubuntu-24.04-kvm-lane.json",
            "channel/baselines/aws-lan-mesh-c7g.json",
        ],
        needs=["channel_bench"],
        n_help="calls per latency metric (ITERS, default 300)",
        default_n=300,
        needs_daemon=True,
        guard="channel/check.py",
    ),
    Bench(
        name="rollouts",
        measures="Headless gVisor Spaces for RL rollouts: create to first exec, 16 at once, warm-pool claims, idle cost, rollout steps per second",
        host="macOS (built-in Linux runtime VM) or Linux with gVisor",
        slug="rollouts",
        kind="gvisor",
        method="Through the SDK and a daemon of its own: 3 desktop and 10 headless creates (serial, create through the first exec), 16 headless creates at once, N Spaces kept for a 20-step rollout, the idle CPU of a headless Space's cua-spacesd over 5 s, and 16 claims from a warm pool. The first create (image pull) is outside the samples. The value is the headless create p50.",
        files=["rollouts/guard.sh", "rollouts/check.py", "rollouts/baseline/macos-m2max-96g.json", "rollouts/baseline/linux-arm64-ci.json", "rollouts/baseline/linux-arm64-c7g.json"],
        needs=["rollout_bench"],
        n_help="Spaces in the rollout (N_ROLL, default 32)",
        default_n=32,
        own_daemon=True,
        guard="rollouts/check.py",
    ),
    Bench(
        name="pool-sessions",
        measures="Short browser sessions across all your machines (`cua sb run --on auto`), each in a fresh Space: Space start, run and delete times, sessions per hour",
        host="Your signed-in machines; a test site the Spaces reach (SITE)",
        slug="pool-sessions",
        kind="gvisor",
        method="COUNT sessions submitted with `cua sb run --on auto --detach` (CONCURRENCY at once), each a fresh gVisor Space that keeps a headless Chromium open for SESSION_S seconds, opening a page of a self-hosted test site about every 10 s, and is deleted when the session ends. Times come from the run's own records (`cua sb runs RUN --json`). The value is the Space start p50.",
        files=[
            "pool-sessions/run.sh",
            "pool-sessions/session.sh",
            "pool-sessions/make-site.py",
            "pool-sessions/analyze.py",
            "pool-sessions/window.py",
            "pool-sessions/sample.sh",
            "pool-sessions/node-stats.sh",
            "pool-sessions/README.md",
        ],
        needs=["site"],
        n_help="sessions (COUNT, default 20)",
        default_n=20,
    ),
]

BY_NAME: Dict[str, Bench] = {b.name: b for b in BENCHES}
COMMON_FILES = ["cuabench.py", "bench_result.py", "README.md"]


def manifest() -> List[str]:
    out = list(COMMON_FILES)
    for b in BENCHES:
        for f in b.files:
            if f not in out:
                out.append(f)
    return out


NEEDS_TEXT = {
    "rollout_bench": "ROLLOUT_BENCH: the SDK's rollout_bench example (`cargo build --release -p cua-sdk --example rollout_bench` in libs/cua of the matching release tag)",
    "channel_bench": "CHANNEL_BENCH: the SDK's channel_bench example (`cargo build --release -p cua-sdk --example channel_bench` in libs/cua of the matching release tag)",
    "source": "a source checkout of the matching release tag with cargo (--src DIR, CUA_SRC, or run from inside the checkout)",
    "disk": "DISK: a Linux Spaces disk image (`crane export --platform linux/arm64 ghcr.io/trycua/linux:24.04-slim-disk - | tar -x disk/disk.img`)",
    "python-sdk": "the cua Python SDK (`pip install cua`, or PYTHONPATH=libs/cua/python/src with its bindings built)",
    "site": "SITE: the base URL of the test site (pool-sessions/make-site.py) that the Spaces reach",
}


# -------------------------------------------------------------------- list


def cmd_list(args) -> int:
    rows = []
    for b in BENCHES:
        rows.append(
            {
                "name": b.name,
                "measures": b.measures,
                "host": b.host,
                "needs": [NEEDS_TEXT[n] for n in b.needs],
                "n": b.n_help,
                "guard": bool(b.guard),
                "runs_here": sys.platform in b.platforms,
            }
        )
    if args.json:
        print(json.dumps(rows, indent=2))
        return 0
    for r in rows:
        mark = "" if r["runs_here"] else "  (not on this OS)"
        print(f"{r['name']}{mark}")
        print(f"  measures: {r['measures']}")
        print(f"  host:     {r['host']}")
        print(f"  --n:      {r['n']}")
        for n in r["needs"]:
            print(f"  needs:    {n}")
        print(f"  guard:    {'cua bench check' if r['guard'] else 'none'}")
        print()
    return 0


# --------------------------------------------------------------- host info


def _sh(*cmd: str) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def host_info() -> dict:
    h: dict = {"name": platform.node().split(".")[0], "where": "local", "cpus": os.cpu_count()}
    if IS_MAC:
        h["model"] = _sh("sysctl", "-n", "hw.model")
        h["chip"] = _sh("sysctl", "-n", "machdep.cpu.brand_string")
        mem = _sh("sysctl", "-n", "hw.memsize")
        if mem.isdigit():
            h["memory_gb"] = round(int(mem) / 2**30)
        h["os"] = f"macOS {_sh('sw_vers', '-productVersion')}"
    else:
        try:
            with open("/proc/meminfo") as f:
                kb = int(next(line for line in f if line.startswith("MemTotal")).split()[1])
            h["memory_gb"] = round(kb / 2**20)
        except (OSError, StopIteration, ValueError):
            pass
        try:
            with open("/proc/cpuinfo") as f:
                for line in f:
                    if line.lower().startswith(("model name", "cpu model")):
                        h["chip"] = line.split(":", 1)[1].strip()
                        break
        except OSError:
            pass
        h["model"] = (open("/sys/devices/virtual/dmi/id/product_name").read().strip() if os.path.exists("/sys/devices/virtual/dmi/id/product_name") else "")
        osr = {}
        if os.path.exists("/etc/os-release"):
            for line in open("/etc/os-release"):
                k, _, v = line.strip().partition("=")
                osr[k] = v.strip('"')
        h["os"] = f"{osr.get('PRETTY_NAME', 'Linux')} (kernel {platform.release()})"
    return {k: v for k, v in h.items() if v not in ("", None)}


def other_vms() -> int:
    """VM processes already running on the host (another run's VMs skew a
    benchmark)."""
    out = _sh("pgrep", "-f", "qemu-system|cua-vz|firecracker|com.apple.Virtualization.VirtualMachine")
    return len([x for x in out.split() if x.strip()])


def cua_version(cua: str) -> str:
    out = _sh(cua, "--version")
    parts = out.split()
    return parts[-1] if parts else ""


# ------------------------------------------------------------------ daemon


class Daemon:
    """A cua daemon of this build in the foreground, for the benches that
    need one running; stopped by its PID afterwards. Never touches a daemon
    it did not start."""

    def __init__(self, cua: str, env: dict, log: str):
        self.cua, self.env, self.log = cua, env, log
        self.proc: Optional[subprocess.Popen] = None

    def running(self) -> bool:
        return subprocess.run([self.cua, "daemon", "status"], env=self.env, capture_output=True).returncode == 0

    def __enter__(self):
        if self.running():
            print(f"using the running daemon of CUA_HOME={self.env['CUA_HOME']}", file=sys.stderr)
            return self
        self.proc = subprocess.Popen(
            [self.cua, "daemon", "start", "--foreground"],
            env=self.env,
            stdout=open(self.log, "ab"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        for _ in range(300):
            if self.running():
                print(f"started a daemon (pid {self.proc.pid}, CUA_HOME={self.env['CUA_HOME']})", file=sys.stderr)
                return self
            if self.proc.poll() is not None:
                break
            time.sleep(0.2)
        self.__exit__(None, None, None)
        raise Usage(f"the daemon did not start; see {self.log}")

    def __exit__(self, *exc):
        if self.proc is None or self.proc.poll() is not None:
            return False
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout=60)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        return False


# --------------------------------------------------------------------- run


def find_source(explicit: Optional[str]) -> str:
    """libs/cua of a source checkout (for fork-fanout's cargo test)."""
    cands = [explicit, os.environ.get("CUA_SRC")]
    d = os.getcwd()
    while True:
        cands.append(d)
        parent = os.path.dirname(d)
        if parent == d:
            break
        d = parent
    cands.append(os.path.dirname(HERE))  # this file inside a checkout
    for c in cands:
        if not c:
            continue
        for libs in (c, os.path.join(c, "libs", "cua")):
            if os.path.exists(os.path.join(libs, "bench", "fork-fanout", "run.sh")) and os.path.exists(os.path.join(libs, "Cargo.toml")):
                return os.path.abspath(libs)
    raise Usage("fork-fanout needs " + NEEDS_TEXT["source"])


def need_env(var: str, what: str) -> str:
    v = os.environ.get(var, "")
    if not v:
        raise Usage(f"set {NEEDS_TEXT[what]}")
    if what in ("rollout_bench", "channel_bench") and not os.access(v, os.X_OK):
        raise Usage(f"{var}={v} is not an executable")
    if what == "disk" and not os.path.exists(v):
        raise Usage(f"DISK={v} does not exist")
    return v


def run_script(cmd: List[str], env: dict, cwd: str, log: str) -> int:
    print(f"$ {' '.join(cmd)}", file=sys.stderr)
    with open(log, "ab") as lf:
        p = subprocess.Popen(cmd, env=env, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        assert p.stdout is not None
        for chunk in iter(lambda: p.stdout.readline(), b""):
            lf.write(chunk)
            sys.stderr.buffer.write(chunk)
            sys.stderr.flush()
        return p.wait()


def latest(raw: str, suffix: str) -> Optional[str]:
    found = [os.path.join(raw, f) for f in os.listdir(raw) if f.endswith(suffix)]
    return max(found, key=os.path.getmtime) if found else None


def pool_sessions(b: Bench, n: int, env: dict, raw: str, log: str) -> int:
    site = need_env("SITE", "site")
    conc = os.environ.get("CONCURRENCY", str(min(n, 50)))
    image = os.environ.get("IMAGE", "ghcr.io/trycua/linux:24.04-slim")
    session_s = os.environ.get("SESSION_S", "60")
    env = dict(env, SITE=site, BATCH=str(n))
    out = subprocess.run(
        ["bash", os.path.join(HERE, "pool-sessions", "run.sh"), "container", "gvisor", str(n), conc, "0.5", "1024", session_s, image],
        env=env,
        capture_output=True,
        text=True,
    )
    with open(log, "a") as f:
        f.write(out.stdout + out.stderr)
    if out.returncode != 0:
        print(out.stderr, file=sys.stderr)
        return out.returncode
    runs = [line.split()[-1] for line in out.stdout.splitlines() if line.strip()]
    if not runs:
        print("no run id from `cua sb run`", file=sys.stderr)
        return 1
    jobs: List[dict] = []
    for run in runs:
        deadline = time.time() + int(session_s) * 4 + 1800
        while True:
            r = subprocess.run([env["CUA"], "sb", "runs", run, "--json"], env=env, capture_output=True, text=True)
            try:
                js = json.loads(r.stdout)["jobs"]
            except (ValueError, KeyError):
                js = []
            done = js and all(j.get("state") in ("succeeded", "failed", "error", "cancelled") for j in js)
            if done or time.time() > deadline:
                jobs.extend(js)
                break
            time.sleep(10)
    with open(os.path.join(raw, "jobs.json"), "w") as f:
        json.dump(jobs, f, indent=2)
    return 0


def execute(b: Bench, n: Optional[int], raw: str, env: dict, args) -> int:
    """Runs the bench's script; its native output lands in `raw`."""
    log = os.path.join(raw, "run.log")
    cua = env["CUA"]
    if b.name.startswith("spaces-startup."):
        phase = b.name.split(".", 1)[1]
        e = dict(env, OUT=raw)
        if phase == "vm-density":
            e["ROLLOUT_BENCH"] = need_env("ROLLOUT_BENCH", "rollout_bench")
            e.setdefault("VMD_PROFILE", "rollout")
            e.setdefault("VMD_LABEL", "run")
            if n:
                e["VMD_MAX"] = str(n)
        elif phase == "density":
            if n:
                e["DENSITY_MAX"] = str(n)
        elif n:
            e["N"] = str(n)
            e.setdefault("NCKPT", str(min(n, 10)))
        return run_script(["bash", os.path.join(HERE, "spaces-startup", "bench.sh"), phase], e, raw, log)
    if b.name == "microvm-fork":
        e = dict(env, OUT=raw, N=str(n or b.default_n))
        with Daemon(cua, e, os.path.join(raw, "daemon.log")):
            return run_script(["bash", os.path.join(HERE, "microvm-fork", "bench.sh")], e, raw, log)
    if b.name == "fork-fanout":
        src = find_source(args.src)
        e = dict(env, DISK=need_env("DISK", "disk"))
        if n:
            e["WAVES"] = str(n)
        out = os.path.join(raw, "fork-fanout.json")
        return run_script(["bash", os.path.join(src, "bench", "fork-fanout", "run.sh"), out], e, src, log)
    if b.name == "fork-batch":
        disk = need_env("DISK", "disk")
        if subprocess.run([sys.executable, "-c", "import cua"], env=env, capture_output=True).returncode != 0:
            raise Usage("fork-batch needs " + NEEDS_TEXT["python-sdk"])
        e = dict(env)
        with Daemon(cua, e, os.path.join(raw, "daemon.log")):
            return run_script(
                [
                    sys.executable,
                    os.path.join(HERE, "fork-batch", "bench.py"),
                    "--bake",
                    f"vm:{disk}",
                    "--kind",
                    "vm",
                    "--runtime",
                    "qemu",
                    "--on",
                    "local",
                    "--count",
                    str(n or b.default_n),
                    "--parallel",
                    os.environ.get("PARALLEL", "4"),
                    "--memory-mb",
                    "1536",
                    "--out",
                    os.path.join(raw, "result.json"),
                ],
                e,
                raw,
                log,
            )
    if b.name == "channel":
        bench = need_env("CHANNEL_BENCH", "channel_bench")
        label = "macos-m2max-qemu" if IS_MAC else "ci-ubuntu-24.04-kvm-lane"
        e = dict(env, OUT=raw, BENCH=bench, ITERS=str(n or b.default_n), RECORD="1")
        with Daemon(cua, e, os.path.join(raw, "daemon.log")):
            rc = run_script(["bash", os.path.join(HERE, "channel", "run.sh"), os.path.join(HERE, "channel", "baselines", f"{label}.json")], e, raw, log)
        got = latest(raw, ".jsonl")
        if got:
            shutil.copyfile(got, os.path.join(raw, "channel.jsonl"))
        return rc
    if b.name == "rollouts":
        e = dict(env, OUT=raw, ROLLOUT_BENCH=need_env("ROLLOUT_BENCH", "rollout_bench"), N_ROLL=str(n or b.default_n), BENCH_HOST="none")
        return run_script(["bash", os.path.join(HERE, "rollouts", "guard.sh")], e, raw, log)
    if b.name == "pool-sessions":
        return pool_sessions(b, n or b.default_n, env, raw, log)
    raise Usage(f"no runner for {b.name}")


def build_info(cua: str, version: str) -> dict:
    source = os.environ.get("CUA_BENCH_SOURCE") or f"cua {version} ({os.path.realpath(cua)})"
    return {"cua": version, "source": source}


def reproduce(b: Bench, n: Optional[int]) -> str:
    env = [{"rollout_bench": "ROLLOUT_BENCH=<rollout_bench>", "channel_bench": "CHANNEL_BENCH=<channel_bench>", "disk": "DISK=<linux disk.img>", "site": "SITE=<test site URL>"}.get(x, "") for x in b.needs]
    env = [x for x in env if x]
    cmd = " ".join(env + ["cua", "bench", "run", b.name] + (["--n", str(n)] if n else []))
    return cmd


def finish_run(b: Bench, n: Optional[int], raw: str, out: str, env: dict, before: dict) -> dict:
    c, samples = br.convert(b.name, raw)
    environment = {"kind": b.kind, "image": env.get("IMAGE", "ghcr.io/trycua/linux:24.04-slim"), **before, "network": "n/a"}
    res = br.finish(
        c,
        bench=b.name,
        slug=b.slug,
        measured_at=dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        build=build_info(env["CUA"], before.pop("_version", "")),
        host=host_info(),
        environment={k: v for k, v in environment.items() if not k.startswith("_")},
        method=b.method + (f" n = {c['n']}." if c.get("n") else ""),
        reproduce=reproduce(b, n),
        caveats=b.caveats,
    )
    errs = br.validate(res)
    if errs:
        raise br.NoResult(f"result fails the schema: {errs}")
    br.write(out, res, samples)
    return res


def cmd_run(args) -> int:
    b = BY_NAME.get(args.name)
    if b is None:
        raise Usage(f"no bench {args.name!r}; `cua bench list` shows them")
    if sys.platform not in b.platforms:
        raise Usage(f"{b.name} runs on {b.host}, not on {sys.platform}")
    cua = os.environ.get("CUA") or shutil.which("cua") or ""
    if not cua:
        raise Usage("set CUA to the cua binary")
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out = os.path.abspath(args.out or os.path.join("bench-results", b.name, stamp))
    raw = os.path.join(out, "raw")
    os.makedirs(raw, exist_ok=True)
    env = dict(os.environ, CUA=cua, CUA_BIN=cua, DO_NOT_TRACK="1", CUA_TELEMETRY="0")
    # Never the user's own Spaces home: a home of the run's own unless one is
    # given (a bench restarts or stops the daemon of the home it runs in).
    if b.name != "pool-sessions":
        env["CUA_HOME"] = os.environ.get("CUA_HOME") or os.path.join(out, "cua-home")
        os.makedirs(env["CUA_HOME"], exist_ok=True)
        # This build's daemon, never an installed app's.
        env.setdefault("CUA_SPACES_CLI", "/nonexistent")
    version = cua_version(cua)
    before = {"load_avg_before": round(os.getloadavg()[0], 2), "other_vms": other_vms(), "_version": version}
    t0 = time.time()
    rc = execute(b, args.n, raw, env, args)
    wall = time.time() - t0
    try:
        res = finish_run(b, args.n, raw, out, env, before)
    except br.NoResult as e:
        print(f"no result: {e} (native output in {raw})", file=sys.stderr)
        return rc or 1
    if args.json:
        print(json.dumps({"out": out, "rc": rc, "result": res}, indent=2))
    else:
        pc = res.get("percentiles") or {}
        tail = f" (p95 {pc['p95']})" if "p95" in pc else ""
        print(f"{b.name}: {res['metric']} {res['stat']} = {res['value']} {res['unit']}{tail}, n = {res['n']}, {wall:.0f} s")
        print(f"result: {os.path.join(out, 'result.json')}")
        print(f"samples: {os.path.join(out, res['samples_csv'])}")
        if rc:
            print(f"the bench script exited {rc}; see {os.path.join(raw, 'run.log')}")
    return 1 if rc else 0


# ------------------------------------------------------------------ check


def resolve(path: str) -> str:
    if os.path.exists(path):
        return os.path.abspath(path)
    inside = os.path.join(HERE, path)
    if os.path.exists(inside):
        return inside
    raise Usage(f"no file {path}")


def cmd_check(args) -> int:
    path = resolve(args.result)
    with open(path) as f:
        res = json.load(f)
    if res.get("schema") != br.SCHEMA:
        raise Usage(f"{path} is not a bench-result v1 file")
    b = BY_NAME.get(res.get("bench", ""))
    if b is None:
        raise Usage(f"unknown bench {res.get('bench')!r}")
    if not b.guard:
        raise Usage(f"{b.name} has no perf guard")
    raw = os.path.join(os.path.dirname(path), "raw")
    base = resolve(args.baseline)
    host = args.host or b.host_class.get(sys.platform)
    py = sys.executable

    def native(name: str) -> str:
        p = os.path.join(raw, name)
        if not os.path.exists(p):
            raise Usage(f"the run's native output {p} is missing")
        return p

    if b.name == "spaces-startup.vm-density":
        jl = latest(raw, ".jsonl") if os.path.isdir(raw) else None
        if not jl:
            raise Usage(f"no vm-density output in {raw}")
        cmd = ["bash", os.path.join(HERE, "spaces-startup", "bench.sh"), "guard", jl, base]
    elif b.name == "microvm-fork":
        cmd = [py, os.path.join(HERE, "microvm-fork", "check.py"), native("summary.json"), base]
    elif b.name == "fork-fanout":
        if not host:
            raise Usage("give --host (a host class in the baseline)")
        cmd = [py, os.path.join(HERE, "fork-fanout", "check.py"), native("fork-fanout.json"), base, "--host", host]
    elif b.name == "fork-batch":
        if not host:
            raise Usage("give --host (a profile in the baseline)")
        cmd = [py, os.path.join(HERE, "fork-batch", "check.py"), "--profile", host, "--baseline", base, native("result.json")]
    elif b.name == "channel":
        cmd = [py, os.path.join(HERE, "channel", "check.py"), base, native("channel.jsonl")]
    elif b.name == "rollouts":
        cmd = [py, os.path.join(HERE, "rollouts", "check.py"), native("guard.json"), "--baseline", base]
    else:
        raise Usage(f"{b.name} has no perf guard")
    print(f"$ {' '.join(cmd)}", file=sys.stderr)
    rc = subprocess.run(cmd).returncode
    print(f"{b.name}: {'no regression' if rc == 0 else 'REGRESSION' if rc == 1 else f'guard error ({rc})'} against {base}")
    return rc if rc in (0, 1) else 2


# ---------------------------------------------------------------- convert


def cmd_convert(args) -> int:
    b = BY_NAME.get(args.name)
    if b is None:
        raise Usage(f"no bench {args.name!r}")
    c, samples = br.convert(b.name, args.raw)
    print(json.dumps({"core": c, "samples": len(samples)}, indent=2))
    return 0


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(prog="cua bench", description="Spaces performance benchmarks")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list")
    p.add_argument("--json", action="store_true")
    p = sub.add_parser("run")
    p.add_argument("name")
    p.add_argument("--out")
    p.add_argument("--n", type=int)
    p.add_argument("--src")
    p.add_argument("--json", action="store_true")
    p = sub.add_parser("check")
    p.add_argument("result")
    p.add_argument("--baseline", required=True)
    p.add_argument("--host")
    p.add_argument("--json", action="store_true")
    p = sub.add_parser("convert")
    p.add_argument("name")
    p.add_argument("raw")
    sub.add_parser("manifest")
    args = ap.parse_args(argv)
    try:
        if args.cmd == "list":
            return cmd_list(args)
        if args.cmd == "run":
            return cmd_run(args)
        if args.cmd == "check":
            return cmd_check(args)
        if args.cmd == "convert":
            return cmd_convert(args)
        if args.cmd == "manifest":
            print("\n".join(manifest()))
            return 0
    except Usage as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except br.NoResult as e:
        print(f"no result: {e}", file=sys.stderr)
        return 1
    return 2


if __name__ == "__main__":
    sys.exit(main())
