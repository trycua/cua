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
        "task_id": "cua.shared.cdb-s04.linked-support-diagnosis",
        "task_version": "1.0.0",
        "brief": "brief.md",
        "brief_sha256": sha256(BUNDLE / "brief.md"),
        "manifest": "task.cuabench.json",
        "variant": "fixture.morrowglass",
        "workspace_layout": {
            "cases": "cases",
            "wiki": "wiki",
            "archive_site": "archive-site",
            "logs": "logs",
            "facts": "cdb-s04.workspace.json",
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
        "required_apps": [
            "Rillwatch Cases", "Margin Lantern Manual",
            "Chromium-family browser", "terminal or shell",
        ],
        "note": "This block is byte-equivalent across platforms; only installation and launch details may vary.",
    }


def descriptor(platform: str) -> dict:
    common = {
        "platform": platform,
        "descriptor_version": "1",
        "semantics": semantics(),
        "notes": [
            "Commands run with cwd set to the bundle unless an app declares cwd.",
            "Install both Electron app dependencies and a supported Chromium-family browser before the network-denied trial.",
            "The archive service binds only to loopback and all browser state is isolated inside the task workspace.",
            "The terminal/log surface may be supplied by the harness shell; no platform-specific terminal process is required.",
        ],
        "prerequisites": [
            {"id": "python", "check": ["python3", "--version"], "min_version": "3.11"},
            {"id": "node", "check": ["node", "--version"], "min_version": "22"},
            {
                "id": "cases-electron", "check": ["npm", "ls", "electron"],
                "cwd": "${bundle}/apps/rillwatch-cases", "install": ["npm", "ci"],
            },
            {
                "id": "wiki-electron", "check": ["npm", "ls", "electron"],
                "cwd": "${bundle}/apps/margin-lantern-manual", "install": ["npm", "ci"],
            },
        ],
    }

    if platform == "macos":
        common["workspace_root"] = "${HOME}/cdb-s04"
        browser = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    elif platform == "windows":
        common["workspace_root"] = "${USERPROFILE}\\cdb-s04"
        browser = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
    else:
        common["workspace_root"] = "${HOME}/cdb-s04"
        browser = "google-chrome"

    common["prerequisites"].append(
        {"id": "browser", "check": [browser, "--version"], "family": "chromium"}
    )
    common["apps"] = [
        {
            "id": "archive", "kind": "service", "role": "Morrowglass Archive reproduction target",
            "cwd": "${workspace}/archive-site", "command": ["node", "server.js"],
            "env": {"CDB_S04_HOST": "127.0.0.1", "CDB_S04_PORT": "4184"},
            "ready": {"url": "http://127.0.0.1:4184/healthz", "timeout_seconds": 15},
        },
        {
            "id": "cases", "kind": "electron", "role": "Rillwatch Cases",
            "cwd": "${bundle}/apps/rillwatch-cases",
            "command": ["npx", "electron", ".", "--workspace=${workspace}"],
            "unset_env": ["ELECTRON_RUN_AS_NODE"],
            "window": {"title": "Rillwatch Cases", "bounds": {"x": 20, "y": 30, "width": 980, "height": 760}},
        },
        {
            "id": "wiki", "kind": "electron", "role": "Margin Lantern Manual",
            "cwd": "${bundle}/apps/margin-lantern-manual",
            "command": ["npx", "electron", ".", "--workspace=${workspace}"],
            "unset_env": ["ELECTRON_RUN_AS_NODE"],
            "window": {"title": "Margin Lantern Manual", "bounds": {"x": 330, "y": 90, "width": 900, "height": 720}},
        },
        {
            "id": "browser", "kind": "browser", "role": "Morrowglass Archive live reproduction",
            "command": [
                browser, "--no-first-run", "--disable-default-apps",
                "--user-data-dir=${workspace}/.runtime/browser",
                "--new-window", "http://127.0.0.1:4184/",
            ],
            "window": {"title_contains": "Morrowglass Archive", "bounds": {"x": 620, "y": 40, "width": 1080, "height": 790}},
        },
    ]
    common["teardown"] = {
        "stop_apps": ["browser", "wiki", "cases", "archive"],
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
