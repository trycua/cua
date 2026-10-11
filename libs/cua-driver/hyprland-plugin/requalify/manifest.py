#!/usr/bin/env python3
"""Merge requalification results into the compatibility manifest and matrix.

The manifest maps each Hyprland build (an ABI key over the tracked packages'
versions and package hashes) and plugin source tree to a status:

- pass: built against the exact headers, CTest passed, loaded into a headless
  Hyprland session and passed the window-targeted input smoke;
- build-only: built and CTest passed, but no headless session was available;
- fail: any build, test, load or smoke failure.

Statuses are CI requalification, not native certification or a kit.
"""

import argparse
import json
import os
from pathlib import Path
import time

SCHEMA = 1
HISTORY_LIMIT = 200
NOTE = ("CI requalification of the Cua Hyprland plugin per Hyprland build. Not native "
        "certification and not a package: the reviewed kit/profile process stays the release gate.")


def entry_from(record, run_url):
    job, measured = record["job"], record.get("measured", {})
    return {
        "abi_key": job["abi_key"],
        "channels": job["channels"],
        "built_from_channel": job["channel"],
        "hyprland": measured.get("hyprland", {"package_version": job["hyprland"]}),
        "compiler": measured.get("compiler"),
        "tracked_packages": measured.get("tracked_packages", job.get("expected_packages")),
        "runtime": measured.get("runtime"),
        "detection_drift": measured.get("detection_drift", {}),
        "plugin": {"tree": job["tree"], "refs": job["refs"], "driver_version": job.get("driver_version", ""),
                   "module_sha256": record["build"].get("module_sha256"), "options": record["build"].get("options")},
        "checks": {
            "build": summary(record["build"]),
            "ctest": summary(record.get("ctest", {"status": "unavailable", "reason": "not built"})),
            "load": summary(record["load"]),
            "smoke": summary(record["smoke"]),
        },
        "error": record.get("error"),
        "status": record["status"],
        "checked_at": record.get("finished_at"),
        "run_url": run_url,
    }


def summary(check):
    keep = ("status", "step", "reason", "total", "failed", "failures", "checks")
    return {key: check[key] for key in keep if key in check}


def merge(previous, snapshot, records, run_url, now=None):
    entries = {(e["abi_key"], e["plugin"]["tree"]): e for e in (previous or {}).get("entries", [])}
    fresh = [entry_from(record, run_url) for record in records]
    for entry in fresh:
        entries[(entry["abi_key"], entry["plugin"]["tree"])] = entry
    ordered = sorted(entries.values(), key=lambda e: e.get("checked_at") or "", reverse=True)[:HISTORY_LIMIT]
    channels = {name: {"abi_key": info["abi_key"],
                       "hyprland": info["packages"]["hyprland"]["version"],
                       "packages": {k: v["version"] for k, v in info["packages"].items()},
                       **{k: info[k] for k in ("omarchy_plugin", "omarchy_driver") if k in info}}
                for name, info in snapshot["channels"].items()}
    current = {}
    for name, info in channels.items():
        current[name] = {}
        for entry in ordered:
            if entry["abi_key"] == info["abi_key"]:
                for ref in entry["plugin"]["refs"]:
                    current[name].setdefault(ref, {"status": entry["status"], "tree": entry["plugin"]["tree"],
                                                   "checked_at": entry["checked_at"]})
    return {
        "schema": SCHEMA,
        "note": NOTE,
        "generated_at": now or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "run_url": run_url,
        "tracked_packages": snapshot.get("tracked", []),
        "channels": channels,
        "upstream": snapshot.get("upstream"),
        "detection_errors": snapshot.get("errors", {}),
        "current": current,
        "entries": ordered,
    }, fresh


def failures(manifest, fresh):
    """Problems worth a tracking issue: failed builds and Omarchy pins a channel no longer satisfies."""
    problems = []
    for entry in fresh:
        if entry["status"] == "fail":
            failed = [name for name, check in entry["checks"].items() if check.get("status") == "fail"]
            problems.append(f"- **Plugin {', '.join(entry['plugin']['refs'])}** (tree `{entry['plugin']['tree'][:12]}`) on "
                            f"Hyprland `{entry['hyprland'].get('package_version')}` ({', '.join(entry['channels'])}): "
                            f"failed {', '.join(failed) or 'setup'}"
                            + (f" ({entry['error']})" if entry.get("error") else ""))
    for name, channel in sorted(manifest["channels"].items()):
        plugin = channel.get("omarchy_plugin")
        if plugin and not plugin.get("installable", True):
            pins = ", ".join(f"{pkg} pinned {v['pinned']}, channel has {v['channel']}"
                             for pkg, v in plugin["broken_pins"].items())
            problems.append(f"- **{name}**: Omarchy's `cua-hyprland-plugin {plugin['version']}` no longer installs ({pins}). "
                            "A rebuild against the channel's packages is needed.")
    return problems


ICONS = {"pass": "pass", "build-only": "build only", "fail": "**FAIL**", "unavailable": "n/a"}


def matrix(manifest, fresh):
    lines = [f"# Hyprland plugin requalification, {manifest['generated_at']}", "", NOTE, "",
             "| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    by_key = {}
    for entry in manifest["entries"]:
        by_key.setdefault(entry["abi_key"], []).append(entry)
    for name, channel in sorted(manifest["channels"].items()):
        for entry in by_key.get(channel["abi_key"], []) or [None]:
            if entry is None:
                lines.append(f"| {name} | {channel['hyprland']} | `{channel['abi_key']}` | not built | | | | | |")
                continue
            checks = entry["checks"]
            ctest = checks["ctest"]
            ctest_cell = ICONS.get(ctest.get("status"), ctest.get("status"))
            if ctest.get("total"):
                ctest_cell += f" ({ctest['total'] - (ctest.get('failed') or 0)}/{ctest['total']})"
            lines.append("| " + " | ".join([
                name, channel["hyprland"], f"`{channel['abi_key']}`",
                f"{', '.join(entry['plugin']['refs'])} (`{entry['plugin']['tree'][:12]}`)",
                ICONS.get(checks["build"].get("status"), "?"), ctest_cell,
                ICONS.get(checks["load"].get("status"), "?"), ICONS.get(checks["smoke"].get("status"), "?"),
                ICONS.get(entry["status"], entry["status"])]) + " |")
    lines += ["", "## Header and module hashes", "",
              "| Hyprland | Binary SHA-256 | Header inventory SHA-256 | GCC | Module SHA-256 | Plugin tree |",
              "| --- | --- | --- | --- | --- | --- |"]
    for entry in fresh or manifest["entries"][:10]:
        hyprland = entry["hyprland"]
        lines.append(f"| {hyprland.get('package_version')} | `{hyprland.get('sha256', '')}` | "
                     f"`{hyprland.get('headers_sha256', '')}` | {(entry.get('compiler') or {}).get('version', '')} | "
                     f"`{entry['plugin'].get('module_sha256') or ''}` | `{entry['plugin']['tree'][:12]}` |")
    upstream = manifest.get("upstream")
    if upstream:
        state = "ahead of every packaged channel: not yet requalifiable" if upstream.get("ahead_of_packages") else "packaged"
        lines += ["", f"Upstream Hyprland: [{upstream['tag']}]({upstream.get('url')}) ({state})."]
    for name, error in sorted(manifest.get("detection_errors", {}).items()):
        lines.append(f"\nDetection error, {name}: {error}")
    for name, channel in sorted(manifest["channels"].items()):
        plugin = channel.get("omarchy_plugin")
        if plugin:
            state = "installable" if plugin.get("installable", True) else "NOT installable"
            lines.append(f"\nOmarchy `cua-hyprland-plugin {plugin['version']}` on {name}: {state}.")
    return "\n".join(lines) + "\n"


def load_records(directory):
    return [json.loads(path.read_text()) for path in sorted(directory.glob("**/result.json"))
            if path.parent.name != "session"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--results", type=Path, required=True, help="directory of downloaded result artifacts")
    parser.add_argument("--previous", type=Path, help="previous compatibility.json")
    parser.add_argument("--run-url", default="")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    previous = json.loads(args.previous.read_text()) if args.previous and args.previous.is_file() else None
    snapshot = json.loads(args.snapshot.read_text())
    records = load_records(args.results) if args.results.is_dir() else []
    manifest, fresh = merge(previous, snapshot, records, args.run_url)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "compatibility.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    (args.out / "matrix.md").write_text(matrix(manifest, fresh))
    problems = failures(manifest, fresh)
    if problems:
        (args.out / "failures.md").write_text("\n".join(problems) + "\n")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as handle:
            handle.write(f"failed={'true' if problems else 'false'}\nbuilt={len(fresh)}\n")
    print(f"{len(fresh)} new result(s), {len(problems)} problem(s)")


if __name__ == "__main__":
    main()
