#!/usr/bin/env python3
"""Build deterministic launch descriptors for every supported platform."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
PLATFORM = BUNDLE / "platform"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def semantics() -> dict:
    return {
        "task_id": "cua.shared.cdb-s03.invoice-reconciliation",
        "task_version": "1.0.0",
        "brief": "brief.md",
        "brief_sha256": sha256(BUNDLE / "brief.md"),
        "manifest": "task.cuabench.json",
        "variant": "fixture.kestrel-wharf",
        "workspace_layout": {
            "mail": "mail",
            "workbook": "planning/kestrel-wharf-reconciliation.ods",
            "book": "book/kestrel-wharf.gnucash",
            "facts": "cdb-s03.workspace.json",
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
        "required_apps": ["Ledger Lantern Mail", "LibreOffice Calc", "GnuCash"],
        "note": "This block is byte-equivalent across platforms; only installation and launch details may vary.",
    }


def descriptor(platform: str) -> dict:
    common = {
        "platform": platform,
        "descriptor_version": "1",
        "semantics": semantics(),
        "notes": [
            "Commands run with cwd set to the bundle unless an app declares cwd.",
            "Install Electron, LibreOffice, and GnuCash before the network-denied trial.",
            "The GnuCash runtime directories are isolated inside the task workspace.",
        ],
        "prerequisites": [
            {"id": "python", "check": ["python3", "--version"], "min_version": "3.11"},
            {"id": "node", "check": ["node", "--version"], "min_version": "22"},
            {
                "id": "electron", "check": ["npm", "ls", "electron"],
                "cwd": "${bundle}/apps/ledger-lantern-mail", "install": ["npm", "ci"],
            },
        ],
    }
    if platform == "macos":
        common["workspace_root"] = "${HOME}/cdb-s03"
        calc = "/Applications/LibreOffice.app/Contents/MacOS/soffice"
        gnucash = "/Applications/Gnucash.app/Contents/MacOS/gnucash"
        common["prerequisites"] += [
            {"id": "libreoffice", "check": [calc, "--version"], "min_version": "25"},
            {"id": "gnucash", "check": [gnucash, "--version"], "min_version": "5.16"},
        ]
    elif platform == "windows":
        common["workspace_root"] = "${USERPROFILE}\\cdb-s03"
        calc = "C:\\Program Files\\LibreOffice\\program\\scalc.exe"
        # GnuCash 5.x's official Windows installer remains a Win32 package.
        gnucash = "C:\\Program Files (x86)\\gnucash\\bin\\gnucash.exe"
        common["prerequisites"] += [
            {"id": "libreoffice", "check": [calc, "--version"], "min_version": "25"},
            {"id": "gnucash", "check": [gnucash, "--version"], "min_version": "5.16"},
        ]
    else:
        common["workspace_root"] = "${HOME}/cdb-s03"
        calc = "libreoffice"
        gnucash = "gnucash"
        common["prerequisites"] += [
            {"id": "libreoffice", "check": [calc, "--version"], "min_version": "25"},
            {"id": "gnucash", "check": [gnucash, "--version"], "min_version": "5.16"},
        ]

    common["apps"] = [
        {
            "id": "mail", "kind": "electron", "role": "Ledger Lantern Mail",
            "cwd": "${bundle}/apps/ledger-lantern-mail",
            "command": ["npx", "electron", ".", "--workspace=${workspace}"],
            "unset_env": ["ELECTRON_RUN_AS_NODE"],
            "window": {"title": "Ledger Lantern Mail", "bounds": {"x": 20, "y": 30, "width": 900, "height": 700}},
        },
        {
            "id": "calc", "kind": "spreadsheet", "role": "LibreOffice Calc reconciliation record",
            "command": [calc, "--calc", "--norestore", "--nodefault", "--nolockcheck",
                        "-env:UserInstallation=${workspace_uri}/.runtime/libreoffice",
                        "${workspace}/planning/kestrel-wharf-reconciliation.ods"],
            "window": {"title_contains": "kestrel-wharf-reconciliation", "bounds": {"x": 350, "y": 150, "width": 1180, "height": 780}},
        },
        {
            "id": "gnucash", "kind": "accounting", "role": "GnuCash payment ledger",
            "command": [gnucash, "--logto=${workspace}/.runtime/gnucash/trace.log",
                        "${workspace}/book/kestrel-wharf.gnucash"],
            "env": {"GNC_DATA_HOME": "${workspace}/.runtime/gnucash", "GNC_CONFIG_HOME": "${workspace}/.runtime/gnucash"},
            "window": {"title_contains": "kestrel-wharf", "bounds": {"x": 650, "y": 90, "width": 1150, "height": 800}},
        },
    ]
    if platform == "windows":
        common["apps"][2]["command"][-1] = "${workspace_uri}/book/kestrel-wharf.gnucash"
    if platform == "linux":
        common["apps"][2]["env"].update({
            "GSETTINGS_BACKEND": "keyfile",
            "XDG_CONFIG_HOME": "${workspace}/.runtime/xdg",
            "XDG_DATA_HOME": "${workspace}/.runtime/xdg-data",
        })
    common["teardown"] = {
        "stop_apps": ["mail", "calc", "gnucash"],
        "then": ["${python}", "reset/setup.py", "--workspace", "${workspace}"],
    }
    return common


def render(platform: str) -> bytes:
    return (json.dumps(descriptor(platform), indent=2, ensure_ascii=False) + "\n").encode()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    PLATFORM.mkdir(parents=True, exist_ok=True)
    stale = []
    for name in ("macos", "windows", "linux"):
        path = PLATFORM / f"launch.{name}.json"
        content = render(name)
        if args.check:
            if not path.is_file() or path.read_bytes() != content:
                stale.append(path.name)
        else:
            path.write_bytes(content)
    if stale:
        raise SystemExit("stale platform descriptors: " + ", ".join(stale))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
