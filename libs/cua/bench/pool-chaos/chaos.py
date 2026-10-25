#!/usr/bin/env python3
"""Seeded chaos runs against real machines pooled through one account.

    python3 chaos.py --nodes nodes.json --seed 7 --minutes 20 --out results/

A workload keeps runs going on all the machines (`cua sb run --on auto`),
plus interactive Spaces (`cua spaces create --on auto`) and runs that must
stay on one machine (`cua sb run`, no `--on`), while a scheduler seeded with
--seed injects faults and heals them: kill -9 of a daemon, of a host's
cua-spacesd, of the relay; partitions (both ways or one way), packet loss
and latency; clock jumps; a full disk; memory pressure; a whole machine
paused (SIGSTOP); a machine away past its lease that comes back with stale
state; signing out mid-run; Provide Spaces turned off mid-job; the network
path rebinding; a spot interruption. Every operation and fault is a line of
history.jsonl. At the end the relay's pool log, its audit and the machines'
job ledgers are collected, and check.py checks the history against the
invariants (one result per job, no lost job, one writer per epoch, only
allowed machines, a complete placement record, local runs stayed local).

nodes.json (see nodes.example.json) says how to reach each machine
(`ssh`: an argv prefix, or null for this machine), its OS, its work dir
(`w`: where node-up.sh put its home, daemon pid and logs), its role
(`provider` or `client`), whether it submits work, and which faults it
allows (a machine without root gets only process-level faults; the user's
own machine only faults of the processes this harness started).
"""
import argparse
import json
import os
import random
import shlex
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def now():
    return round(time.time(), 3)


class History:
    def __init__(self, path):
        self.f = open(path, "a", buffering=1)
        self.lock = threading.Lock()

    def add(self, kind, **kw):
        rec = {"t": now(), "kind": kind, **kw}
        with self.lock:
            self.f.write(json.dumps(rec) + "\n")
        return rec


class Node:
    def __init__(self, d):
        self.__dict__.update(d)
        self.faults = d.get("faults", [])

    def sh(self, cmd, timeout=120, check=False):
        """Runs `cmd` (bash) on the node with its cua environment."""
        full = f"{self.prelude} {cmd}" if getattr(self, "prelude", "") else cmd
        argv = (self.ssh + [full]) if self.ssh else ["bash", "-lc", full]
        try:
            p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return 124, "", "timeout"
        if check and p.returncode:
            raise RuntimeError(f"{self.name}: {cmd}: {p.stderr.strip()[-400:]}")
        return p.returncode, p.stdout, p.stderr


def cua(node, args, timeout=120):
    return node.sh(f"cua {args}", timeout=timeout)


# ---- faults -----------------------------------------------------------------

DAEMON = '"$(command -v cua) daemon start"'
# The daemon of this machine's home, whatever started it (the CLI, the
# host's cua-spacesd, this harness): its pid from `$CUA_HOME/daemon.json`.
DAEMON_PID = """$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$CUA_HOME/daemon.json" 2>/dev/null)"""


def daemon_up(n):
    # The host's cua-spacesd also starts the daemon on its next request.
    n.sh(f"kill -0 {DAEMON_PID} 2>/dev/null || (cd {n.w} && (nohup \"$(command -v cua)\" daemon start --foreground >> {n.w}/daemon.log 2>&1 &))")


def fault(n, kind, rng):
    """Injects `kind` on `n`; returns (detail, heal callable)."""
    w = n.w
    relay_ip = n.relay_ip
    if kind == "kill_daemon":
        n.sh(f"kill -9 {DAEMON_PID}; true")
        return "kill -9 the cua daemon", lambda: daemon_up(n)
    if kind == "kill_spacesd":
        n.sh(f"pkill -9 -f 'cua-spacesd.*{w}' ; true")
        return "kill -9 the host's cua-spacesd", lambda: n.sh(f"bash {w}/host-again.sh")
    if kind == "stop":
        pids = f"{DAEMON_PID} $(pgrep -f 'cua-spacesd.*{w}' | tr '\\n' ' ')"
        n.sh(f"kill -STOP {pids}")
        return "SIGSTOP the whole stack (asleep)", lambda: n.sh(f"kill -CONT {pids}")
    if kind == "stale":
        pid = n.sh(f"echo {DAEMON_PID}")[1].strip()
        n.sh(f"kill -STOP {pid}")
        return "away past its lease, back with stale state", lambda: n.sh(f"kill -CONT {pid}")
    if kind == "signout":
        n.sh("cua auth logout")
        return "signed out mid-run", lambda: n.sh("cua auth login --no-browser")
    if kind == "toggle":
        n.sh("cua host config --provide-spaces off")
        return "Provide Spaces off mid-job", lambda: n.sh("cua host config --provide-spaces on")
    if kind == "tunnel":
        # The path to the relay drops and comes back on a new port.
        n.sh(f"pkill -STOP -f 'tunnel.sh' ; pkill -f 'ssh .*-L 127.0.0.1:8080' ; true")
        return "path to the relay lost, back on a new port", lambda: n.sh("pkill -CONT -f 'tunnel.sh'; true")
    if kind == "partition":
        oneway = rng.random() < 0.5
        n.sh(f"sudo iptables -I OUTPUT -d {relay_ip} -j DROP" + ("" if oneway else f" && sudo iptables -I INPUT -s {relay_ip} -j DROP"))
        def heal():
            n.sh(f"sudo iptables -D OUTPUT -d {relay_ip} -j DROP; sudo iptables -D INPUT -s {relay_ip} -j DROP; true")
        return f"partitioned from the relay ({'one way' if oneway else 'both ways'})", heal
    if kind == "netem":
        loss, delay, jit = rng.choice([5, 10, 20]), rng.choice([50, 150, 300]), rng.choice([20, 80])
        n.sh(f"sudo tc qdisc add dev {n.nic} root netem loss {loss}% delay {delay}ms {jit}ms")
        return f"loss {loss}% delay {delay}±{jit} ms", lambda: n.sh(f"sudo tc qdisc del dev {n.nic} root; true")
    if kind == "clock":
        jump = rng.choice([-1, 1]) * rng.choice([90, 600, 3600])
        n.sh(f"sudo timedatectl set-ntp false; sudo date -s \"$(date -d '{jump} sec')\" >/dev/null")
        return f"clock jumped {jump:+d} s", lambda: n.sh("sudo timedatectl set-ntp true; sudo chronyc -a makestep >/dev/null 2>&1; sudo date -s \"$(curl -sI https://aws.amazon.com | grep -i '^date:' | cut -d' ' -f2-)\" >/dev/null; true")
    if kind == "disk":
        n.sh(f"fallocate -l $(( $(df --output=avail -B1 {w} | tail -1) - 50*1024*1024 )) {w}/fill.bin")
        return "disk full", lambda: n.sh(f"rm -f {w}/fill.bin")
    if kind == "mem":
        n.sh(f"(nohup stress-ng --vm 2 --vm-bytes 85% --timeout 600s > /dev/null 2>&1 &)")
        return "memory pressure", lambda: n.sh("pkill -f stress-ng; true")
    if kind == "kill_relay":
        n.sh("sudo systemctl kill -s KILL sp-relay")
        return "kill -9 the relay (restarts itself)", lambda: None
    raise ValueError(kind)


# ---- workload -------------------------------------------------------------------

def submitter(n, hist, stop, rng, allowed):
    k = 0
    while not stop.is_set():
        k += 1
        local = "local_runs" in n.__dict__ and n.local_runs and rng.random() < 0.25
        count = rng.randint(5, 20)
        sleep = rng.randint(3, 25)
        run = f"{'l' if local else 'p'}-{n.seed:x}{k:04d}{rng.randint(0, 9999):04d}"
        on = "" if local else "--on auto"
        # A secret only the run knows, in its environment, its command and
        # its output: the relay must never see it (check.py, invariant 8).
        secret = f"cuasecret{rng.getrandbits(64):016x}"
        cmd = f"sh -c 'sleep {sleep}; echo {run} $(cat /etc/hostname 2>/dev/null || hostname) {secret} $CHAOS_TOKEN'"
        rc, out, err = cua(n, f"sb run {on} --id {run} --count {count} --env CHAOS_TOKEN={secret}x --detach -- {cmd}", timeout=180)
        hist.add("submit", node=n.name, run=run, count=count, local=local, rc=rc, secret=secret, err=err.strip()[-300:])
        time.sleep(rng.uniform(15, 45))


def spaces(n, hist, stop, rng):
    while not stop.is_set():
        rc, out, err = cua(n, "spaces create linux --on auto --json", timeout=900)
        rec = {"rc": rc, "err": err.strip()[-300:]}
        try:
            v = json.loads(out)
            v = (v.get("spaces") or [v])[0]
            rec.update(id=v.get("id"), host=v.get("host"), host_name=v.get("host_name"))
        except Exception:
            pass
        hist.add("space_create", node=n.name, **rec)
        time.sleep(rng.uniform(10, 40))
        if rec.get("id"):
            rc, _, err = cua(n, f"spaces delete {shlex.quote(rec['id'])} --force", timeout=600)
            hist.add("space_delete", node=n.name, id=rec["id"], rc=rc, err=err.strip()[-300:])
        time.sleep(rng.uniform(5, 30))


# ---- collection ------------------------------------------------------------------

def wait_done(sub, nodes, hist, minutes):
    """Waits until every acknowledged run ended (bounded)."""
    deadline = time.time() + minutes * 60
    runs = [json.loads(l) for l in open(hist.f.name)]
    acked = [r for r in runs if r["kind"] == "submit" and r["rc"] == 0]
    pending = {r["run"] for r in acked}
    # A run on one machine is asked there.
    where = {r["run"]: next(n for n in nodes if n.name == r["node"]) if r["local"] else sub for r in acked}
    while pending and time.time() < deadline:
        for run in sorted(pending):
            rc, out, _ = cua(where[run], f"sb runs {run} --json", timeout=120)
            try:
                jobs = json.loads(out)["jobs"]
            except Exception:
                continue
            if jobs and all(j["state"] in ("succeeded", "failed", "error", "canceled") for j in jobs):
                pending.discard(run)
        time.sleep(10)
    return pending


def collect(cfg, nodes, sub, hist, out):
    runs = [json.loads(l) for l in open(hist.f.name) if '"submit"' in l]
    results = {}
    for r in runs:
        if r["rc"] != 0:
            continue
        who = sub if not r["local"] else next(n for n in nodes if n.name == r["node"])
        # Each job with its output (the result itself).
        rc, o, _ = cua(who, f"sb runs {r['run']} --json --output", timeout=300)
        try:
            results[r["run"]] = json.loads(o)["jobs"]
        except Exception:
            results[r["run"]] = None
    json.dump(results, open(os.path.join(out, "results.json"), "w"))
    relay = Node(cfg["relay"])
    _, logs, _ = relay.sh("cat ~/relay/state.pool/*/log.jsonl 2>/dev/null", timeout=300)
    open(os.path.join(out, "relay-log.jsonl"), "w").write(logs)
    _, lease, _ = relay.sh("cat ~/relay/state.pool/*/lease.json 2>/dev/null")
    open(os.path.join(out, "relay-lease.json"), "w").write(lease)
    _, dev, _ = relay.sh("cat ~/relay/state.devices.json 2>/dev/null", timeout=120)
    open(os.path.join(out, "relay-devices.json"), "w").write(dev)
    # Every snapshot the relay holds, and a search of everything it keeps
    # (state, log, snapshots, its journal) for the runs' secrets.
    _, snaps, _ = relay.sh("for f in ~/relay/state.pool/*/snapshot.bin; do [ -f \"$f\" ] && cat \"$f\" && echo; done", timeout=120)
    open(os.path.join(out, "relay-snapshots.txt"), "w").write(snaps)
    secrets = sorted({json.loads(l)["secret"] for l in open(hist.f.name) if '"submit"' in l and '"secret"' in l})
    if secrets:
        es = " ".join(f"-e {x}" for x in secrets)
        _, found, _ = relay.sh(
            f"(grep -rlF {es} ~/relay 2>/dev/null; sudo journalctl -u sp-relay --no-pager 2>/dev/null | grep -cF {es} | grep -v '^0$' | sed 's/^/journal matches: /') || true",
            timeout=300)
        open(os.path.join(out, "relay-grep.txt"), "w").write(found)
    for n in nodes:
        _, led, _ = n.sh(f"cat {n.w}/home/host/jobs/ledger.jsonl 2>/dev/null", timeout=300)
        open(os.path.join(out, f"ledger-{n.name}.jsonl"), "w").write(led)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--nodes", required=True)
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("--minutes", type=float, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("--settle", type=float, default=20, help="minutes to wait for runs to end")
    ap.add_argument("--faults", default="",
                    help="only these faults (comma-separated), e.g. kill_relay,clock,mem")
    ap.add_argument("--collect-only", action="store_true",
                    help="no workload or faults: wait for the recorded runs, collect and check")
    a = ap.parse_args()
    cfg = json.load(open(a.nodes))
    out = os.path.join(a.out, f"seed-{a.seed}")
    os.makedirs(out, exist_ok=True)
    hist = History(os.path.join(out, "history.jsonl"))
    rng = random.Random(a.seed)
    nodes = []
    for d in cfg["nodes"]:
        d = {"relay_ip": cfg["relay"]["ip"], **d, "seed": a.seed}
        nodes.append(Node(d))
    allowed = [n.name for n in nodes if n.role == "provider"]
    if a.collect_only:
        sub = next(n for n in nodes if getattr(n, "submits", False) and not getattr(n, "local_runs", False))
        pending = wait_done(sub, nodes, hist, a.settle)
        hist.add("settled", pending=sorted(pending))
        collect(cfg, nodes, sub, hist, out)
        hist.add("collected")
        sys.exit(subprocess.call([sys.executable, os.path.join(HERE, "check.py"), out]))
    hist.add("start", seed=a.seed, minutes=a.minutes, nodes=[n.name for n in nodes], allowed=allowed)
    stop = threading.Event()
    threads = []
    for n in nodes:
        if getattr(n, "submits", False):
            threads.append(threading.Thread(target=submitter, args=(n, hist, stop, random.Random(rng.random()), allowed), daemon=True))
        if getattr(n, "spaces", False):
            threads.append(threading.Thread(target=spaces, args=(n, hist, stop, random.Random(rng.random())), daemon=True))
    for t in threads:
        t.start()
    end = time.time() + a.minutes * 60
    relay = Node({**cfg["relay"], "faults": ["kill_relay"], "w": "~/relay", "relay_ip": cfg["relay"]["ip"]})
    candidates = [(n, f) for n in nodes for f in n.faults] + [(relay, "kill_relay")]
    if a.faults:
        only = set(a.faults.split(","))
        candidates = [(n, f) for n, f in candidates if f in only]
    time.sleep(30)
    while time.time() < end - 60:
        # One fault at a time, sometimes two overlapping on different machines.
        picks = [rng.choice(candidates)]
        if rng.random() < 0.3:
            other = rng.choice(candidates)
            if other[0] is not picks[0][0]:
                picks.append(other)
        heals = []
        for n, f in picks:
            try:
                detail, heal = fault(n, f, rng)
                hist.add("fault", node=n.name, fault=f, detail=detail)
                heals.append((n, f, heal))
            except Exception as e:  # a fault that could not be injected
                hist.add("fault_error", node=n.name, fault=f, error=str(e)[-300:])
        dur = rng.uniform(15, 90)
        time.sleep(dur)
        for n, f, heal in heals:
            try:
                if heal:
                    heal()
                hist.add("heal", node=n.name, fault=f, after_s=round(dur))
            except Exception as e:
                hist.add("heal_error", node=n.name, fault=f, error=str(e)[-300:])
        time.sleep(rng.uniform(20, 60))
    stop.set()
    hist.add("workload_stopped")
    sub = next(n for n in nodes if getattr(n, "submits", False) and not getattr(n, "local_runs", False))
    pending = wait_done(sub, nodes, hist, a.settle)
    hist.add("settled", pending=sorted(pending))
    collect(cfg, nodes, sub, hist, out)
    hist.add("collected")
    rc = subprocess.call([sys.executable, os.path.join(HERE, "check.py"), out])
    sys.exit(rc)


if __name__ == "__main__":
    main()
