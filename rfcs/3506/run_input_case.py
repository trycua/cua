#!/usr/bin/python
"""One admitted operation per invocation, with a durable no-replay ledger.

Run only with the two synthetic fixture sources recording. Keys are single,
unmodified letters; pointer input is deliberately not implemented here.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

import dbus
from dbus.mainloop.glib import DBusGMainLoop
from activation_probe import observe

BINARY = Path("/home/netbos/.hermes/cache/scratch/cua3506-driver/source/libs/cua-driver/rust/target/debug/examples/rfc3506")
parser = argparse.ArgumentParser()
parser.add_argument("--directory", type=Path, required=True)
parser.add_argument("--seq", type=int, required=True)
parser.add_argument("--case", choices=("A", "B", "inactive", "precheck_takeover", "postcheck_takeover",
                                       "drop_ack", "expired", "closed", "background"), required=True)
args = parser.parse_args()
DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
fixture_owner = json.loads((args.directory / "fixture-owner.json").read_text())
fixture = dbus.Interface(bus.get_object(fixture_owner["owner"], "/Fixture"), "org.cua.ProofFixture")
pid = fixture_owner["pid"]
ledger = args.directory / ("operation-" + str(args.seq) + ".jsonl")
# Exclusive creation is the parent-owned replay barrier across process restarts.
log = ledger.open("x", buffering=1)
log.write(json.dumps({"seq": args.seq, "case": args.case, "status": "consumed_before_submission",
                      "time_ns": time.monotonic_ns(), "no_retry": True}) + "\n")
env = os.environ.copy()
for name, leaf in (("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"),
                   ("XDG_DATA_HOME", "data")):
    env[name] = str(args.directory / leaf)
env.pop("LIBEI_SOCKET", None)  # Require the real authorized portal, no socket bypass.
identity = json.loads(subprocess.check_output([str(BINARY), "--identity"], env=env, text=True))
targets = {}
for label in ("A", "B"):
    matches = [w for w in identity["snapshot"]["windows"] if w["pid"] == pid and
               w["title"] == "Cua 3506 Synthetic " + label]
    if len(matches) != 1:
        raise RuntimeError("fixture_selection_not_exact")
    targets[label] = matches[0]
label = "B" if args.case == "B" else "A"
target = targets[label]
generation = identity["snapshot"]["generation"]
before = json.loads(str(fixture.GetState()))
prefix = args.directory / ("gate-" + str(args.seq))
postfix = args.directory / ("postgate-" + str(args.seq))
for path in (prefix, postfix):
    if Path(str(path) + ".reached").exists() or Path(str(path) + ".release").exists():
        raise RuntimeError("stale_gate_files")
expected = {"owner": identity["owner"], "pid": pid, "token": target["token"],
            "generation": generation, "internal_id": target["internal_id"],
            "deadline_ns": time.monotonic_ns() + 60_000_000_000,
            "op_seq": args.seq, "key": "a", "drop_ack": args.case == "drop_ack",
            "precheck_gate": str(prefix),
            "postcheck_gate": str(postfix) if args.case == "postcheck_takeover" else None}
mode = "--execute"
if args.case == "expired":
    expected["deadline_ns"] = time.monotonic_ns() - 1
if args.case == "closed":
    mode = "--execute-closed"
if args.case == "background":
    mode = "--background-refusal"
process = subprocess.Popen([str(BINARY), mode], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, env=env, text=True)
assert process.stdin is not None
process.stdin.write(json.dumps(expected))
process.stdin.close()
process.stdin = None
observations = []
try:
    end = time.monotonic() + 40
    while not Path(str(prefix) + ".reached").exists() and process.poll() is None:
        if time.monotonic() >= end:
            raise TimeoutError("readiness_or_admission_not_reached")
        time.sleep(0.01)
    if process.poll() is None:
        # Portal readiness/consent completes before requesting activation.
        wanted = "B" if args.case in ("inactive", "precheck_takeover") else label
        observations.append(observe(bus, pid, targets[wanted]["token"], generation))
        Path(str(prefix) + ".release").touch(exist_ok=False)
        if args.case == "postcheck_takeover":
            while not Path(str(postfix) + ".reached").exists() and process.poll() is None:
                if time.monotonic() >= end:
                    raise TimeoutError("postcheck_gate_not_reached")
                time.sleep(0.001)
            if process.poll() is None:
                observations.append(observe(bus, pid, targets["B"]["token"], generation))
                Path(str(postfix) + ".release").touch(exist_ok=False)
    stdout, stderr = process.communicate(timeout=30)
except BaseException as error:
    process.kill()
    stdout, stderr = process.communicate()
    log.write(json.dumps({"status": "outer_unknown_never_replay", "error": str(error),
                          "stdout": stdout, "stderr": stderr}) + "\n")
    raise
report = None
for line in stdout.splitlines():
    try:
        candidate = json.loads(line)
        if "operation" in candidate:
            report = candidate
    except (ValueError, TypeError):
        pass
# Receipt may lag client flush. This wait is observation, not a safety threshold.
end = time.monotonic() + 2
while True:
    after = json.loads(str(fixture.GetState()))
    if any(after["windows"][k]["presses"] > before["windows"][k]["presses"] for k in ("A", "B")):
        if all(not after["windows"][k]["held_keys"] for k in ("A", "B")):
            break
    if time.monotonic() >= end:
        break
    time.sleep(0.01)
deltas = {k: {c: after["windows"][k][c] - before["windows"][k][c]
               for c in ("presses", "releases", "clicks", "button_releases")} for k in ("A", "B")}
result = {"case": args.case, "seq": args.seq, "expected": expected,
          "observations": observations, "report": report, "exit_code": process.returncode,
          "stdout": stdout, "stderr": stderr, "before": before, "after": after,
          "deltas": deltas, "completed_ns": time.monotonic_ns(), "no_retry": True}
log.write(json.dumps(result) + "\n")
log.close()
print(json.dumps({"case": args.case, "seq": args.seq, "report": report,
                  "deltas": deltas, "ledger": str(ledger)}), flush=True)
