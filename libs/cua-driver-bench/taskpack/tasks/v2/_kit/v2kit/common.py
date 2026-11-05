"""Workspace reset, app profiles, and the result format every v2 evaluator writes."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable

RESULT_VERSION = 1


# ------------------------------------------------------------------ files


def write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".tmp-", suffix=".json")
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=2, sort_keys=True)
        handle.write("\n")
    os.replace(tmp, path)


def read_json(path: Path, default: Any = None) -> Any:
    try:
        return json.loads(path.read_text("utf-8"))
    except (OSError, ValueError):
        return default


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    try:
        lines = path.read_text("utf-8").splitlines()
    except OSError:
        return out
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if isinstance(item, dict):
            out.append(item)
    return out


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree_digest(root: Path, skip: tuple[str, ...] = (".runtime", "__pycache__")) -> dict[str, str]:
    """Relative path -> sha256 of every file under root (for reset verification)."""
    out: dict[str, str] = {}
    for item in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = item.relative_to(root)
        if any(part in skip for part in rel.parts) or item.name == ".DS_Store":
            continue
        out[str(rel)] = sha256_file(item)
    return out


def reset_workspace(bundle: Path, workspace: Path) -> None:
    """Remove the workspace and copy the task's fixture/ tree into it."""
    if workspace.exists():
        shutil.rmtree(workspace)
    shutil.copytree(bundle / "fixture", workspace)


def fixture_matches(bundle: Path, workspace: Path) -> list[str]:
    """Differences between fixture/ and the workspace (ignores .runtime and state/)."""
    want = tree_digest(bundle / "fixture")
    got = tree_digest(workspace, skip=(".runtime", "__pycache__", "state"))
    problems = [f"missing {k}" for k in want if k not in got]
    problems += [f"changed {k}" for k in want if k in got and got[k] != want[k]]
    problems += [f"extra {k}" for k in got if k not in want]
    return problems


# ------------------------------------------------------------------ app profiles

LIBREOFFICE_SETTINGS = {
    # No modal or infobar on a fresh profile; recovery off so a killed trial leaves nothing behind.
    ("/org.openoffice.Office.Common/Misc", "ShowTipOfTheDay"): ("xs:boolean", "false"),
    ("/org.openoffice.Office.Common/Misc", "FirstRun"): ("xs:boolean", "false"),
    ("/org.openoffice.Office.Recovery/AutoSave", "Enabled"): ("xs:boolean", "false"),
    ("/org.openoffice.Office.Recovery/RecoveryInfo", "Enabled"): ("xs:boolean", "false"),
    ("/org.openoffice.Office.Common/Misc", "UseSystemFileDialog"): ("xs:boolean", "true"),
    ("/org.openoffice.Setup/Office", "ooSetupInstCompleted"): ("xs:boolean", "true"),
    # The welcome / what's-new dialog shows when the last-run version differs from the installed one
    # (LibreOffice 26.2 in the macOS VM; validation met it on a fresh profile).
    ("/org.openoffice.Setup/Product", "ooSetupLastVersion"): ("xs:string", "26.2"),
}


def seed_libreoffice_profile(workspace: Path, extra: dict[tuple[str, str], tuple[str, str]] | None = None) -> Path:
    """A per-workspace LibreOffice user profile (`-env:UserInstallation=<workspace uri>/.runtime/libreoffice`)."""
    user = workspace / ".runtime" / "libreoffice" / "user"
    user.mkdir(parents=True, exist_ok=True)
    settings = dict(LIBREOFFICE_SETTINGS)
    settings.update(extra or {})
    items = []
    for (path, name), (kind, value) in sorted(settings.items()):
        items.append(
            f'<item oor:path="{path}"><prop oor:name="{name}" oor:op="fuse">'
            f'<value>{value}</value></prop></item>'
        )
    text = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<oor:items xmlns:oor="http://openoffice.org/2001/registry" '
        'xmlns:xs="http://www.w3.org/2001/XMLSchema" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">\n'
        + "\n".join(items)
        + "\n</oor:items>\n"
    )
    (user / "registrymodifications.xcu").write_text(text, "utf-8")
    return user


def seed_chrome_profile(workspace: Path) -> Path:
    """A per-workspace Chrome profile without first-run, sign-in, password or translate prompts."""
    root = workspace / ".runtime" / "chrome"
    (root / "Default").mkdir(parents=True, exist_ok=True)
    (root / "First Run").write_text("", "utf-8")
    prefs = {
        "browser": {"has_seen_welcome_page": True, "check_default_browser": False},
        "credentials_enable_service": False,
        "profile": {"password_manager_enabled": False, "default_content_setting_values": {}},
        "translate": {"enabled": False},
        "autofill": {"profile_enabled": False, "credit_card_enabled": False},
        "signin": {"allowed": False},
        "sync_promo": {"show_on_first_run_allowed": False},
        "distribution": {"skip_first_run_ui": True, "suppress_first_run_bubble": True},
    }
    (root / "Default" / "Preferences").write_text(json.dumps(prefs), "utf-8")
    return root


# ------------------------------------------------------------------ checks and results


class Checks:
    """Named checks. `progress` checks make up the score; a failed `preservation` check halves it.

    passed = every check passed. score = passed progress checks / progress checks, halved when any
    preservation check fails. An untouched start must score 0 (every task's tests check this)."""

    def __init__(self) -> None:
        self.items: dict[str, dict[str, Any]] = {}
        self.diagnostics: dict[str, Any] = {}

    def add(self, name: str, ok: bool, detail: str = "", kind: str = "progress") -> bool:
        if kind not in ("progress", "preservation"):
            raise ValueError(kind)
        if name in self.items:
            raise ValueError(f"duplicate check {name}")
        self.items[name] = {"passed": bool(ok), "detail": str(detail)[:500], "kind": kind}
        return bool(ok)

    def diag(self, name: str, value: Any) -> None:
        self.diagnostics[name] = value

    @property
    def passed(self) -> bool:
        return bool(self.items) and all(c["passed"] for c in self.items.values())

    @property
    def score(self) -> float:
        progress = [c for c in self.items.values() if c["kind"] == "progress"]
        if not progress:
            return 0.0
        value = sum(c["passed"] for c in progress) / len(progress)
        if any(not c["passed"] for c in self.items.values() if c["kind"] == "preservation"):
            value /= 2
        return round(value, 4)

    def result(self, task_id: str) -> dict[str, Any]:
        return {
            "result_version": RESULT_VERSION,
            "task_id": task_id,
            "passed": self.passed,
            "score": self.score,
            "checks": self.items,
            "diagnostics": self.diagnostics,
        }


def evaluator_main(task_id: str, evaluate: Callable[[Path, Checks], None]) -> None:
    """CLI shared by every v2 evaluator: --workspace W --result R [--artifacts A] [--agent-exit-code N]."""
    parser = argparse.ArgumentParser(description=f"Evaluate {task_id}")
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--result", required=True, type=Path)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--agent-exit-code", default="0")
    args = parser.parse_args()
    checks = Checks()
    try:
        evaluate(args.workspace, checks)
    except Exception as error:  # noqa: BLE001 - a crash is a failed check, never a pass
        checks.items.setdefault(
            "evaluator_ok", {"passed": False, "detail": f"{type(error).__name__}: {error}"[:500], "kind": "progress"}
        )
    data = checks.result(task_id)
    data["agent_exit_code"] = args.agent_exit_code
    write_json(args.result, data)
    print(json.dumps({"passed": data["passed"], "score": data["score"]}))


def evaluate_dir(task_id: str, evaluate: Callable[[Path, Checks], None], workspace: Path) -> dict[str, Any]:
    """The same as evaluator_main, in-process (for tests)."""
    checks = Checks()
    evaluate(workspace, checks)
    return checks.result(task_id)


def reset_main(bundle: Path, task_id: str, setup: Callable[[Path], None] | None = None) -> None:
    """CLI for reset/setup.py: copy fixture/ to the workspace, then the task's own setup."""
    parser = argparse.ArgumentParser(description=f"Reset {task_id}")
    parser.add_argument("--workspace", required=True, type=Path)
    args = parser.parse_args()
    reset_workspace(bundle, args.workspace)
    if setup is not None:
        setup(args.workspace)
    print(f"{task_id}: workspace reset at {args.workspace}")


def verify_main(bundle: Path, task_id: str, verify: Callable[[Path], list[str]] | None = None) -> None:
    """CLI for reset/verify.py: the workspace equals the fixture (plus the task's own checks)."""
    parser = argparse.ArgumentParser(description=f"Verify the reset of {task_id}")
    parser.add_argument("--workspace", required=True, type=Path)
    args = parser.parse_args()
    problems = fixture_matches(bundle, args.workspace)
    if verify is not None:
        problems += verify(args.workspace)
    if problems:
        print(f"{task_id}: reset verification failed: {problems[:10]}", file=sys.stderr)
        raise SystemExit(1)
    print(f"{task_id}: reset verified")


def app_reset_main(
    bundle: Path,
    task_id: str,
    files: Callable[[Path], None] | None,
    app_state: Callable[[Path], None] | None,
) -> None:
    """CLI for the reset of a native-app task: copy the fixture, then (only with --apply-app-state, which
    the launch descriptor passes inside the macOS VM) reset the app's own data. Tests never pass it."""
    parser = argparse.ArgumentParser(description=f"Reset {task_id}")
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--apply-app-state", action="store_true")
    args = parser.parse_args()
    reset_workspace(bundle, args.workspace)
    if files is not None:
        files(args.workspace)
    if args.apply_app_state and app_state is not None:
        app_state(args.workspace)
    print(f"{task_id}: workspace reset at {args.workspace}" + (" (app state too)" if args.apply_app_state else ""))


def app_verify_main(
    bundle: Path,
    task_id: str,
    files: Callable[[Path], list[str]] | None,
    app_state: Callable[[Path], list[str]] | None,
) -> None:
    """CLI for the reset verification of a native-app task (app data only with --apply-app-state)."""
    parser = argparse.ArgumentParser(description=f"Verify the reset of {task_id}")
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--apply-app-state", action="store_true")
    args = parser.parse_args()
    problems = fixture_matches(bundle, args.workspace)
    if files is not None:
        problems += files(args.workspace)
    if args.apply_app_state and app_state is not None:
        problems += app_state(args.workspace)
    if problems:
        print(f"{task_id}: reset verification failed: {problems[:10]}", file=sys.stderr)
        raise SystemExit(1)
    print(f"{task_id}: reset verified")
