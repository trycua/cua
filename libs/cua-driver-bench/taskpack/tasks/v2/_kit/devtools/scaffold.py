#!/usr/bin/env python3
"""Create the boilerplate of a v2 task (descriptor, reset, verify, evaluator stub, test stub, provenance).

    python3 scaffold.py --id WF-01 --dir wf01-workshop-booking --category web_forms \
        --title "..." --kind web --port 4201 --window "Page title" [--apps "Google Chrome"]

Task content (brief, app, fixture, checks) is written by hand afterwards. Existing files are kept.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]  # tasks/v2
TODAY = "2026-10-11"
AUTHOR = "Cua, agent-authored, reviewed"

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
SOFFICE = "/Applications/LibreOffice.app/Contents/MacOS/soffice"

HEADER = '''import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
'''


def write(path: Path, text: str) -> None:
    if path.exists():
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, "utf-8")


def chrome_app(port: int, window: str, path: str = "/") -> dict:
    return {
        "id": "browser",
        "kind": "browser",
        "role": "Google Chrome on the task's local page",
        "command": [
            CHROME, "--no-first-run", "--no-default-browser-check", "--disable-default-apps", "--disable-sync",
            "--disable-features=Translate,PasswordManagerOnboarding,AutofillServerCommunication",
            "--user-data-dir=${workspace}/.runtime/chrome", "--new-window", f"http://127.0.0.1:{port}{path}",
        ],
        "window": {"title_contains": window, "bounds": {"x": 40, "y": 40, "width": 1240, "height": 900}},
    }


def service_app(port: int) -> dict:
    return {
        "id": "site",
        "kind": "service",
        "role": "the task's local web app (loopback only)",
        "command": [
            "${python}", "${bundle}/../_kit/v2kit/serve.py", "--app", "${bundle}/app", "--state-dir",
            "${workspace}/state", "--port", str(port), "--initial", "${bundle}/app/initial-state.json",
        ],
        "ready": {"http": f"http://127.0.0.1:{port}/healthz", "timeout_seconds": 15},
    }


def libreoffice_app(module: str, document: str, window: str) -> dict:
    return {
        "id": "office",
        "kind": "office",
        "role": f"LibreOffice {module}",
        "command": [
            SOFFICE, f"--{module}", "--norestore", "--nodefault", "--nolockcheck",
            "-env:UserInstallation=${workspace_uri}/.runtime/libreoffice", "${workspace}/" + document,
        ],
        "process_names": ["soffice", "soffice.bin"],
        "window": {"title_contains": window, "bounds": {"x": 40, "y": 40, "width": 1280, "height": 900}},
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", required=True)
    ap.add_argument("--dir", required=True)
    ap.add_argument("--category", required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--kind", choices=("web", "office", "native"), required=True)
    ap.add_argument("--port", type=int)
    ap.add_argument("--window", default="")
    ap.add_argument("--module", default="calc")
    ap.add_argument("--document", default="")
    ap.add_argument("--apps", default="")
    args = ap.parse_args()
    task = ROOT / args.dir
    slug = args.dir.split("-", 1)[1] if "-" in args.dir else args.dir
    apps = []
    required = [a.strip() for a in args.apps.split(",") if a.strip()]
    if args.kind == "web":
        apps = [service_app(args.port), chrome_app(args.port, args.window)]
        required = required or ["Google Chrome"]
    elif args.kind == "office":
        apps = [libreoffice_app(args.module, args.document, args.window)]
        required = required or [f"LibreOffice {args.module.title()}"]
    descriptor = {
        "platform": "macos",
        "descriptor_version": "1",
        "semantics": {
            "task_id": f"cua.v2.{args.dir}",
            "task_version": "1.0.0",
            "brief": "brief.md",
            "reset": {
                "setup": ["${python}", "reset/setup.py", "--workspace", "${workspace}"],
                "verify": ["${python}", "reset/verify.py", "--workspace", "${workspace}"],
            },
            "evaluate": [
                "${python}", "evaluator/evaluate.py", "--workspace", "${workspace}", "--artifacts", "${artifacts}",
                "--result", "${result}", "--agent-exit-code", "${agent_exit_code}",
            ],
            "required_apps": required,
        },
        "workspace_root": "${HOME}/v2-" + args.dir.split("-", 1)[0],
        "apps": apps,
    }
    write(task / "platform" / "launch.macos.json", json.dumps(descriptor, indent=2) + "\n")
    meta = {
        "id": args.id,
        "pack_task": f"v2/{args.dir}",
        "title": args.title,
        "category": args.category,
        "gui_only": True,
        "apps": required,
        "conditions": ["foreground", "background"],
        "written": TODAY,
        "author": AUTHOR,
        "provenance": "original, written from scratch",
    }
    write(task / "task.v2.json", json.dumps(meta, indent=2) + "\n")
    setup_extra = {
        "web": "    common.seed_chrome_profile(workspace)\n",
        "office": "    common.seed_libreoffice_profile(workspace)\n",
        "native": "    pass\n",
    }[args.kind]
    write(task / "reset" / "setup.py", f'"""Reset {args.id}: copy the fixture into the workspace."""\n\n{HEADER}\nfrom v2kit import common  # noqa: E402\n\n\ndef setup(workspace: Path) -> None:\n{setup_extra}\n\nif __name__ == "__main__":\n    common.reset_main(BUNDLE, "{args.id}", setup)\n')
    write(task / "reset" / "verify.py", f'"""Verify the reset of {args.id}."""\n\n{HEADER}\nfrom v2kit import common  # noqa: E402\n\nif __name__ == "__main__":\n    common.verify_main(BUNDLE, "{args.id}")\n')
    write(task / "evaluator" / "evaluate.py", f'"""Evaluate {args.id}."""\n\nfrom __future__ import annotations\n\n{HEADER}\nfrom v2kit import common  # noqa: E402\n\nTASK_ID = "{args.id}"\n\n\ndef evaluate(workspace: Path, checks: common.Checks) -> None:\n    raise NotImplementedError\n\n\nif __name__ == "__main__":\n    common.evaluator_main(TASK_ID, evaluate)\n')
    write(task / "fixture" / "README.txt", f"Workspace of Bench v2 task {args.id}. Synthetic data only.\n")
    write(task / "PROVENANCE.md", f"""# Provenance: {args.id} ({args.title})

* **Classification:** original, written from scratch for the Bench v2 task-set extension (42 tasks).
* **Written:** {TODAY}.
* **Author:** {AUTHOR}. Written by a Claude agent session working for Cua, then reviewed before merge (scenario, wording, fixture, checker and tests).
* **Sources:** none. The scenario, every name and number, the page or document, the fixture and the
  checker were invented for this task. No task, text, fixture, checker or asset of any other benchmark
  (OSWorld, WindowsAgentArena, WebArena, VisualWebArena, Mind2Web, WorkArena, AndroidWorld, tau-bench,
  GDPval, Terminal-Bench, SkillsBench, ALE, Hunch Bench, Mac Agent Bench or others) or of any customer
  corpus was opened, adapted, paraphrased or structurally mirrored.
* **Exposure declared:** the author had read a one-line-per-task summary of a few public benchmark
  tasks while auditing provenance. No benchmark task file was opened while writing.
* **Fixtures:** synthetic. All people, organisations, places and figures are invented. No real person,
  company or copyrighted asset appears.
* **Third-party material:** none. The task runs on {", ".join(required) or "the apps named in its descriptor"},
  which are runtime prerequisites, not distributed.
""")
    print(task)


if __name__ == "__main__":
    main()
