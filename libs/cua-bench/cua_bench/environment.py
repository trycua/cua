"""Simplified, provider-driven environment."""

from __future__ import annotations

import inspect
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any, Callable, Dict, Literal, Optional, Tuple

from .bot import Bot
from .computers import DesktopSession, DesktopSetupConfig, get_session
from .tracing import Tracing
from .types import Action

# Telemetry imports (optional)
try:
    from cua_bench.telemetry import (
        track_task_evaluation_completed,
        track_task_execution_failed,
        track_task_execution_started,
    )

    _telemetry_available = True
except ImportError:
    _telemetry_available = False


async def _call_function(func, *args, **kwargs):
    """Calls a function, awaiting it if it is awaitable."""
    result = func(*args, **kwargs)
    if inspect.isawaitable(result):
        return await result
    else:
        return result


class MaxStepsExceeded(Exception):
    """Raised when the environment's max step budget is exhausted."""

    pass


class Environment:
    """A minimal environment wrapper that delegates everything to a provider.

    Functions can be injected directly, or discovered from a module via
    `make_from_module` based on cua-bench decorators (`_td_type`, `_td_split`).
    """

    session: Optional[DesktopSession] = None
    env_name: Optional[str] = None
    split: Optional[str] = None
    headless: bool = True
    print_actions: bool = False
    bot: Optional[Bot] = None
    tracing: Optional[Tracing] = None

    # step counter
    step_count: int = 0
    max_steps: Optional[int] = None

    # Telemetry tracking
    _execution_start_time: Optional[float] = None
    _run_id: Optional[str] = None

    def __init__(
        self,
        env_name: Optional[str] = None,
        *,
        split: str = "train",
        tasks_config_fn: Optional[Callable[..., Any]] = None,
        setup_task_fn: Optional[Callable[..., Any]] = None,
        solve_task_fn: Optional[Callable[..., Any]] = None,
        evaluate_task_fn: Optional[Callable[..., Any]] = None,
    ) -> None:
        self.env_name = env_name
        self.split = split

        self.tasks_config_fn = tasks_config_fn
        self.setup_task_fn = setup_task_fn
        self.solve_task_fn = solve_task_fn
        self.evaluate_task_fn = evaluate_task_fn

        self.tasks: Optional[list] = None
        self.current_task: Optional[Any] = None

        self.session_name: Optional[str] = None
        self.session_config: Dict[str, Any] = {}
        self.setup_config: DesktopSetupConfig = {}

        self.session: Optional[Any] = None
        self.page: Optional[Any] = None
        self.bot: Optional[Bot] = None
        self.tracing: Optional[Tracing] = Tracing(self)

    # --- Construction helpers ---
    @classmethod
    def make_from_module(
        cls,
        module: Any,
        *,
        env_path: str | Path,
        split: str = "train",
    ) -> "Environment":
        tasks_config_fn = None
        setup_task_fn = None
        solve_task_fn = None
        evaluate_task_fn = None

        for name in dir(module):
            obj = getattr(module, name)
            if callable(obj) and hasattr(obj, "_td_type"):
                if getattr(obj, "_td_split", None) != split:
                    continue
                t = getattr(obj, "_td_type")
                if t == "tasks_config":
                    tasks_config_fn = obj
                elif t == "setup_task":
                    setup_task_fn = obj
                elif t == "solve_task":
                    solve_task_fn = obj
                elif t == "evaluate_task":
                    evaluate_task_fn = obj

        return cls(
            env_name=Path(env_path).resolve().name,
            split=split,
            tasks_config_fn=tasks_config_fn,
            setup_task_fn=setup_task_fn,
            solve_task_fn=solve_task_fn,
            evaluate_task_fn=evaluate_task_fn,
        )

    # --- Session wiring ---
    async def create_sandbox(
        self,
        provider: str,
        provider_config: Dict[str, Any] | None = None,
        setup_config: DesktopSetupConfig | None = None,
    ) -> None:
        self.session_name = provider
        self.session_config = dict(provider_config or {})
        self.setup_config = DesktopSetupConfig(**(setup_config or {}))

        SessionCls = get_session(self.session_name)
        self.session = SessionCls(**self.session_config)
        self.session.env = self

        await self.session.start(config=self.setup_config, headless=self.headless)
        self.page = self.session.page

        self.bot = Bot(self)

    # --- Lifecycle API ---
    async def reset(
        self, task_id: Optional[int] = None, run_id: Optional[str] = None
    ) -> Tuple[bytes, Dict]:
        # Reset session state
        if self.session is not None:
            await self.session.close()
            self.session = None
            self.page = None

        # Reset step counter and telemetry tracking
        self.step_count = 0
        self._execution_start_time = time.time()
        self._run_id = run_id

        # Get tasks
        if self.tasks is None and self.tasks_config_fn is not None:
            self.tasks = self.tasks_config_fn()

        # Get current task
        if self.tasks is not None and self.current_task is None:
            self.current_task = self.tasks[0] if task_id is None else self.tasks[task_id]

        # Create sandbox from task config if provided
        if (
            self.current_task is not None
            and hasattr(self.current_task, "computer")
            and self.current_task.computer
        ):
            computer_config = self.current_task.computer
            provider = computer_config.get("provider", "webtop")
            setup_config = computer_config.get("setup_config", {})
            await self.create_sandbox(provider=provider, setup_config=setup_config)

        # Setup current task
        if self.current_task is not None and self.setup_task_fn is not None:
            try:
                await _call_function(self.setup_task_fn, self.current_task, self.session)
            except Exception as e:
                # Track setup failure
                if _telemetry_available:
                    track_task_execution_failed(
                        env_name=self.env_name or "unknown",
                        task_index=task_id or 0,
                        error_type=type(e).__name__,
                        error_message=str(e),
                        stage="setup",
                        run_id=self._run_id,
                    )
                raise

        # Track task execution started
        if _telemetry_available:
            provider_type = self.session_name if self.session_name else None
            os_type = self.setup_config.get("os_type") if self.setup_config else None
            track_task_execution_started(
                env_name=self.env_name or "unknown",
                task_index=task_id or 0,
                provider_type=provider_type,
                os_type=os_type,
                max_steps=self.max_steps,
                run_id=self._run_id,
            )

        # Return screenshot and task
        if self.session is None:
            raise RuntimeError(
                "create_sandbox was never called, please fix your task setup code to call env.create_sandbox() or add computer config to Task"
            )

        # Record reset event
        screenshot = await self.session.screenshot()
        if self.tracing is not None:
            # Try to capture a session snapshot of windows/state
            try:
                snapshot = await self.session.get_snapshot()  # type: ignore[attr-defined]
                snapshot_payload = asdict(snapshot)
            except Exception as e:
                import traceback

                snapshot_payload = {"error": repr(e), "traceback": traceback.format_exc()}

            # Include setup_config in the reset event for dataset processing
            self.tracing.record(
                "reset",
                {
                    "task": repr(self.current_task),
                    "snapshot": snapshot_payload,
                    "setup_config": dict(self.setup_config),
                },
                [screenshot],
            )

        return screenshot, self.current_task

    async def step(
        self, action: Action, dry_run: bool | Literal["before", "after"] = False
    ) -> bytes:
        # validate session
        if self.session is None:
            raise RuntimeError(
                "create_sandbox was never called, please call env.reset() or fix your task setup code"
            )

        # check for max steps
        if self.max_steps is not None and self.step_count >= self.max_steps:
            raise MaxStepsExceeded("Max steps exceeded")

        # record step:before
        if self.tracing is not None and dry_run in (False, "before"):
            before_screenshot = await self.session.screenshot()
            try:
                before_snapshot = await self.session.get_snapshot()  # type: ignore[attr-defined]
                before_snapshot_payload = asdict(before_snapshot)
            except Exception as e:
                import traceback

                before_snapshot_payload = {"error": repr(e), "traceback": traceback.format_exc()}
            self.tracing.record(
                "step:before",
                {
                    "action": repr(action),
                    "step_count": self.step_count,
                    "snapshot": before_snapshot_payload,
                },
                [before_screenshot],
            )
            if dry_run == "before":
                return before_screenshot

        # execute high-level action via session
        if not dry_run:
            if self.print_actions:
                print(f"Executing action: {action}")
            try:
                await self.session.execute_action(action)
            except Exception as e:
                # Track step failure
                if _telemetry_available:
                    track_task_execution_failed(
                        env_name=self.env_name or "unknown",
                        task_index=0,
                        error_type=type(e).__name__,
                        error_message=str(e),
                        stage="step",
                        run_id=self._run_id,
                    )
                raise

        # take screenshot, record trace event, and return
        screenshot = await self.session.screenshot()
        if self.tracing is not None and dry_run in (False, "after"):
            try:
                snapshot = await self.session.get_snapshot()  # type: ignore[attr-defined]
                snapshot_payload = asdict(snapshot)
            except Exception as e:
                import traceback

                snapshot_payload = {"error": repr(e), "traceback": traceback.format_exc()}
            self.tracing.record(
                "step:after",
                {
                    "action": repr(action),
                    "step_count": self.step_count,
                    "snapshot": snapshot_payload,
                },
                [screenshot],
            )

        # Increment step counter
        self.step_count += 1

        return screenshot

    async def solve(self) -> bytes:
        # validate state and solver
        if self.session is None and self.solve_task_fn is None:
            raise RuntimeError(
                "create_sandbox was never called, please call env.reset() or fix your task setup code"
            )
        if self.current_task is None:
            raise RuntimeError("No task is selected; call reset() first")
        if self.solve_task_fn is None:
            raise RuntimeError("No solve_task_fn provided")

        # solve task
        try:
            await _call_function(self.solve_task_fn, self.current_task, self.session)
        except MaxStepsExceeded:
            # Gracefully stop solving when the step budget is exhausted
            pass
        except Exception as e:
            # Track solve failure (not MaxStepsExceeded)
            if _telemetry_available:
                track_task_execution_failed(
                    env_name=self.env_name or "unknown",
                    task_index=0,
                    error_type=type(e).__name__,
                    error_message=str(e),
                    stage="solve",
                    run_id=self._run_id,
                )
            raise

        # validate session
        if self.session is None:
            raise RuntimeError(
                "create_sandbox was never called, please call env.reset() or fix your task setup code"
            )

        # return screenshot
        return await self.session.screenshot()

    async def evaluate(self) -> Any:
        # validate state and evaluator
        if self.current_task is None:
            raise RuntimeError("No task is selected; call reset() first")
        if self.evaluate_task_fn is None:
            raise RuntimeError("No evaluate_task_fn provided")

        # evaluate task, return score
        try:
            result = await _call_function(self.evaluate_task_fn, self.current_task, self.session)
        except Exception as e:
            # Track evaluate failure
            if _telemetry_available:
                track_task_execution_failed(
                    env_name=self.env_name or "unknown",
                    task_index=0,
                    error_type=type(e).__name__,
                    error_message=str(e),
                    stage="evaluate",
                    run_id=self._run_id,
                )
            raise

        if self.tracing is not None:
            self.tracing.record(
                "evaluate",
                {"result": result},
            )

        # Track task evaluation completed
        if _telemetry_available:
            duration = time.time() - self._execution_start_time if self._execution_start_time else 0
            success = False
            if isinstance(result, (int, float)):
                success = result >= 0.5  # Common threshold for success
            elif isinstance(result, bool):
                success = result
            elif isinstance(result, dict) and "success" in result:
                success = result["success"]

            track_task_evaluation_completed(
                env_name=self.env_name or "unknown",
                task_index=0,  # We don't track which variant was selected
                result=result,
                success=success,
                total_steps=self.step_count,
                duration_seconds=duration,
                run_id=self._run_id,
            )

        return result

    async def close(self) -> None:
        if self.session is not None:
            await self.session.close()
            self.session = None
            self.page = None
