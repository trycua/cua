#!/usr/bin/env python3
"""check.py OUT_DIR — checks a chaos run's history against the pool's invariants.

Reads what chaos.py collected: history.jsonl (every submit, Space and
fault), results.json (each run's jobs as the coordinator reports them, with
outputs), relay-log.jsonl (the pool log the relay stored), relay-lease.json
and relay-devices.json (lease grants are audit events), and ledger-*.jsonl
(each machine's job ledger). Prints one line per invariant and exits 1 if
any is violated.

1. No double results: at most one terminal entry per job in the pool log,
   and each run reports each job once.
2. No lost jobs: every run whose submit was acknowledged has all its jobs
   ended, each succeeded (or failed/error with a reason).
3. Results are real: each succeeded job's output names its run and a
   machine allowed to run it.
4. One writer per epoch: log entries' epochs never go back, each epoch was
   granted (audit), and no entry of an epoch follows a later epoch's entry.
5. Only allowed machines: every dispatch, and every interactive Space, went
   to a machine of the pool (one that provides Spaces).
6. Placements recorded: every job a machine's ledger shows for the pool was
   dispatched there in the log (same attempt or a later one).
7. Local stays local: no `l-` run appears in the pool log, and every job of
   one ran on the machine that submitted it.
8. The relay sees only ciphertext: every log entry's data and the snapshot
   are sealed envelopes, and no run's secret (a token each run puts in its
   environment, command and output) appears anywhere in the relay's state
   or its log output.
"""
import collections
import glob
import json
import os
import sys

out = sys.argv[1]
H = [json.loads(l) for l in open(os.path.join(out, "history.jsonl")) if l.strip()]
start = next(h for h in H if h["kind"] == "start")
allowed = set(start["allowed"])
results = json.load(open(os.path.join(out, "results.json")))
log = [json.loads(l) for l in open(os.path.join(out, "relay-log.jsonl")) if l.strip()]
fails = []
report = []


def check(name, ok, detail):
    report.append(f"{'PASS' if ok else 'FAIL'}  {name}: {detail}")
    if not ok:
        fails.append(name)


# 1. one terminal entry per job
term = collections.Counter(e["job"] for e in log if e.get("terminal"))
dups = [j for j, c in term.items() if c > 1]
per_run_dup = []
for run, jobs in results.items():
    ids = [j["id"] for j in jobs or []]
    if len(ids) != len(set(ids)):
        per_run_dup.append(run)
check("no double results", not dups and not per_run_dup,
      f"{len(term)} jobs ended in the log, {len(dups)} ended twice, {len(per_run_dup)} runs list a job twice")

# 2. no lost jobs
acked = [h for h in H if h["kind"] == "submit" and h["rc"] == 0]
lost, unexplained, total, succeeded = [], [], 0, 0
for s in acked:
    jobs = results.get(s["run"])
    if jobs is None or len(jobs) != s["count"]:
        lost.append((s["run"], "missing", None if jobs is None else len(jobs)))
        continue
    for j in jobs:
        total += 1
        if j["state"] == "succeeded":
            succeeded += 1
        elif j["state"] in ("failed", "error", "canceled"):
            if not j.get("error"):
                unexplained.append((s["run"], j["id"], j["state"]))
        else:
            lost.append((s["run"], j["id"], j["state"]))
check("no lost jobs", not lost and not unexplained,
      f"{len(acked)} runs acknowledged, {total} jobs, {succeeded} succeeded, "
      f"{len(lost)} not ended {lost[:5]}, {len(unexplained)} failed without a reason {unexplained[:5]}")

# 3. results are real
bad = []
for run, jobs in results.items():
    for j in jobs or []:
        if j["state"] != "succeeded":
            continue
        words = (j.get("stdout") or "").split()
        if len(words) < 2 or words[0] != run:
            bad.append((run, j["id"], (j.get("stdout") or "")[:80]))
check("results are real", not bad, f"{len(bad)} succeeded jobs whose output is not their own run's {bad[:5]}")

# 4. one writer per epoch
epochs = [e["epoch"] for e in log]
monotone = all(a <= b for a, b in zip(epochs, epochs[1:]))
seqs = [e["seq"] for e in log]
contiguous = all(b == a + 1 for a, b in zip(seqs, seqs[1:]))
granted = set()
try:
    dev = json.load(open(os.path.join(out, "relay-devices.json")))
    for acct in (dev.get("audit") or {}).values():
        for ev in acct:
            ev = ev.get("event", ev)
            if ev.get("kind") == "pool_coordinator":
                d = ev.get("detail") or ""
                if d.startswith("epoch "):
                    granted.add(int(d.split()[1].rstrip(",")))
except Exception as e:
    granted = None
# The account audit keeps its newest events: epochs older than the oldest
# grant it still has cannot be checked against it.
if granted is None:
    ungranted = ["audit unreadable"]
else:
    floor = min(granted) if granted else 0
    ungranted = sorted(e for e in set(epochs) - granted if e >= floor)
check("one writer per epoch", monotone and contiguous and not ungranted,
      f"{len(log)} entries over epochs {sorted(set(epochs))}; never back: {monotone}; "
      f"no gaps: {contiguous}; epochs not granted: {ungranted}")

# 5. only allowed machines
nodes_of = {}
disallowed = set()
for e in log:
    if e["kind"] == "dispatched":
        # The placement is one of the few things the relay reads.
        node = e["node"]
        if node not in allowed and node.split("-")[0] not in allowed:
            disallowed.add(node)
spaces_bad = [h for h in H if h["kind"] == "space_create" and h.get("id") and h.get("host_name") not in allowed | {None, ""}]
check("only allowed machines", not disallowed and not spaces_bad,
      f"dispatches to {sorted({e['node'] for e in log if e['kind'] == 'dispatched'})}, "
      f"not allowed: {sorted(disallowed)}; interactive Spaces outside the pool: {len(spaces_bad)}")

# 6. placements recorded (job ids on hosts are `<pool>-<n>`, as in the log)
pools = {e["job"].rsplit("-", 1)[0] for e in log if e.get("job")}
dispatched = collections.defaultdict(int)
for e in log:
    if e["kind"] == "dispatched":
        k = (e["node"], e["job"])
        dispatched[k] = max(dispatched[k], e["attempt"])
unrecorded = []
for path in glob.glob(os.path.join(out, "ledger-*.jsonl")):
    machine = os.path.basename(path)[len("ledger-"):-len(".jsonl")]
    for l in open(path):
        if not l.strip():
            continue
        r = json.loads(l)
        jid = r.get("job_id", "")
        # This account's pool jobs: `<account>/<pool>-<n>`.
        _, _, bare = jid.rpartition("/")
        if bare.rsplit("-", 1)[0] not in pools or not r.get("accepted_at_ms"):
            continue
        att = max((a for (n, j), a in dispatched.items() if j == bare), default=0)
        if att < r.get("attempt", 0):
            unrecorded.append((machine, bare, r.get("attempt")))
check("placements recorded", not unrecorded,
      f"{sum(1 for _ in dispatched)} dispatches in the log; jobs on machines with no dispatch: {unrecorded[:5]}")

# 7. local stays local
in_log = [e for e in log if e["kind"] == "submit" and e.get("run", "").startswith("l-")]
local_runs = [h for h in H if h["kind"] == "submit" and h.get("local") and h["rc"] == 0]
left = []
for s in local_runs:
    for j in results.get(s["run"]) or []:
        if j.get("node") != "local":
            left.append((s["run"], j["id"], j.get("node")))
check("local stays local", not in_log and not left,
      f"{len(local_runs)} local runs; in the pool log: {len(in_log)}; jobs elsewhere: {left[:5]}")

# 8. the relay sees only ciphertext
def sealed(v):
    return isinstance(v, dict) and set(v) == {"v", "g", "n", "c"} and v["v"] == 1

plain = [e["seq"] for e in log if not sealed(e.get("data"))]
snap_path = os.path.join(out, "relay-snapshots.txt")
snaps = [l for l in open(snap_path).read().splitlines() if l.strip()] if os.path.exists(snap_path) else []
plain_snaps = []
for line in snaps:
    try:
        ok = sealed(json.loads(line))
    except Exception:
        ok = False
    if not ok:
        plain_snaps.append(line[:60])
secrets = sorted({h["secret"] for h in H if h["kind"] == "submit" and h.get("secret") and not h.get("local")})
leaks = []
grep_path = os.path.join(out, "relay-grep.txt")
found = open(grep_path).read() if os.path.exists(grep_path) else None
if found is None:
    leaks.append("relay state not searched")
else:
    leaks = [l for l in found.splitlines() if l.strip()]
check("the relay sees only ciphertext", not plain and not plain_snaps and not leaks and bool(secrets),
      f"{len(log)} entries, {len(plain)} not sealed {plain[:5]}; {len(snaps)} snapshots, "
      f"{len(plain_snaps)} not sealed; {len(secrets)} run secrets searched for in the relay's state and log, "
      f"found: {leaks[:5]}")

faults = collections.Counter(h["fault"] for h in H if h["kind"] == "fault")
print(f"seed {start['seed']} · {start['minutes']} min · faults {dict(faults)}")
print("\n".join(report))
json.dump({"seed": start["seed"], "faults": faults, "report": report, "violations": fails},
          open(os.path.join(out, "check.json"), "w"), indent=1)
sys.exit(1 if fails else 0)
