#!/usr/bin/env python3
"""Generate deterministic, semantically identical CDB-S02 launch descriptors."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

BUNDLE = Path(__file__).resolve().parent.parent
PLATFORM = BUNDLE / "platform"


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def semantics() -> dict:
    return {
        "task_id": "cua.shared.cdb-s02.conflicting-change-replan",
        "task_version": "1.0.0",
        "brief": "brief.md",
        "brief_sha256": digest(BUNDLE / "brief.md"),
        "manifest": "task.cuabench.json",
        "variant": "fixture.lighthouse",
        "workspace_layout": {
            "mail": "mail/messages.json",
            "chat": "chat",
            "calendar": "calendar/events.json",
            "workbook": "planning/lighthouse-replan.ods",
            "facts": "cdb-s02.workspace.json",
        },
        "reset": {
            "setup": ["${python}", "reset/setup.py", "--workspace", "${workspace}"],
            "verify": ["${python}", "reset/verify.py", "--workspace", "${workspace}"],
        },
        "evaluate": [
            "${python}", "evaluator/evaluate.py", "--workspace", "${workspace}",
            "--artifacts", "${artifacts}", "--result", "${result}",
            "--agent-exit-code", "${agent_exit_code}",
        ],
        "required_apps": ["Copperfield Mail", "Signal Quay Chat", "Daymark Calendar", "LibreOffice Calc"],
        "note": "This block is byte-equivalent across platforms; only installation and launch details may vary.",
    }


def electron(platform: str, surface: str, name: str, bounds: dict) -> dict:
    return {
        "id": surface,
        "kind": "electron",
        "role": name,
        "cwd": "${bundle}/apps/lighthouse-desk",
        "command": ["npx.cmd" if platform == "windows" else "npx", "electron", ".", f"--surface={surface}", "--workspace=${workspace}"],
        "unset_env": ["ELECTRON_RUN_AS_NODE"],
        "window": {"title": name, "bounds": bounds},
    }


def descriptor(platform: str) -> dict:
    if platform == "macos":
        python = ["python3", "--version"]
        calc = "/Applications/LibreOffice.app/Contents/MacOS/soffice"
        root = "${HOME}/cdb-s02"
    elif platform == "windows":
        python = ["python", "--version"]
        calc = "C:\\Program Files\\LibreOffice\\program\\soffice.exe"
        root = "${USERPROFILE}\\cdb-s02"
    else:
        python = ["python3", "--version"]
        calc = "libreoffice"
        root = "${HOME}/cdb-s02"
    apps = [
        electron(platform, "mail", "Copperfield Mail", {"x": 20, "y": 30, "width": 900, "height": 680}),
        electron(platform, "chat", "Signal Quay Chat", {"x": 940, "y": 30, "width": 900, "height": 700}),
        electron(platform, "calendar", "Daymark Calendar", {"x": 40, "y": 180, "width": 1120, "height": 760}),
        {
            "id": "calc",
            "kind": "spreadsheet",
            "role": "LibreOffice Calc schedule record",
            "command": [
                calc, "--calc", "--norestore", "--nodefault", "--nolockcheck",
                "-env:UserInstallation=${workspace_uri}/.runtime/libreoffice",
                "${workspace}/planning/lighthouse-replan.ods",
            ],
            "window": {
                "title_contains": "lighthouse-replan",
                "bounds": {"x": 380, "y": 160, "width": 1180, "height": 780},
            },
        },
    ]
    return {
        "platform": platform,
        "descriptor_version": "1",
        "semantics": semantics(),
        "notes": [
            "Commands run with cwd set to the bundle unless an app declares cwd.",
            "Install Electron and LibreOffice before the network-denied trial; run apps in an interactive desktop session.",
        ],
        "prerequisites": [
            {"id": "python", "check": python, "min_version": "3.11"},
            {"id": "node", "check": ["node", "--version"], "min_version": "22"},
            {
                "id": "electron", "check": ["npm", "ls", "electron"],
                "cwd": "${bundle}/apps/lighthouse-desk", "install": ["npm", "ci"],
            },
            {"id": "libreoffice", "check": [calc, "--version"], "min_version": "25"},
        ],
        "workspace_root": root,
        "apps": apps,
        "teardown": {
            "stop_apps": ["mail", "chat", "calendar", "calc"],
            "then": ["${python}", "reset/setup.py", "--workspace", "${workspace}"],
        },
    }


def rendered(platform: str) -> str:
    return json.dumps(descriptor(platform), indent=2, ensure_ascii=False) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    for platform in ("macos", "windows", "linux"):
        path = PLATFORM / f"launch.{platform}.json"
        expected = rendered(platform)
        if args.check:
            if not path.is_file() or path.read_text(encoding="utf-8") != expected:
                stale.append(path.name)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(expected, encoding="utf-8")
    if stale:
        print("stale descriptors: " + ", ".join(stale))
        return 1
    print("platform descriptors up to date" if args.check else "platform descriptors updated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
