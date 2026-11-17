#!/usr/bin/env python3
"""Doctor reports: CI summaries and the image-doctor ledger.

The ledger is what the docs gate trusts: one JSON file per image manifest
digest (the per-platform child a runtime actually pulls), at
``<ledger>/<registry>/<repository>/<hex>.json``, recording each lane's
verdict, the report artifact (an OCI referrer of that digest) and the run.
CI keeps it on the ``image-doctor-ledger`` branch; nothing but the image
workflows writes it.

    doctor_ledger.py summary DIR                 markdown table of lane reports
    doctor_ledger.py record --ledger DIR --repo REPO --digest sha256:... \\
        --report report.json [--lane lane.json] [--report-ref REF] [--run URL]
    doctor_ledger.py regress --ledger DIR --repo REPO --digest sha256:... --lane L --run URL
    doctor_ledger.py status --ledger DIR --repo REPO --digest sha256:... [--lanes a,b]
    doctor_ledger.py merge --from DIR --into DIR     replay entries lane by lane

Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
from typing import Any

SCHEMA_VERSION = 1
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def load(path: str) -> Any:
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def entry_path(ledger: str, repo: str, digest: str) -> str:
    if not DIGEST_RE.match(digest):
        raise ValueError(f"not a sha256 digest: {digest!r}")
    repo = repo.strip("/")
    if not re.match(r"^[a-z0-9.-]+(:[0-9]+)?/[a-z0-9._/-]+$", repo):
        raise ValueError(f"not a repository: {repo!r}")
    return os.path.join(ledger, *repo.split("/"), digest.split(":", 1)[1] + ".json")


def overall(lanes: dict[str, dict[str, Any]]) -> str:
    statuses = [lane.get("status") for lane in lanes.values()]
    if not statuses:
        return "missing"
    if "regressed" in statuses:
        return "regressed"
    if "fail" in statuses:
        return "fail"
    return "pass"


def lane_name(report: dict[str, Any], lane_info: dict[str, Any] | None) -> str:
    if lane_info:
        name = lane_info.get("lane", "")
        if lane_info.get("claim_secrets"):
            name += "-claim-secrets"
        if name:
            return name
    runtime = report.get("environment", {}).get("runtime", "unknown")
    return {"container": "runc", "gvisor": "runsc"}.get(runtime, runtime)


def read_entry(ledger: str, repo: str, digest: str) -> dict[str, Any] | None:
    path = entry_path(ledger, repo, digest)
    if not os.path.exists(path):
        return None
    return load(path)


def write_entry(ledger: str, entry: dict[str, Any]) -> str:
    path = entry_path(ledger, entry["repo"], entry["digest"])
    entry["status"] = overall(entry["lanes"])
    entry["updated"] = now()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(entry, fh, indent=2, sort_keys=True)
        fh.write("\n")
    os.replace(tmp, path)
    return path


def record(ledger: str, repo: str, digest: str, report: dict[str, Any],
           lane_info: dict[str, Any] | None, report_ref: str, run: str) -> dict[str, Any]:
    if report.get("schema_version") != 1:
        raise ValueError("report schema_version must be 1")
    entry = read_entry(ledger, repo, digest) or {
        "schema_version": SCHEMA_VERSION, "repo": repo, "digest": digest, "lanes": {},
    }
    summary = report.get("summary", {})
    image = report.get("image", {})
    env = report.get("environment", {})
    entry["variant"] = image.get("variant") or (lane_info or {}).get("variant", "")
    entry["os"] = image.get("os", "")
    if env.get("arch"):
        entry["arch"] = env["arch"]
    entry["lanes"][lane_name(report, lane_info)] = {
        "status": summary.get("status", "fail") if summary.get("status") != "warn" or not summary.get("strict") else "fail",
        "strict": bool(summary.get("strict")),
        "counts": {k: summary.get(k, 0) for k in ("pass", "warn", "fail", "skip")},
        # Reports from images built before the rename say "guestd".
        "spacesd_version": (report.get("spacesd") or report.get("guestd") or {}).get("version", ""),
        "producer": report.get("producer", ""),
        "manifest_sha256": image.get("manifest_sha256", ""),
        "report": report_ref,
        "workflow_run": run,
        "date": now(),
    }
    write_entry(ledger, entry)
    return entry


def regress(ledger: str, repo: str, digest: str, lane: str, run: str) -> dict[str, Any]:
    entry = read_entry(ledger, repo, digest) or {
        "schema_version": SCHEMA_VERSION, "repo": repo, "digest": digest, "lanes": {},
    }
    previous = entry["lanes"].get(lane, {})
    entry["lanes"][lane] = {**previous, "status": "regressed", "regressed_run": run, "date": now()}
    write_entry(ledger, entry)
    return entry


def merge(src: str, dst: str) -> list[str]:
    """Apply every entry under ``src`` onto ``dst``, lane by lane (``src``
    wins on a lane both hold). Entries are one file per image digest, so two
    jobs that recorded different lanes or images merge without conflict; the
    image workflows use it to replay their entries onto a ledger another job
    pushed first. Returns the merged entry paths (relative to ``dst``)."""
    merged = []
    for root, _dirs, files in os.walk(src):
        if os.sep + ".git" in root + os.sep:
            continue
        for name in sorted(files):
            if not name.endswith(".json"):
                continue
            ours = load(os.path.join(root, name))
            if not isinstance(ours, dict) or "digest" not in ours or "repo" not in ours:
                continue
            theirs = read_entry(dst, ours["repo"], ours["digest"])
            entry = dict(theirs or {}, **{k: v for k, v in ours.items() if k != "lanes"})
            entry["lanes"] = {**((theirs or {}).get("lanes") or {}), **(ours.get("lanes") or {})}
            path = write_entry(dst, entry)
            merged.append(os.path.relpath(path, dst))
    return merged


def status(ledger: str, repo: str, digest: str, lanes: list[str]) -> tuple[str, str]:
    entry = read_entry(ledger, repo, digest)
    if entry is None:
        return "missing", "no ledger entry"
    if lanes:
        missing = [lane for lane in lanes if lane not in entry["lanes"]]
        if missing:
            return "missing", "lanes never run: " + ", ".join(missing)
        picked = {lane: entry["lanes"][lane] for lane in lanes}
    else:
        picked = entry["lanes"]
    verdict = overall(picked)
    return verdict, ", ".join(f"{name}={lane['status']}" for name, lane in sorted(picked.items()))


def summary(directory: str) -> str:
    rows = []
    for root, _dirs, files in sorted(os.walk(directory)):
        if "report.json" not in files:
            continue
        try:
            report = load(os.path.join(root, "report.json"))
        except (OSError, json.JSONDecodeError) as error:
            rows.append((os.path.relpath(root, directory), "?", "?", "unreadable", str(error)))
            continue
        lane_info = None
        if "lane.json" in files:
            lane_info = load(os.path.join(root, "lane.json"))
        s = report.get("summary", {})
        failing = [
            f"`{c['id']}` {c.get('message', '')}"
            for c in report.get("checks", [])
            if c.get("status") == "fail" or (s.get("strict") and c.get("status") == "warn")
        ]
        rows.append((
            lane_name(report, lane_info),
            report.get("environment", {}).get("arch", ""),
            f"{report.get('image', {}).get('name', '')} {report.get('image', {}).get('variant', '')}".strip(),
            f"**{s.get('status', '?')}** {s.get('pass', 0)}/{s.get('warn', 0)}/{s.get('fail', 0)}/{s.get('skip', 0)}",
            "<br>".join(failing[:8]).replace("|", "\\|") or "",
        ))
    out = ["### Image doctor", "", "| lane | arch | image | status (pass/warn/fail/skip) | failing |",
           "|---|---|---|---|---|"]
    out += [f"| {a} | {b} | {c} | {d} | {e} |" for a, b, c, d, e in rows]
    if not rows:
        out.append("| (no reports) | | | | |")
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("summary")
    s.add_argument("dir")
    r = sub.add_parser("record")
    for p in (r,):
        p.add_argument("--ledger", required=True)
        p.add_argument("--repo", required=True)
        p.add_argument("--digest", required=True)
    r.add_argument("--report", required=True)
    r.add_argument("--lane")
    r.add_argument("--report-ref", default="")
    r.add_argument("--run", default="")
    g = sub.add_parser("regress")
    g.add_argument("--ledger", required=True)
    g.add_argument("--repo", required=True)
    g.add_argument("--digest", required=True)
    g.add_argument("--lane", required=True)
    g.add_argument("--run", default="")
    m = sub.add_parser("merge")
    m.add_argument("--from", dest="src", required=True)
    m.add_argument("--into", dest="dst", required=True)
    t = sub.add_parser("status")
    t.add_argument("--ledger", required=True)
    t.add_argument("--repo", required=True)
    t.add_argument("--digest", required=True)
    t.add_argument("--lanes", default="")
    args = parser.parse_args(argv)
    if args.cmd == "summary":
        sys.stdout.write(summary(args.dir))
    elif args.cmd == "record":
        entry = record(args.ledger, args.repo, args.digest, load(args.report),
                       load(args.lane) if args.lane else None, args.report_ref, args.run)
        print(json.dumps({"path": entry_path(args.ledger, args.repo, args.digest), "status": entry["status"]}))
    elif args.cmd == "merge":
        for path in merge(args.src, args.dst):
            print(path)
    elif args.cmd == "regress":
        entry = regress(args.ledger, args.repo, args.digest, args.lane, args.run)
        print(json.dumps({"status": entry["status"]}))
    else:
        verdict, detail = status(args.ledger, args.repo, args.digest,
                                 [x for x in args.lanes.split(",") if x])
        print(f"{verdict}: {detail}")
        return 0 if verdict == "pass" else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
