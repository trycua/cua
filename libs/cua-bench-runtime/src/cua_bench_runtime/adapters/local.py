"""Headless local adapters used by Phase 2 conformance tests."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from cua_bench_runtime.adapters.agent_harnesses.system import ProductionSystemPlan
    from cua_bench_runtime.credential_lease import CredentialLease

from cua_bench_runtime.adapters import (
    EnvironmentAdapter,
    EvaluatorAdapter,
    HarnessAdapter,
    ObserverAdapter,
)
from cua_bench_runtime.adapters.cua_recording import CuaRecordingObserver
from cua_bench_runtime.canon import canonical_json, digest_file
from cua_bench_runtime.errors import (
    DeadlineExceeded,
    HardAbort,
    HarnessFailure,
    TrialInterrupted,
    ValidationFailure,
)
from cua_bench_runtime.model import (
    AgentOutcome,
    CleanupReport,
    EnvironmentHandle,
    Evaluation,
    ObserverReport,
    TrialContext,
)
from cua_bench_runtime.process import command_for, run_process
from cua_bench_runtime.signals import InterruptFlag


def _host_platform() -> str:
    if sys.platform == "darwin":
        return "macos"
    if os.name == "nt":
        return "windows"
    return "linux"


class LocalWorkspace(EnvironmentAdapter):
    name = "local"

    def __init__(self, *, fail_cleanup: bool = False) -> None:
        self.fail_cleanup = fail_cleanup

    def setup(self, context: TrialContext) -> EnvironmentHandle:
        workspace = context.artifacts / "workspace"
        workspace.mkdir()
        variant = context.task["variants"][0]
        payload = {
            "seed": context.config["seed"],
            "parameters": variant.get("parameters", {}),
        }
        (workspace / "input.json").write_bytes(canonical_json(payload) + b"\n")
        return EnvironmentHandle(
            kind=self.name,
            root=workspace,
            facts={
                "variant": str(variant["id"]),
                "platform": _host_platform(),
                "target_reset": True,
                "cache_state": "empty",
                "persistent_state": "absent",
            },
        )

    def cleanup(
        self,
        context: TrialContext,
        handle: EnvironmentHandle | None,
        timeout_seconds: float,
    ) -> CleanupReport:
        del handle, timeout_seconds
        if self.fail_cleanup:
            return CleanupReport(ok=False, error="injected cleanup failure")
        marker = context.artifacts / "cleanup.json"
        marker.write_bytes(canonical_json({"ok": True}) + b"\n")
        return CleanupReport(ok=True)

    def export(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome,
    ) -> EnvironmentHandle:
        del context, outcome
        return handle


class TaskLocalSmokeWorkspace(EnvironmentAdapter):
    """Run a task's pinned reset contract without claiming evaluator isolation.

    This adapter exists for maintainer desktop smokes on one host. The runtime
    records the trial as non-certifying because the agent and evaluator share
    an OS account. Official runs need an environment that enforces the task's
    declared evaluator staging and network policy.
    """

    name = "task-local-smoke"

    def __init__(self, bundle_root: Path) -> None:
        self.bundle_root = bundle_root.resolve()

    def _entrypoint(self, relative: str) -> Path:
        command = (self.bundle_root / relative).resolve()
        if not command.is_relative_to(self.bundle_root):
            raise ValidationFailure(f"task lifecycle entrypoint escapes bundle: {relative}")
        if not command.is_file():
            raise HarnessFailure(f"task lifecycle entrypoint is missing: {relative}")
        return command

    def _run_lifecycle(
        self,
        context: TrialContext,
        role: str,
        command: Path,
        workspace: Path,
        timeout_seconds: float,
    ) -> dict:
        stdout = context.artifacts / f"{role}.stdout"
        stderr = context.artifacts / f"{role}.stderr"
        argv = command_for(command, ["--workspace", str(workspace)])
        context.emit("process_spawned", {"role": role, "argv0": command.name})
        try:
            completed = subprocess.run(
                argv,
                cwd=self.bundle_root,
                capture_output=True,
                timeout=timeout_seconds,
                check=False,
            )
        except subprocess.TimeoutExpired as error:
            raise HarnessFailure(f"{role} exceeded {timeout_seconds:g} seconds") from error
        stdout.write_bytes(completed.stdout[-65536:])
        stderr.write_bytes(completed.stderr[-65536:])
        context.emit(
            "process_exited",
            {
                "role": role,
                "exit_code": completed.returncode,
                "truncated": len(completed.stdout) > 65536 or len(completed.stderr) > 65536,
            },
        )
        if completed.returncode != 0:
            tail = completed.stderr[-1000:].decode("utf-8", errors="replace").strip()
            raise HarnessFailure(f"{role} exited with {completed.returncode}: {tail}")
        try:
            return json.loads(completed.stdout.decode("utf-8").splitlines()[-1])
        except (IndexError, UnicodeDecodeError, json.JSONDecodeError):
            return {}

    def setup(self, context: TrialContext) -> EnvironmentHandle:
        workspace = context.artifacts / "workspace"
        reset = context.task["reset"]
        setup = self._entrypoint(str(reset["entrypoint"]))
        verify = self._entrypoint(str(reset["verification"]["entrypoint"]))
        setup_facts = self._run_lifecycle(context, "reset_setup", setup, workspace, 60.0)
        verify_facts = self._run_lifecycle(
            context,
            "reset_verify",
            verify,
            workspace,
            float(reset["verification"]["timeout_seconds"]),
        )
        return EnvironmentHandle(
            kind=self.name,
            root=workspace,
            facts={
                "variant": str(context.task["variants"][0]["id"]),
                "platform": _host_platform(),
                "reset_verified": verify_facts.get("ok") is True,
                "target_reset": verify_facts.get("ok") is True,
                "cache_state": "empty",
                "persistent_state": "absent",
                **{
                    key: value
                    for key, value in setup_facts.items()
                    if key not in {"ok", "workspace"}
                },
                "certifying": False,
            },
        )

    def cleanup(
        self,
        context: TrialContext,
        handle: EnvironmentHandle | None,
        timeout_seconds: float,
    ) -> CleanupReport:
        del handle, timeout_seconds
        marker = context.artifacts / "cleanup.json"
        marker.write_bytes(canonical_json({"ok": True, "workspace_preserved": True}) + b"\n")
        return CleanupReport(ok=True)

    def export(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome,
    ) -> EnvironmentHandle:
        del context, outcome
        return handle


class SubprocessHarness(HarnessAdapter):
    name = "subprocess"

    def __init__(self, command: Path, expected_digest: str) -> None:
        self.command = command.resolve()
        self.expected_digest = expected_digest

    def run(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> AgentOutcome:
        if digest_file(self.command) != self.expected_digest:
            raise ValidationFailure("materialized agent command digest changed")
        stdout = context.artifacts / "agent.stdout"
        stderr = context.artifacts / "agent.stderr"
        argv = command_for(
            self.command,
            ["--workspace", str(handle.root), "--artifacts", str(context.artifacts)],
        )
        context.emit("process_spawned", {"role": "agent", "argv0": self.command.name})
        try:
            process = run_process(
                argv,
                cwd=context.harness_workspace,
                stdout_path=stdout,
                stderr_path=stderr,
                timeout_seconds=timeout_seconds,
                interrupt=interrupt,
            )
        except (DeadlineExceeded, TrialInterrupted, HardAbort) as error:
            context.emit(
                "process_exited",
                {"role": "agent", "exit_code": None, "reason": error.status},
            )
            raise
        context.emit(
            "process_exited",
            {
                "role": "agent",
                "exit_code": process.returncode,
                "truncated": process.truncated,
                "output_exceeded": process.output_exceeded,
            },
        )
        outputs = tuple(
            path.relative_to(context.artifacts).as_posix()
            for path in sorted(
                context.artifacts.rglob("*"),
                key=lambda candidate: candidate.relative_to(context.artifacts).as_posix(),
            )
            if path.is_file()
        )
        return AgentOutcome(
            completed=process.returncode == 0,
            exit_code=process.returncode,
            duration_ms=process.duration_ms,
            artifacts=outputs,
        )


class ExternalEvaluator(EvaluatorAdapter):
    name = "external"

    def __init__(
        self,
        command: Path,
        expected_digest: str,
        *,
        node_path: Path | None = None,
        node_sha256: str | None = None,
        node_version: str | None = None,
    ) -> None:
        self.command = command.resolve()
        self.expected_digest = expected_digest
        node_contract = (node_path, node_sha256, node_version)
        if any(value is not None for value in node_contract) and not all(
            value is not None for value in node_contract
        ):
            raise ValueError("evaluator Node path, SHA-256, and version are required together")
        self.node_path = node_path
        self.node_sha256 = node_sha256
        self.node_version = node_version

    def evaluate(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> Evaluation:
        if digest_file(self.command) != self.expected_digest:
            raise ValidationFailure("materialized evaluator command digest changed")
        evaluator_dir = context.trial_dir / "evaluator"
        evaluator_dir.mkdir()
        result_path = evaluator_dir / "evaluation.json"
        stdout = evaluator_dir / "stdout"
        stderr = evaluator_dir / "stderr"
        evaluator_args = [
            "--workspace",
            str(handle.root),
            "--artifacts",
            str(context.artifacts),
            "--result",
            str(result_path),
            "--agent-exit-code",
            str(outcome.exit_code if outcome.exit_code is not None else -1),
        ]
        if self.node_path is not None:
            evaluator_args.extend(
                [
                    "--node",
                    str(self.node_path),
                    "--node-sha256",
                    str(self.node_sha256),
                    "--node-version",
                    str(self.node_version),
                ]
            )
        argv = command_for(self.command, evaluator_args)
        context.emit("process_spawned", {"role": "evaluator", "argv0": self.command.name})
        try:
            process = run_process(
                argv,
                cwd=handle.root,
                stdout_path=stdout,
                stderr_path=stderr,
                timeout_seconds=timeout_seconds,
                interrupt=interrupt,
            )
        except (DeadlineExceeded, TrialInterrupted, HardAbort) as error:
            context.emit(
                "process_exited",
                {"role": "evaluator", "exit_code": None, "reason": error.status},
            )
            raise
        context.emit(
            "process_exited",
            {
                "role": "evaluator",
                "exit_code": process.returncode,
                "truncated": process.truncated,
                "output_exceeded": process.output_exceeded,
            },
        )
        if process.output_exceeded:
            raise HarnessFailure("evaluator output exceeded the capture limit")
        if process.returncode != 0:
            raise HarnessFailure(f"evaluator exited with {process.returncode}")
        try:
            document = json.loads(
                result_path.read_text(encoding="utf-8"),
                parse_constant=lambda value: (_ for _ in ()).throw(
                    ValueError(f"invalid JSON constant: {value}")
                ),
            )
            if not isinstance(document["passed"], bool):
                raise TypeError("passed must be boolean")
            score = document.get("score")
            if score is not None and not isinstance(score, (int, float)):
                raise TypeError("score must be numeric or null")
            detail = document.get("detail", {})
            if not isinstance(detail, dict):
                raise TypeError("detail must be an object")
        except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
            raise HarnessFailure(f"invalid evaluator result: {error}") from error
        return Evaluation(bool(document["passed"]), score, detail)


class NoParticipationObserver(ObserverAdapter):
    """Explicitly records that local adapters have no trusted driver proxy."""

    name = "none"

    def start(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        requirements: tuple[dict, ...],
    ) -> None:
        del context, handle, requirements

    def finish(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome | None,
    ) -> ObserverReport:
        del context, handle, outcome
        return ObserverReport(
            name=self.name,
            trust="unavailable",
            detail="the local adapter has no benchmark-owned driver observer",
        )


def adapters(
    environment_name: str,
    agent_command: Path,
    agent_digest: str,
    evaluator_command: Path,
    evaluator_digest: str,
    *,
    lume_config_path: Path | None = None,
    guest_launch_path: Path | None = None,
    production_plan: ProductionSystemPlan | None = None,
    credential_lease: CredentialLease | None = None,
    debug_mode: bool = False,
    evaluator_node_path: Path | None = None,
    evaluator_node_sha256: str | None = None,
    evaluator_node_version: str | None = None,
) -> tuple[
    EnvironmentAdapter,
    HarnessAdapter,
    ObserverAdapter,
    EvaluatorAdapter,
]:
    guest_harness: HarnessAdapter | None = None
    if environment_name in {"local", "local-cua-smoke"}:
        environment = LocalWorkspace()
    elif environment_name == "local-fail-cleanup":
        environment = LocalWorkspace(fail_cleanup=True)
    elif environment_name in {"task-local-smoke", "task-local-cua-smoke"}:
        environment = TaskLocalSmokeWorkspace(evaluator_command.parents[1])
    elif environment_name in {"lume-macos", "lume-macos-certifying"}:
        if lume_config_path is None or guest_launch_path is None:
            raise HarnessFailure("lume-macos requires frozen environment and launch configs")
        from cua_bench_runtime.adapters.lume_macos import (
            LumeGuestHarness,
            LumeMacosEnvironment,
            build_control,
            load_lume_settings,
        )
        from cua_bench_runtime.guest_launch import load_guest_launch

        settings = load_lume_settings(lume_config_path)
        launch = load_guest_launch(guest_launch_path)
        environment = LumeMacosEnvironment(
            build_control(settings),
            launch,
            agent_command,
            certifying=environment_name == "lume-macos-certifying",
            **({"production_plan": production_plan} if production_plan is not None else {}),
            debug_mode=debug_mode,
        )
        guest_harness = LumeGuestHarness(
            environment,
            agent_command,
            agent_digest,
            **({"credential_lease": credential_lease} if production_plan is not None else {}),
        )
    else:
        raise HarnessFailure(f"unknown environment adapter: {environment_name}")
    if environment_name in {"local-cua-smoke", "task-local-cua-smoke"}:
        observer: ObserverAdapter = CuaRecordingObserver()
    elif environment_name == "lume-macos-certifying":
        from cua_bench_runtime.adapters.protected_cua import ProtectedCuaMediatorObserver

        observer = ProtectedCuaMediatorObserver(environment)
    else:
        observer = NoParticipationObserver()
    return (
        environment,
        guest_harness or SubprocessHarness(agent_command, agent_digest),
        observer,
        ExternalEvaluator(
            evaluator_command,
            evaluator_digest,
            node_path=evaluator_node_path,
            node_sha256=evaluator_node_sha256,
            node_version=evaluator_node_version,
        ),
    )


def adapter_names(environment_name: str) -> tuple[str, str]:
    if environment_name in {
        "local",
        "local-fail-cleanup",
        "local-cua-smoke",
        "task-local-smoke",
        "task-local-cua-smoke",
    }:
        return "subprocess", "external"
    if environment_name in {"lume-macos", "lume-macos-certifying"}:
        return "lume-guest", "external"
    # Tests and private apparatus adapters patch the constructor at the engine
    # boundary while preserving the legacy local adapter names.
    return "subprocess", "external"
