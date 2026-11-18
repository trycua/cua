#!/usr/bin/env python3
"""ALE (rdi-berkeley/agents-last-exam) compatibility check for cua-bench.

ALE pins cua-bench and uses it as a library: the task-authoring API
(Tier 1: decorators, ``cb.Task``, ``cb.DesktopSession``) and
``RemoteDesktopSession`` as its VM client (Tier 2/3). This script runs
ALE's own code against the installed cua-bench, offline:

* level 1: import every ``tasks/**/main.py`` with ALE's TaskLoader, call
  ``load()`` and check the ``cb.Task`` objects it returns.
* level 3: also run every task's ``start``/``evaluate`` through ALE's own
  harness code (``StaticProvider.open_session`` with its
  ``_init_computer_skip_wait``, ``TaskDriver`` with its resilient command
  patch, ``_force_close_session``) against a loopback computer-server replay
  (``cua_bench/tests/fake_computer_server.py``: in-memory files, shell
  commands recorded, never executed). A task may fail on missing data; a
  cua-bench surface it needs (AttributeError/TypeError on the session or
  interface) fails the check.

Nothing here provisions a VM or runs a task command on the host.

Usage::

    python scripts/ale_compat/check_ale.py --ale /path/to/agents-last-exam [--json out.json]

Exit status is non-zero only for cua-bench contract breaks. A task whose
module needs a third-party package that is not installed is reported as
``skipped`` (install ALE's ``tasks/`` eval deps to cover it).
"""

from __future__ import annotations

import argparse
import inspect
import json
import sys
import traceback
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

#: Modules that must stay out of ``import cua_bench`` (ALE imports every task
#: module on the host, so the package import has to stay light).
HEAVY_MODULES = ("playwright", "docker", "matplotlib", "torch", "cua_sandbox", "datasets")


@dataclass
class Outcome:
    task: str
    status: str  # ok | skipped | contract | task-error
    detail: str = ""
    variants: int = 0
    extra: dict = field(default_factory=dict)


def _is_cua_bench_frame(tb: Any) -> bool:
    for frame, _ in traceback.walk_tb(tb):
        if "cua_bench" in frame.f_code.co_filename:
            return True
    return False


def _missing_third_party(error: BaseException) -> Optional[str]:
    if isinstance(error, ModuleNotFoundError) and error.name:
        root = error.name.split(".")[0]
        # `computer` (cua-computer) came with cua-bench 0.2.x: part of the contract.
        if root not in ("cua_bench", "tasks", "ale_run", "computer"):
            return error.name
    return None


def load_task_module(main_py: Path) -> Any:
    """Import a task ``main.py`` with ALE's own ``TaskLoader`` (unchanged ALE code)."""
    from ale_run.tasks.loader import TaskLoader

    return TaskLoader(str(main_py.parent))._load_module()


def check_import_is_light() -> list[str]:
    """``import cua_bench`` must not drag in heavy optional stacks."""
    import subprocess

    code = (
        "import sys, cua_bench, cua_bench.computers.base, cua_bench.computers.remote;"
        f"print(','.join(m for m in {HEAVY_MODULES!r} if m in sys.modules))"
    )
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=120)
    if out.returncode != 0:
        return [f"import cua_bench failed: {out.stderr.strip()[-500:]}"]
    loaded = [m for m in out.stdout.strip().split(",") if m]
    return [f"import cua_bench loads heavy module(s): {', '.join(loaded)}"] if loaded else []


def check_tier1_surface() -> list[str]:
    """The authoring API ALE's 165 tasks rely on (ale-notes.md section 10)."""
    problems = []
    import cua_bench as cb

    for name in (
        "tasks_config",
        "setup_task",
        "evaluate_task",
        "solve_task",
        "Task",
        "DesktopSession",
    ):
        if not hasattr(cb, name):
            problems.append(f"cua_bench.{name} is missing")
    from cua_bench.computers.base import DesktopSession  # noqa: F401

    task = cb.Task(description="d", metadata={"a": 1}, computer={"provider": "computer"})
    task.metadata = {"b": 2}
    if (task.description, task.metadata, task.computer) != (
        "d",
        {"b": 2},
        {"provider": "computer"},
    ):
        problems.append("cb.Task attributes changed")

    @cb.tasks_config(split="train")
    def load():
        return [task]

    @cb.setup_task(split="train")
    async def start(task_cfg, session):
        return "started"

    @cb.evaluate_task(split="train")
    async def evaluate(task_cfg, session):
        return [1.0]

    if load() != [task]:
        problems.append("@cb.tasks_config no longer returns the function's value")
    import asyncio

    for fn, value in ((start, "started"), (evaluate, [1.0])):
        # ALE calls fn(task_cfg, session) and awaits a coroutine result.
        if list(inspect.signature(fn).parameters) != ["task_cfg", "session"]:
            problems.append(f"{fn.__name__} signature changed after decoration")
        result = fn(task, None)
        if not inspect.isawaitable(result):
            problems.append(f"{fn.__name__}(...) no longer returns an awaitable")
        elif asyncio.run(result) != value:
            problems.append(f"{fn.__name__} changed the return value")
    return problems


def check_session_surface() -> list[str]:
    """``RemoteDesktopSession`` as ALE's VM client (Tier 2 and the Tier-3 attrs)."""
    problems = []
    from cua_bench.computers.remote import RemoteDesktopSession

    session = RemoteDesktopSession(api_url="http://127.0.0.1:5000", os_type="linux")
    for attr in ("_os_type", "_api_host", "_api_port", "_vnc_port", "_initialized"):
        if not hasattr(session, attr):
            problems.append(f"RemoteDesktopSession.{attr} is missing (ALE reads it)")
    kwargs = inspect.signature(RemoteDesktopSession.__init__).parameters
    for name in ("api_url", "os_type", "ephemeral", "headless"):
        if name not in kwargs:
            problems.append(f"RemoteDesktopSession(..., {name}=) is gone")
    for name in (
        "file_exists",
        "directory_exists",
        "read_bytes",
        "read_file",
        "write_file",
        "write_bytes",
        "list_dir",
        "run_command",
        "screenshot",
        "check_status",
        "wait_until_ready",
        "close",
    ):
        if not inspect.iscoroutinefunction(getattr(RemoteDesktopSession, name, None)):
            problems.append(f"RemoteDesktopSession.{name} is missing or not async")
    return problems


def tier1(ale_root: Path, only: Optional[str] = None) -> list[Outcome]:
    import cua_bench as cb

    outcomes = []
    mains = sorted((ale_root / "tasks").glob("*/*/main.py"))
    if only:
        mains = [m for m in mains if only in str(m)]
    for main_py in mains:
        name = str(main_py.parent.relative_to(ale_root / "tasks"))
        try:
            module = load_task_module(main_py)
        except BaseException as error:  # noqa: BLE001 - classified below
            missing = _missing_third_party(error)
            if missing:
                outcomes.append(Outcome(name, "skipped", f"needs {missing}"))
            elif _is_cua_bench_frame(error.__traceback__) or "cua_bench" in repr(error):
                outcomes.append(Outcome(name, "contract", f"import: {error!r}"))
            else:
                outcomes.append(Outcome(name, "task-error", f"import: {error!r}"))
            continue
        load = getattr(module, "load", None)
        if load is None:
            outcomes.append(Outcome(name, "ok", "no load() (ALE fallback discovery)"))
            continue
        try:
            tasks = load()
        except BaseException as error:  # noqa: BLE001
            status = "contract" if _is_cua_bench_frame(error.__traceback__) else "task-error"
            outcomes.append(Outcome(name, status, f"load(): {error!r}"))
            continue
        bad = [t for t in tasks if not isinstance(t, cb.Task)]
        if bad or not isinstance(tasks, list):
            outcomes.append(Outcome(name, "contract", f"load() returned {type(tasks).__name__}"))
            continue
        problems = []
        for task in tasks:
            setup = (task.computer or {}).get("setup_config") or {}
            if not isinstance(task.description, str) or not isinstance(task.metadata, dict):
                problems.append("description/metadata types")
            if not isinstance(setup, dict):
                problems.append("computer.setup_config is not a dict")
        for fn_name in ("start", "evaluate"):
            fn = getattr(module, fn_name, None)
            if fn is not None and not callable(fn):
                problems.append(f"{fn_name} is not callable")
        if problems:
            outcomes.append(Outcome(name, "contract", "; ".join(sorted(set(problems)))))
        else:
            outcomes.append(Outcome(name, "ok", variants=len(tasks)))
    return outcomes


# ── Tier 2/3: ALE's harness against a legacy computer-server replay ─────────

#: Exceptions that mean cua-bench broke a surface ALE uses (not a task failure).
_CONTRACT_TYPES = (AttributeError, TypeError, NotImplementedError, ImportError)
_OUR_TYPES = (
    "RemoteDesktopSession",
    "LegacyInterface",
    "ComputerServerInterface",
    "SandboxInterface",
    "Computer",
    "_SandboxComputer",
)


def _classify(error: BaseException) -> str:
    if isinstance(error, ModuleNotFoundError) and _missing_third_party(error):
        return "skipped"
    if isinstance(error, _CONTRACT_TYPES):
        obj = getattr(error, "obj", None)
        if obj is not None and type(obj).__name__ in _OUR_TYPES:
            return "contract"
        frames = list(traceback.walk_tb(error.__traceback__))
        if frames and "cua_bench" in frames[-1][0].f_code.co_filename:
            return "contract"
        if any(name in str(error) for name in _OUR_TYPES):
            return "contract"
    return "task-error"


async def _run_task_on_replay(
    ale_root: Path, main_py: Path, server: Any, timeout: float
) -> Outcome:
    """ALE's own path: StaticProvider.open_session (RemoteDesktopSession +
    _init_computer_skip_wait), TaskDriver (install_resilient_cua_commands),
    the task's start/evaluate, then _force_close_session."""
    import asyncio

    from ale_run.environments.env import _force_close_session
    from ale_run.environments.providers.static import StaticProvider
    from ale_run.tasks.driver import TaskDriver

    name = str(main_py.parent.relative_to(ale_root / "tasks"))
    try:
        from ale_run.tasks.loader import TaskLoader

        info = TaskLoader(str(main_py.parent)).load(variant_index=0)
    except BaseException as error:  # noqa: BLE001
        return Outcome(name, _classify(error), f"load: {error!r}")
    os_type = info.get("os_type", "linux")
    image = "ale-ubuntu22" if os_type == "linux" else "ale-win10"
    provider = StaticProvider({"endpoint": server.url, "image": image})
    handle = await provider.acquire(type("Spec", (), {"snapshot": None})())
    try:
        session = provider.open_session(handle)
    except BaseException as error:  # noqa: BLE001
        status = (
            "contract" if isinstance(error, (ImportError, AttributeError)) else _classify(error)
        )
        return Outcome(name, status, f"open_session: {error!r}")
    before = len(server.commands)
    extra: dict = {}
    try:
        driver = TaskDriver(str(main_py.parent), session, variant=0, os_type=handle.os)
        iface = session.computer.interface
        extra["resilient_installed"] = bool(getattr(iface, "_ale_resilient_commands", False))
        if not extra["resilient_installed"]:
            return Outcome(
                name, "contract", "install_resilient_cua_commands did not apply", extra=extra
            )
        task_cfg = driver._make_task_cfg()
        phases = []
        for phase, getter in (("setup", "get_setup_fn"), ("evaluate", "get_evaluate_fn")):
            fn = getattr(driver._task_loader, getter)()
            if fn is None:
                continue
            try:
                result = fn(task_cfg, session)
                if asyncio.iscoroutine(result):
                    result = await asyncio.wait_for(result, timeout)
                phases.append(f"{phase}=ok")
            except asyncio.TimeoutError:
                phases.append(f"{phase}=timeout")
            except BaseException as error:  # noqa: BLE001
                status = _classify(error)
                if status == "contract":
                    return Outcome(name, "contract", f"{phase}: {error!r}", extra=extra)
                phases.append(f"{phase}={type(error).__name__}")
        extra["commands"] = sorted({c for c, _ in server.commands[before:]})
        return Outcome(name, "ok", " ".join(phases), extra=extra)
    finally:
        await _force_close_session(session)


def tier3(ale_root: Path, only: Optional[str], timeout: float) -> list[Outcome]:
    """Every task's start/evaluate through ALE's harness on a replay server."""
    import asyncio
    import logging

    # Loaded by path so the checked cua_bench is whichever one is installed.
    import importlib.util

    fake_py = (
        Path(__file__).resolve().parents[2] / "cua_bench" / "tests" / "fake_computer_server.py"
    )
    spec = importlib.util.spec_from_file_location("_cb_fake_computer_server", fake_py)
    fake_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fake_module)
    FakeComputerServer = fake_module.FakeComputerServer

    import ale_run.tasks.driver as ale_driver

    async def no_backoff(_attempt: int) -> None:  # CI speed; ALE's retry logic is unchanged
        return None

    ale_driver._sleep_before_retry = no_backoff
    logging.getLogger("ale_run").setLevel(logging.ERROR)

    async def run_all() -> list[Outcome]:
        server = await FakeComputerServer().start()
        outcomes = []
        try:
            mains = sorted((ale_root / "tasks").glob("*/*/main.py"))
            if only:
                mains = [m for m in mains if only in str(m)]
            for main_py in mains:
                try:
                    outcome = await asyncio.wait_for(
                        _run_task_on_replay(ale_root, main_py, server, timeout), timeout * 3
                    )
                except asyncio.TimeoutError:
                    outcome = Outcome(str(main_py.parent.name), "task-error", "timeout")
                outcomes.append(outcome)
        finally:
            await server.stop()
        return outcomes

    return asyncio.run(run_all())


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--ale", required=True, type=Path, help="agents-last-exam checkout")
    parser.add_argument("--only", help="substring filter on task paths")
    parser.add_argument("--json", type=Path, help="write the per-task report here")
    parser.add_argument(
        "--level",
        type=int,
        default=3,
        choices=(1, 3),
        help="1: task-authoring API only; 3: also RemoteDesktopSession and ALE's harness",
    )
    parser.add_argument(
        "--task-timeout",
        type=float,
        default=30.0,
        help="seconds per start/evaluate call on the replay server",
    )
    parser.add_argument(
        "--strict-import",
        action="store_true",
        help="fail when `import cua_bench` loads a heavy optional module",
    )
    args = parser.parse_args(argv)

    ale_root = args.ale.resolve()
    sys.path.insert(0, str(ale_root))
    # ALE's evaluators write their JSON under ./trycua/cua-bench (or
    # EVALUATION_OUTPUT_DIR): keep that out of the checkout.
    import os
    import tempfile

    scratch = tempfile.mkdtemp(prefix="ale-compat-")
    os.environ.setdefault("EVALUATION_OUTPUT_DIR", os.path.join(scratch, "eval"))
    if args.json:
        args.json = args.json.resolve()
    os.chdir(scratch)
    problems = check_tier1_surface()
    heavy = check_import_is_light()
    if args.strict_import:
        problems += heavy
    else:
        for note in heavy:
            print(f"  [warning] {note}")
    if args.level >= 3:
        problems += check_session_surface()
    outcomes = tier1(ale_root, args.only)
    if args.level >= 3:
        replay = tier3(ale_root, args.only, args.task_timeout)
        counts3: dict[str, int] = {}
        for outcome in replay:
            counts3[outcome.status] = counts3.get(outcome.status, 0) + 1
        print(
            f"ALE harness replay (setup+evaluate): {len(replay)}  "
            + "  ".join(f"{k}={v}" for k, v in sorted(counts3.items()))
        )
        for outcome in replay:
            if outcome.status == "contract":
                print(f"  [contract] {outcome.task}: {outcome.detail}")
        outcomes += [
            Outcome(o.task, o.status, "replay: " + o.detail, o.variants, o.extra) for o in replay
        ]

    counts: dict[str, int] = {}
    for outcome in outcomes:
        counts[outcome.status] = counts.get(outcome.status, 0) + 1
    print(
        f"ALE tasks: {len(outcomes)}  " + "  ".join(f"{k}={v}" for k, v in sorted(counts.items()))
    )
    for outcome in outcomes:
        if outcome.status in ("contract", "task-error", "skipped"):
            print(f"  [{outcome.status}] {outcome.task}: {outcome.detail}")
    for problem in problems:
        print(f"  [contract] {problem}")
    if args.json:
        args.json.write_text(
            json.dumps(
                {
                    "problems": problems,
                    "counts": counts,
                    "tasks": [outcome.__dict__ for outcome in outcomes],
                },
                indent=2,
            )
        )
    contract = problems + [o for o in outcomes if o.status == "contract"]
    return 1 if contract else 0


if __name__ == "__main__":
    sys.exit(main())
