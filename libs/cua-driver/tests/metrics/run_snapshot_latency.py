import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path

arguments = argparse.ArgumentParser()
arguments.add_argument("evidence_dir", type=Path)
root = arguments.parse_args().evidence_dir.resolve()
binaries = {}
for label in ("baseline", "candidate"):
    for line in (root / f"{label}-build.jsonl").read_text().splitlines():
        event = json.loads(line)
        if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "snapshot_latency" and event.get("executable"):
            binaries[label] = event["executable"]
assert len(binaries) == 2
results = {label: {} for label in binaries}
for repeat in range(3):
    order = ("baseline", "candidate") if repeat % 2 == 0 else ("candidate", "baseline")
    for label in order:
        run = subprocess.run([binaries[label], "snapshot_cache_latency", "--ignored", "--exact", "--nocapture", "--test-threads=1"], capture_output=True, text=True, check=True)
        (root / f"{label}-latency-{repeat}.log").write_text(run.stdout + run.stderr)
        seen = set()
        for line in run.stdout.splitlines():
            start = line.find("{")
            if start < 0:
                continue
            try:
                event = json.loads(line[start:])
            except json.JSONDecodeError:
                continue
            if "workload" in event:
                seen.add(event["workload"])
                results[label].setdefault(event["workload"], []).append(event["ns_per_operation"])
        assert len(seen) == 3
summary = {}
for workload in results["baseline"]:
    row = {}
    for label in binaries:
        runs = results[label][workload]
        assert len(runs) == 3 and all(len(run) == 40 for run in runs)
        row[label] = {"run_medians_ns": [statistics.median(run) for run in runs], "pooled_median_ns": statistics.median(value for run in runs for value in run)}
    row["median_reduction_percent"] = 100 * (1 - row["candidate"]["pooled_median_ns"] / row["baseline"]["pooled_median_ns"])
    summary[workload] = row
report = {"binaries": {label: {"path": path, "sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest()} for label, path in binaries.items()}, "results": summary}
(root / "latency-summary.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(summary, indent=2))
