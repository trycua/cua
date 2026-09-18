"""Trial lifecycle engine."""

from __future__ import annotations

import json
import os
import re
import signal
import stat
import subprocess
import threading
import time
from contextlib import contextmanager
from dataclasses import asdict
from enum import Enum
from pathlib import Path
from typing import Any

from cua_bench_runtime import __version__, exit_codes
from cua_bench_runtime.adapters.local import adapter_names, adapters
from cua_bench_runtime.adapters.agent_harnesses.system import (
    ProductionSystemError,
    ProductionSystemPlan,
    production_system_plan,
)
from cua_bench_runtime.canon import canonical_json, digest_file, digest_json, sha256_bytes
from cua_bench_runtime.certification import (
    build_certification_receipt,
    verify_certification_receipt,
)
from cua_bench_runtime.clock import Clock
from cua_bench_runtime.credential_lease import (
    CredentialLease,
    CredentialLeaseError,
    MAX_SECRET_BYTES,
)
from cua_bench_runtime.errors import (
    BudgetExceeded,
    CbError,
    DeadlineExceeded,
    HardAbort,
    HarnessFailure,
    TrialInterrupted,
    UsageFailure,
    ValidationFailure,
)
from cua_bench_runtime.events import EventLog
from cua_bench_runtime.model import (
    AgentOutcome,
    CleanupReport,
    EnvironmentHandle,
    Evaluation,
    ObserverReport,
    TrialContext,
)
from cua_bench_runtime.participation import evaluate_participation
from cua_bench_runtime.policy import ExecutionPolicyController, certification_integrity
from cua_bench_runtime.receipt_signing import (
    key_id,
    sign_certification_receipt,
    sign_receipt,
    verify_certification_signature,
    verify_receipt,
)
from cua_bench_runtime.schemas import validate_manifest
from cua_bench_runtime.signals import InterruptFlag
from cua_bench_runtime.trial import materialize_trial, write_result
from cua_bench_runtime.guest_launch import load_guest_launch


class State(str, Enum):
    INIT = "init"
    VALIDATED = "validated"
    MATERIALIZED = "materialized"
    SETUP = "setup"
    AGENT = "agent"
    EXPORT = "export"
    EVALUATION = "evaluation"
    CLEANUP = "cleanup"
    COLLECT = "collect"
    DONE = "done"


ALLOWED = {
    State.INIT: {State.VALIDATED},
    State.VALIDATED: {State.MATERIALIZED},
    State.MATERIALIZED: {State.SETUP, State.CLEANUP},
    State.SETUP: {State.AGENT, State.CLEANUP},
    State.AGENT: {State.EXPORT, State.CLEANUP},
    State.EXPORT: {State.EVALUATION, State.CLEANUP},
    State.EVALUATION: {State.CLEANUP},
    State.CLEANUP: {State.COLLECT},
    State.COLLECT: {State.DONE},
    State.DONE: set(),
}

TRIAL_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
CERTIFYING_ENVIRONMENT_ADAPTERS: frozenset[str] = frozenset({"lume-macos-certifying"})
SHA256_DIGEST = re.compile(r"^sha256:[a-f0-9]{64}$")
MAX_AGENT_BRIEF_BYTES = 1024 * 1024
HARD_ABORT_CLEANUP_TIMEOUT_SECONDS = 1.0
MAX_HARD_ABORT_CLEANUP_TIMEOUT_SECONDS = 30.0


class _EmergencyCleanupTimeout(BaseException):
    """Escape adapter exception boundaries when hard-abort cleanup overruns."""


def _scrub_bytearray(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


@contextmanager
def _destroy_credential_lease_on_exit(credential_lease: CredentialLease | None):
    """Scrub a production credential lease when its lifecycle scope exits."""

    destroyed = False

    def destroy() -> None:
        nonlocal destroyed
        if destroyed or credential_lease is None:
            return
        destroyed = True
        credential_lease.destroy()

    try:
        yield destroy
    finally:
        destroy()


def _bounded_emergency_cleanup(
    environment: Any,
    context: TrialContext,
    handle: EnvironmentHandle | None,
) -> None:
    """Give hard-abort cleanup one short best-effort window."""

    requested_timeout = getattr(environment, "hard_abort_cleanup_timeout_seconds", None)
    if not isinstance(requested_timeout, (int, float)):
        requested_timeout = HARD_ABORT_CLEANUP_TIMEOUT_SECONDS
    cleanup_timeout = getattr(environment, "cleanup_timeout_seconds", None)
    if isinstance(cleanup_timeout, (int, float)):
        requested_timeout = min(requested_timeout, cleanup_timeout)
    timeout_seconds = min(
        MAX_HARD_ABORT_CLEANUP_TIMEOUT_SECONDS,
        max(0.001, float(requested_timeout)),
    )
    alarm = getattr(signal, "SIGALRM", None)
    set_timer = getattr(signal, "setitimer", None)
    get_timer = getattr(signal, "getitimer", None)
    timer_kind = getattr(signal, "ITIMER_REAL", None)
    if alarm is None or not callable(set_timer) or not callable(get_timer):
        # Windows has no equivalent interruptible in-process timer.  Run cleanup
        # in a daemon thread and wait only for the hard-abort window: a hung or
        # failing adapter cannot delay or replace the original HardAbort.
        def cleanup_in_background() -> None:
            try:
                environment.cleanup(context, handle, timeout_seconds=timeout_seconds)
            except BaseException:
                pass

        thread = threading.Thread(target=cleanup_in_background, daemon=True)
        try:
            thread.start()
            thread.join(timeout_seconds)
        except (RuntimeError, OSError):
            # Thread startup and joining are also best effort during HardAbort.
            pass
        return

    def deadline(_signum: int, _frame: object) -> None:
        raise _EmergencyCleanupTimeout()

    try:
        previous_handler = signal.getsignal(alarm)
        previous_timer = get_timer(timer_kind)
        signal.signal(alarm, deadline)
    except (OSError, TypeError, ValueError):
        # signal handlers can only be installed from the main thread.  Preserve
        # the hard-abort bound when that precondition is unavailable.
        return
    started = time.monotonic()
    try:
        set_timer(timer_kind, timeout_seconds)
        environment.cleanup(context, handle, timeout_seconds=timeout_seconds)
    finally:
        set_timer(timer_kind, 0.0)
        signal.signal(alarm, previous_handler)
        previous_delay, previous_interval = previous_timer
        if previous_delay > 0.0:
            elapsed = time.monotonic() - started
            set_timer(
                timer_kind,
                max(0.000001, previous_delay - elapsed),
                previous_interval,
            )


def _credential_lease_from_file(
    path: Path,
    *,
    plan: ProductionSystemPlan,
    agent_limit: float,
) -> CredentialLease:
    """Read one host credential without following or persisting its path."""

    get_effective_uid = getattr(os, "geteuid", None)
    nofollow = getattr(os, "O_NOFOLLOW", None)
    if not callable(get_effective_uid) or nofollow is None:
        raise UsageFailure("secure credential file reads are unavailable")
    runtime_uid = get_effective_uid()
    if not path.is_absolute():
        raise UsageFailure("credential file path must be absolute")
    if len(plan.credential_environment) != 1:
        raise UsageFailure("credential file requires exactly one credential environment name")
    try:
        before = path.lstat()
    except OSError as error:
        raise UsageFailure("credential file is not readable") from error
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise UsageFailure("credential file must be a regular non-symlink file")
    if before.st_uid != runtime_uid:
        raise UsageFailure("credential file must be owned by the runtime user")
    if stat.S_IMODE(before.st_mode) != 0o600:
        raise UsageFailure("credential file must have mode 0600")
    if before.st_nlink != 1:
        raise UsageFailure("credential file must have exactly one link")
    if before.st_size > MAX_SECRET_BYTES:
        raise UsageFailure("credential file exceeds the size limit")
    secret = bytearray()
    descriptor: int | None = None
    try:
        descriptor = os.open(path, os.O_RDONLY | nofollow)
        after = os.fstat(descriptor)
        if (
            not stat.S_ISREG(after.st_mode)
            or (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino)
            or after.st_uid != runtime_uid
            or stat.S_IMODE(after.st_mode) != 0o600
            or after.st_nlink != 1
        ):
            raise UsageFailure("credential file changed during secure open")
        while len(secret) <= MAX_SECRET_BYTES:
            chunk = os.read(descriptor, min(4096, MAX_SECRET_BYTES + 1 - len(secret)))
            if not chunk:
                break
            secret.extend(chunk)
        if len(secret) > MAX_SECRET_BYTES:
            raise UsageFailure("credential file exceeds the size limit")
        final = os.fstat(descriptor)
        if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != (
            final.st_dev,
            final.st_ino,
            final.st_size,
            final.st_mtime_ns,
        ) or final.st_nlink != 1:
            raise UsageFailure("credential file changed during secure read")
        return CredentialLease(
            provider=plan.model_route.provider,
            harness=plan.kind.value,
            credentials={plan.credential_environment[0]: secret},
            expires_at=time.time() + agent_limit + 300.0,
            policy_digest=plan.policy_digest,
        )
    except CredentialLeaseError as error:
        raise UsageFailure("credential file does not contain a valid secret") from error
    except OSError as error:
        raise UsageFailure("credential file could not be read securely") from error
    finally:
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError:
                pass
        _scrub_bytearray(secret)


def _validate_production_credential_option(
    plan: ProductionSystemPlan | None, credential_file_path: Path | None
) -> None:
    if plan is not None and plan.credential_environment and credential_file_path is None:
        raise UsageFailure("production systems require --credential-file")
    if plan is not None and not plan.credential_environment and credential_file_path is not None:
        raise UsageFailure("credential file is forbidden for an anonymous production route")
    if plan is None and credential_file_path is not None:
        raise UsageFailure("credential file is only valid with a production system")


def _validate_agent_brief_option(
    path: Path | None,
    *,
    apparatus_check_requested: bool,
    environment_name: str,
    production_plan: ProductionSystemPlan | None,
) -> None:
    if path is None:
        return
    if not apparatus_check_requested:
        raise UsageFailure("agent brief override requires --apparatus-check")
    if environment_name not in {"lume-macos", "lume-macos-certifying"}:
        raise UsageFailure("agent brief override is only valid with production Lume")
    if production_plan is None:
        raise UsageFailure("agent brief override is only valid with a production system")


def _read_agent_brief(path: Path) -> bytes:
    """Capture one host-owned brief without following a mutable path."""

    get_effective_uid = getattr(os, "geteuid", None)
    nofollow = getattr(os, "O_NOFOLLOW", None)
    if not callable(get_effective_uid) or nofollow is None:
        raise UsageFailure("secure agent brief reads are unavailable")
    try:
        before = path.lstat()
    except OSError as error:
        raise UsageFailure("agent brief is not readable") from error
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise UsageFailure("agent brief must be a regular non-symlink file")
    if before.st_uid != get_effective_uid():
        raise UsageFailure("agent brief must be owned by the runtime user")
    if before.st_nlink != 1:
        raise UsageFailure("agent brief must have exactly one link")
    if before.st_size > MAX_AGENT_BRIEF_BYTES:
        raise UsageFailure("agent brief exceeds the size limit")
    descriptor: int | None = None
    try:
        descriptor = os.open(path, os.O_RDONLY | nofollow)
        opened = os.fstat(descriptor)
        if (
            not stat.S_ISREG(opened.st_mode)
            or (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino)
            or opened.st_uid != get_effective_uid()
            or opened.st_nlink != 1
        ):
            raise UsageFailure("agent brief changed during secure open")
        content = bytearray()
        while len(content) <= MAX_AGENT_BRIEF_BYTES:
            chunk = os.read(
                descriptor,
                min(65536, MAX_AGENT_BRIEF_BYTES + 1 - len(content)),
            )
            if not chunk:
                break
            content.extend(chunk)
        if len(content) > MAX_AGENT_BRIEF_BYTES:
            raise UsageFailure("agent brief exceeds the size limit")
        final = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns) != (
            final.st_dev,
            final.st_ino,
            final.st_size,
            final.st_mtime_ns,
        ) or final.st_nlink != 1:
            raise UsageFailure("agent brief changed during secure read")
        return bytes(content)
    except OSError as error:
        raise UsageFailure("agent brief could not be read securely") from error
    finally:
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError:
                pass


def _has_certifying_environment_evidence(
    environment_name: str,
    handle: EnvironmentHandle | None,
    cleanup: CleanupReport,
) -> bool:
    if environment_name != "lume-macos-certifying":
        return True
    facts = handle.facts if handle is not None else {}
    return bool(
        cleanup.ok
        and facts.get("certifying") is True
        and facts.get("human_input_channel") == "closed-no-vnc"
        and facts.get("task_store_mode") == "protected-console-only"
        and facts.get("mediator_enforcer") == "guest-root-cdb-helper"
        and facts.get("vm_stopped_before_collection") is True
        and facts.get("protected_collection_read_only") is True
        and SHA256_DIGEST.fullmatch(str(facts.get("protected_log_digest")))
    )


def _manifest_input(
    manifest_path: Path, artifact: dict[str, str], namespace: str
) -> tuple[Path, str]:
    source = (manifest_path.parent / artifact["path"]).resolve()
    if not source.is_relative_to(manifest_path.parent.resolve()):
        raise ValidationFailure(
            f"{namespace} artifact escapes manifest directory: {artifact['path']}"
        )
    if digest_file(source).removeprefix("sha256:") != artifact["sha256"]:
        raise ValidationFailure(f"{namespace} artifact digest mismatch: {artifact['path']}")
    return source, f"{namespace}-artifacts/{artifact['path']}"


def _system_policy_inputs(
    system_path: Path,
    system: dict[str, Any],
    policy_path: Path,
    policy: dict[str, Any],
) -> tuple[tuple[Path, str], ...]:
    declarations = (
        *(
            _manifest_input(system_path, artifact, "system")
            for artifact in (
                system["harness"]["build"],
                system["harness"]["configuration"],
                system["capability_inventory"]["tools"],
                system["capability_inventory"]["skills"],
            )
        ),
        _manifest_input(policy_path, policy["accounting"]["model_price_table"], "policy"),
        _manifest_input(policy_path, policy["credential_state_profile"], "policy"),
    )
    return (
        (system_path, "system.cuabench.json"),
        (policy_path, "execution-policy.cuabench.json"),
        *declarations,
    )


def _mutated_inputs(trial_dir: Path, config_digest: str, inputs_manifest_digest: str) -> list[str]:
    mutations: list[str] = []
    config_path = trial_dir / "config.json"
    try:
        if digest_json(json.loads(config_path.read_text(encoding="utf-8"))) != config_digest:
            mutations.append("config.json")
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        mutations.append("config.json")
    manifest_path = trial_dir / "inputs.manifest.json"
    if not manifest_path.is_file() or digest_file(manifest_path) != inputs_manifest_digest:
        mutations.append("inputs.manifest.json")
        return mutations
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    input_root = (trial_dir / "inputs").resolve()
    for relative, expected in manifest["files"].items():
        path = (input_root / relative).resolve()
        if (
            not path.is_relative_to(input_root)
            or not path.is_file()
            or digest_file(path) != expected
        ):
            mutations.append(relative)
    return mutations


def _write_fresh_signed_artifact(path: Path, artifact: dict[str, Any]) -> None:
    """Create one host-owned signature artifact without following an agent path."""

    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags, 0o644)
    except OSError as error:
        raise HarnessFailure("signature artifact path is not fresh") from error
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(canonical_json(artifact) + b"\n")
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        if path.is_file() and not path.is_symlink():
            path.unlink(missing_ok=True)
        raise


def _certification_apparatus(
    *,
    environment_name: str,
    environment_declared_certifying: bool,
    apparatus_check: bool,
    handle: EnvironmentHandle | None,
    cleanup: CleanupReport,
    inputs_unchanged: bool,
    evaluation: Evaluation | None,
    participation: dict[str, Any],
    participation_signature: dict[str, Any] | None,
    policy_receipt: dict[str, Any] | None,
) -> dict[str, Any]:
    facts = dict(handle.facts) if handle is not None else {}
    policy_integrity = certification_integrity(policy_receipt)
    return {
        "environment_adapter": environment_name,
        "environment_declared_certifying": environment_declared_certifying,
        "apparatus_check": apparatus_check,
        "cleanup_ok": cleanup.ok,
        "seed_provenance_digest": facts.get("seed_provenance_digest"),
        "pristine_fingerprint": facts.get("pristine_fingerprint"),
        "post_run_fingerprint": facts.get("post_run_fingerprint"),
        "post_run_matches_pristine": facts.get("post_run_matches_pristine"),
        "initial_driver_identity_digest": facts.get("initial_driver_identity_digest"),
        "post_run_driver_identity_digest": facts.get("post_run_driver_identity_digest"),
        "fresh_harness_workspace": facts.get("fresh_harness_workspace"),
        "target_reset": bool(
            facts.get("target_reset") is True and facts.get("reset_verified") is True
        ),
        "task_store_relative": facts.get("task_store_relative"),
        "agent_processes_frozen": facts.get("agent_processes_frozen"),
        "network_mode": facts.get("applied_network_mode"),
        "network_enforcer": facts.get("network_enforcer"),
        "network_evidence_digest": facts.get("network_evidence_digest"),
        "post_run_network_evidence_digest": facts.get("post_run_network_evidence_digest"),
        "production_harness": facts.get("production_harness"),
        "provider_proxy": facts.get("provider_proxy"),
        "human_input_channel": facts.get("human_input_channel"),
        "human_input_enforcer": facts.get("human_input_enforcer"),
        "human_input_evidence_digest": facts.get("human_input_evidence_digest"),
        "post_run_human_input_evidence_digest": facts.get("post_run_human_input_evidence_digest"),
        "protected_endpoint_binding_digest": facts.get("protected_endpoint_binding_digest"),
        "sealed_endpoint_binding_digest": facts.get("sealed_endpoint_binding_digest"),
        "vm_stopped_before_collection": facts.get("vm_stopped_before_collection"),
        "protected_collection_read_only": facts.get("protected_collection_read_only"),
        "collection_manifest_digest": facts.get("collection_manifest_digest"),
        "protected_collection_manifest_digest": facts.get("protected_collection_manifest_digest"),
        "protected_log_digest": facts.get("protected_log_digest"),
        "protected_report_digest": facts.get("protected_report_digest"),
        "protected_tool_contract_validated": (
            facts.get("protected_tool_contract_validated") is True
        ),
        "observed_daemon_tool_schemas_sha256": facts.get("observed_daemon_tool_schemas_sha256"),
        "expected_daemon_tool_list_envelope_sha256": facts.get(
            "expected_daemon_tool_list_envelope_sha256"
        ),
        "observed_daemon_tool_list_envelope_sha256": facts.get(
            "observed_daemon_tool_list_envelope_sha256"
        ),
        "protected_daemon_tool_list_envelope_validated": (
            facts.get("protected_daemon_tool_list_envelope_validated") is True
        ),
        "protected_log_tail": facts.get("protected_log_tail"),
        "protected_log_records": facts.get("protected_log_records"),
        "protected_transport_integrity": facts.get("protected_transport_integrity"),
        "protected_evidence_complete": facts.get("protected_evidence_complete"),
        "protected_off_target_activity": facts.get("protected_off_target_activity"),
        "inputs_unchanged": inputs_unchanged,
        "evaluation_digest": (digest_json(asdict(evaluation)) if evaluation is not None else None),
        "participation_receipt_digest": participation.get("receipt_digest"),
        "participation_signature_digest": (
            participation_signature.get("digest") if participation_signature is not None else None
        ),
        "execution_policy_receipt_digest": (
            policy_receipt.get("receipt_digest") if policy_receipt is not None else None
        ),
        "execution_policy_integrity_passed": policy_integrity["passed"],
        "execution_policy_integrity_violations": policy_integrity["violations"],
    }


class StateMachine:
    def __init__(self, events: EventLog) -> None:
        self.state = State.MATERIALIZED
        self.events = events

    def enter(self, target: State) -> None:
        if target not in ALLOWED[self.state]:
            raise HarnessFailure(f"illegal lifecycle transition: {self.state} -> {target}")
        self.events.append("state_exit", {"state": self.state.value}, sync=True)
        self.state = target
        self.events.append("state_enter", {"state": target.value}, sync=True)


def _default_trial_id(task: dict[str, Any], seed: int) -> str:
    stable = digest_json({"task": task["id"], "version": task["version"], "seed": seed})
    slug = str(task["id"]).split(".")[-1].replace("_", "-")
    return f"{slug}-{seed}-{stable[-10:]}"


def _validate_evaluator_node_contract(
    path: Path, expected_sha256: str, expected_version: str
) -> None:
    if not path.is_absolute():
        raise UsageFailure("evaluator Node must be an absolute path")
    if re.fullmatch(r"[a-f0-9]{64}", expected_sha256) is None:
        raise UsageFailure("evaluator Node SHA-256 must be 64 lowercase hex characters")
    version_match = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", expected_version)
    if version_match is None or int(version_match.group(1)) < 22:
        raise UsageFailure("evaluator Node version must be Node >=22 in vMAJOR.MINOR.PATCH form")
    try:
        status = path.lstat()
    except OSError as error:
        raise UsageFailure(f"evaluator Node is unavailable: {error}") from error
    if not stat.S_ISREG(status.st_mode) or not os.access(path, os.X_OK):
        raise UsageFailure("evaluator Node must be an executable, non-symlink regular file")
    actual_sha256 = digest_file(path).removeprefix("sha256:")
    if actual_sha256 != expected_sha256:
        raise UsageFailure(
            f"evaluator Node SHA-256 mismatch: expected {expected_sha256}, got {actual_sha256}"
        )
    try:
        completed = subprocess.run(
            [str(path), "--version"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise UsageFailure(f"evaluator Node version check failed: {error}") from error
    actual_version = completed.stdout.strip()
    if completed.returncode != 0 or actual_version != expected_version:
        raise UsageFailure(
            f"evaluator Node version mismatch: expected {expected_version}, got "
            f"{actual_version or '<no version>'}"
        )


def run_trial(
    *,
    task_path: Path,
    agent_command: Path,
    out: Path,
    trial_id: str | None = None,
    seed: int = 0,
    timeout_seconds: float | None = None,
    environment_name: str = "local",
    system_path: Path | None = None,
    execution_policy_path: Path | None = None,
    participation_signing_key: Path | None = None,
    participation_verifier_key: Path | None = None,
    apparatus_check: bool = False,
    lume_config_path: Path | None = None,
    guest_launch_path: Path | None = None,
    credential_file_path: Path | None = None,
    agent_brief_path: Path | None = None,
    evaluator_node_path: Path | None = None,
    evaluator_node_sha256: str | None = None,
    evaluator_node_version: str | None = None,
    debug_mode: bool = False,
) -> tuple[int, Path, dict[str, Any]]:
    apparatus_check_requested = apparatus_check
    task_path = task_path.resolve()
    task = validate_manifest(task_path, "task")
    evaluator_node_values = (
        evaluator_node_path,
        evaluator_node_sha256,
        evaluator_node_version,
    )
    if any(value is not None for value in evaluator_node_values) and not all(
        value is not None for value in evaluator_node_values
    ):
        raise UsageFailure(
            "evaluator Node requires --evaluator-node, --evaluator-node-sha256, "
            "and --evaluator-node-version together"
        )
    if evaluator_node_path is not None:
        assert evaluator_node_sha256 is not None
        assert evaluator_node_version is not None
        _validate_evaluator_node_contract(
            evaluator_node_path, evaluator_node_sha256, evaluator_node_version
        )
    if (system_path is None) != (execution_policy_path is None):
        raise UsageFailure("system and execution policy must be supplied together")
    system: dict[str, Any] | None = None
    execution_policy: dict[str, Any] | None = None
    system_digest: str | None = None
    execution_policy_digest: str | None = None
    additional_inputs: tuple[tuple[Path, str], ...] = ()
    guest_launch = None
    provisional_production_plan: ProductionSystemPlan | None = None
    lume_environments = {"lume-macos", "lume-macos-certifying"}
    if credential_file_path is not None and environment_name not in lume_environments:
        raise UsageFailure("credential file is only valid with production Lume")
    if environment_name in lume_environments:
        if environment_name == "lume-macos":
            apparatus_check = True
        if lume_config_path is None or guest_launch_path is None:
            raise UsageFailure("lume-macos requires --lume-config and --guest-launch")
        lume_config_path = lume_config_path.resolve()
        guest_launch_path = guest_launch_path.resolve()
        if not lume_config_path.is_file():
            raise UsageFailure("Lume config is not a file")
        guest_launch = load_guest_launch(guest_launch_path)
    elif lume_config_path is not None or guest_launch_path is not None:
        raise UsageFailure("Lume configuration is only valid with a Lume macOS environment")
    if (participation_signing_key is None) != (participation_verifier_key is None):
        raise UsageFailure("participation signing and verifier keys must be supplied together")
    if system_path is not None and execution_policy_path is not None:
        system_path = system_path.resolve()
        execution_policy_path = execution_policy_path.resolve()
        system = validate_manifest(system_path, "system")
        execution_policy = validate_manifest(execution_policy_path, "execution-policy")
        if system["schema_version"] != "0.3.0" or execution_policy["schema_version"] != "0.3.0":
            raise ValidationFailure("system comparisons require v0.3.0 manifests")
        system_digest = digest_file(system_path)
        execution_policy_digest = digest_file(execution_policy_path)
        additional_inputs = _system_policy_inputs(
            system_path,
            system,
            execution_policy_path,
            execution_policy,
        )
        if agent_brief_path is not None or debug_mode:
            build_relative = system["harness"]["build"]["path"]
            configuration_relative = system["harness"]["configuration"]["path"]
            try:
                provisional_production_plan = production_system_plan(
                    system,
                    build_path=(system_path.parent / build_relative).resolve(),
                    configuration_path=(system_path.parent / configuration_relative).resolve(),
                    skill_inventory_path=(
                        system_path.parent / system["capability_inventory"]["skills"]["path"]
                    ).resolve(),
                    tool_inventory_path=(
                        system_path.parent / system["capability_inventory"]["tools"]["path"]
                    ).resolve(),
                )
            except ProductionSystemError as error:
                raise ValidationFailure(str(error)) from error
        if environment_name == "lume-macos" and not str(system["id"]).startswith("apparatus."):
            raise UsageFailure("non-certifying lume-macos runs require an apparatus.* system")
    if credential_file_path is not None and system is None:
        raise UsageFailure("credential file is only valid with a production system")
    if debug_mode:
        if environment_name != "lume-macos-certifying":
            raise UsageFailure(
                "debug mode requires the protected lume-macos-certifying environment"
            )
        if apparatus_check_requested:
            raise UsageFailure("debug mode cannot be combined with an apparatus check")
        if provisional_production_plan is None:
            raise UsageFailure("debug mode requires a production system")
    _validate_agent_brief_option(
        agent_brief_path,
        apparatus_check_requested=apparatus_check_requested,
        environment_name=environment_name,
        production_plan=provisional_production_plan,
    )
    agent_brief = _read_agent_brief(agent_brief_path) if agent_brief_path is not None else None
    if guest_launch is not None and guest_launch_path is not None:
        additional_inputs = (
            *additional_inputs,
            (lume_config_path, "apparatus/lume/config.json"),
            (guest_launch_path, "apparatus/launch/guest-launch.json"),
            *(
                (
                    (guest_launch_path.parent / artifact.path).resolve(),
                    f"apparatus/launch/{artifact.path}",
                )
                for artifact in guest_launch.build_artifacts
            ),
        )
    verifier_key_id: str | None = None
    if participation_signing_key is not None and participation_verifier_key is not None:
        participation_signing_key = participation_signing_key.resolve()
        participation_verifier_key = participation_verifier_key.resolve()
        if not participation_signing_key.is_file():
            raise UsageFailure("participation signing key is not a file")
        if not participation_verifier_key.is_file():
            raise UsageFailure("participation verifier key is not a file")
        verifier_key_id = key_id(participation_verifier_key)
        additional_inputs = (
            *additional_inputs,
            (participation_verifier_key, "apparatus/participation-verifier.pub"),
        )
    if (
        environment_name in {"local", "local-cua-smoke", "local-fail-cleanup"}
        and task["evaluator"]["staging"] != "agent-visible"
    ):
        raise ValidationFailure("local environment requires evaluator staging to be agent-visible")
    if (
        environment_name in lume_environments
        and task["evaluator"]["staging"] != "agent-inaccessible"
    ):
        raise ValidationFailure("lume-macos requires evaluator staging to be agent-inaccessible")
    agent_command = agent_command.resolve()
    if not agent_command.is_file():
        raise UsageFailure(f"agent command is not a file: {agent_command}")
    evaluator_command = (task_path.parent / task["evaluator"]["entrypoint"]).resolve()
    if not evaluator_command.is_relative_to(task_path.parent):
        raise ValidationFailure("evaluator entrypoint escapes task directory")
    if not evaluator_command.is_file():
        raise UsageFailure(f"evaluator command is not a file: {evaluator_command}")
    resolved_id = trial_id or _default_trial_id(task, seed)
    if not TRIAL_ID.fullmatch(resolved_id):
        raise UsageFailure(
            "trial id must be 1-64 ASCII letters, digits, dots, underscores, or hyphens"
        )
    if timeout_seconds is not None and timeout_seconds <= 0:
        raise UsageFailure("timeout must be greater than zero")
    agent_limit = float(
        task["limits"]["agent_seconds"] if timeout_seconds is None else timeout_seconds
    )
    if execution_policy is not None:
        agent_limit = min(agent_limit, execution_policy["limits"]["wall_time_ms"] / 1000.0)
    evaluator_limit = float(task["limits"]["evaluator_seconds"])
    output_schema_version = "0.3.0" if system is not None else task["schema_version"]
    participation_requirements = tuple(task.get("participation_requirements", ()))
    local_environments = {
        "local",
        "local-cua-smoke",
        "local-fail-cleanup",
        "task-local-smoke",
        "task-local-cua-smoke",
    }
    environment_certifying = (
        environment_name in {"local", "local-cua-smoke", "local-fail-cleanup"}
        if output_schema_version == "0.1.0"
        else environment_name in CERTIFYING_ENVIRONMENT_ADAPTERS
    )
    environment_certifying = environment_certifying and not apparatus_check
    signed_apparatus_check = environment_name == "lume-macos-certifying" and apparatus_check
    if participation_signing_key is not None:
        if task["schema_version"] == "0.1.0":
            raise UsageFailure("signed participation requires task schema 0.2.0 or newer")
        if not environment_certifying and not signed_apparatus_check:
            raise UsageFailure(
                "signed participation requires an explicitly certifying environment adapter"
            )
    if environment_name == "lume-macos-certifying" and (
        participation_signing_key is None or participation_verifier_key is None
    ):
        raise UsageFailure(
            "certifying Lume execution requires participation signing and verifier keys"
        )
    agent_adapter_name, evaluator_adapter_name = adapter_names(environment_name)
    config = {
        "schema_version": output_schema_version,
        "trial_id": resolved_id,
        "task": {
            "id": task["id"],
            "version": task["version"],
            "digest": digest_file(task_path),
        },
        "variant": task["variants"][0]["id"],
        "seed": seed,
        "environment_adapter": environment_name,
        "agent_adapter": agent_adapter_name,
        "evaluator_adapter": evaluator_adapter_name,
        "certifying": environment_certifying,
        "participation_required": bool(participation_requirements),
        "runtime": {"name": "cua-bench-runtime", "version": __version__},
        "agent_command": {
            "name": agent_command.name,
            "digest": digest_file(agent_command),
        },
        "evaluator_command": {
            "name": evaluator_command.name,
            "digest": digest_file(evaluator_command),
        },
        "limits": {"agent_seconds": agent_limit, "evaluator_seconds": evaluator_limit},
    }
    if evaluator_node_path is not None:
        config["evaluator_runtime"] = {
            "node": {
                "sha256": f"sha256:{evaluator_node_sha256}",
                "version": evaluator_node_version,
            }
        }
    if apparatus_check:
        config["apparatus_check"] = True
    if debug_mode:
        config["debug_mode"] = True
    if agent_brief is not None:
        config["agent_brief"] = {
            "path": "inputs/apparatus/agent-brief.md",
            "digest": sha256_bytes(agent_brief),
        }
    if guest_launch is not None and lume_config_path is not None:
        config["guest_launch"] = {
            "id": guest_launch.id,
            "digest": guest_launch.digest,
        }
        config["lume_configuration"] = {
            "name": lume_config_path.name,
            "digest": digest_file(lume_config_path),
        }
    if system is not None and execution_policy is not None:
        config["task_schema_version"] = task["schema_version"]
        config["system"] = {
            "id": system["id"],
            "version": system["version"],
            "digest": system_digest,
        }
        config["execution_policy"] = {
            "id": execution_policy["id"],
            "version": execution_policy["version"],
            "digest": execution_policy_digest,
        }
    if verifier_key_id is not None:
        config["participation_verifier"] = {
            "path": "inputs/apparatus/participation-verifier.pub",
            "digest": digest_file(participation_verifier_key),
            "key_id": verifier_key_id,
            "required": True,
        }
    (
        trial_dir,
        config_digest,
        inputs_manifest_digest,
        materialized_agent,
        materialized_evaluator,
    ) = materialize_trial(
        out,
        resolved_id,
        task_path,
        task,
        config,
        agent_command,
        additional_inputs,
        agent_brief=agent_brief,
    )
    production_plan: ProductionSystemPlan | None = None
    credential_lease: CredentialLease | None = None
    if system is not None:
        build_relative = system["harness"]["build"]["path"]
        configuration_relative = system["harness"]["configuration"]["path"]
        try:
            production_plan = production_system_plan(
                system,
                build_path=(trial_dir / "inputs/system-artifacts" / build_relative),
                configuration_path=(trial_dir / "inputs/system-artifacts" / configuration_relative),
                skill_inventory_path=(
                    trial_dir
                    / "inputs/system-artifacts"
                    / system["capability_inventory"]["skills"]["path"]
                ),
                tool_inventory_path=(
                    trial_dir
                    / "inputs/system-artifacts"
                    / system["capability_inventory"]["tools"]["path"]
                ),
            )
        except ProductionSystemError as error:
            raise ValidationFailure(str(error)) from error
        if (
            agent_brief_path is not None or debug_mode
        ) and production_plan != provisional_production_plan:
            raise ValidationFailure(
                "production system changed while trial inputs were materialized"
            )
    if production_plan is not None and environment_name not in lume_environments:
        raise UsageFailure("production systems require a Lume macOS environment")
    _validate_production_credential_option(production_plan, credential_file_path)
    event_path = trial_dir / "events.ndjson"
    clock = Clock()
    evaluation: Evaluation | None = None
    outcome: AgentOutcome | None = None
    handle: EnvironmentHandle | None = None
    cleanup = CleanupReport(ok=False, error="cleanup did not run")
    original_exit = exit_codes.OK
    status = "completed"
    error_detail: str | None = None
    policy_controller = (
        ExecutionPolicyController(
            trial_id=resolved_id,
            system=system,
            policy=execution_policy,
            model_price_table=json.loads(
                (
                    execution_policy_path.parent
                    / execution_policy["accounting"]["model_price_table"]["path"]
                ).read_text(encoding="utf-8")
            ),
            system_digest=system_digest,
            policy_digest=execution_policy_digest,
        )
        if system is not None
        and execution_policy is not None
        and system_digest is not None
        and execution_policy_digest is not None
        else None
    )
    if debug_mode:
        assert policy_controller is not None
        policy_controller.record_debug_mode()
    if credential_file_path is not None:
        assert production_plan is not None
        credential_lease = _credential_lease_from_file(
            credential_file_path,
            plan=production_plan,
            agent_limit=agent_limit,
        )
    adapter_arguments = (
        environment_name,
        materialized_agent,
        config["agent_command"]["digest"],
        materialized_evaluator,
        config["evaluator_command"]["digest"],
    )
    evaluator_options = (
        {
            "evaluator_node_path": evaluator_node_path,
            "evaluator_node_sha256": evaluator_node_sha256,
            "evaluator_node_version": evaluator_node_version,
        }
        if evaluator_node_path is not None
        else {}
    )
    if environment_name in lume_environments:
        try:
            environment, agent, observer, evaluator = adapters(
                *adapter_arguments,
                lume_config_path=(trial_dir / "inputs/apparatus/lume/config.json").resolve(),
                guest_launch_path=(trial_dir / "inputs/apparatus/launch/guest-launch.json"),
                production_plan=production_plan,
                credential_lease=credential_lease,
                **({"debug_mode": True} if debug_mode else {}),
                **evaluator_options,
            )
        except BaseException:
            if credential_lease is not None:
                credential_lease.destroy()
            raise
    else:
        environment, agent, observer, evaluator = adapters(*adapter_arguments, **evaluator_options)

    try:
        events = EventLog(event_path, clock)
        machine = StateMachine(events)
        events.append(
            "trial_started",
            {
                "trial_id": resolved_id,
                "config_digest": config_digest,
                "inputs_manifest_digest": inputs_manifest_digest,
            },
            sync=True,
        )
    except BaseException:
        if credential_lease is not None:
            credential_lease.destroy()
        raise

    def emit(event_type: str, data: dict[str, Any]) -> None:
        events.append(event_type, data, sync=event_type == "process_spawned")

    def append_participation_event(event_type: str, data: dict[str, Any]) -> str:
        return events.append(event_type, data, sync=True)

    context = TrialContext(
        trial_id=resolved_id,
        task=task,
        task_path=task_path,
        config=config,
        trial_dir=trial_dir,
        artifacts=trial_dir / "artifacts",
        harness_workspace=trial_dir / "harness-workspace",
        emit=emit,
        policy=policy_controller,
    )
    collected: tuple[int, Path, dict[str, Any]] | None = None
    observer_started = False
    observer_finished = False
    observer_report = ObserverReport(
        name=getattr(observer, "name", "unknown"),
        trust="unavailable",
        detail="observer did not start",
    )
    participation: dict[str, Any] | None = None
    participation_emitted = False
    participation_signature: dict[str, Any] | None = None
    policy_receipt: dict[str, Any] | None = None
    certification_receipt: dict[str, Any] | None = None
    certification_signature: dict[str, Any] | None = None
    setup_started = False
    hard_aborted = False

    def finish_observer() -> None:
        nonlocal observer_finished, observer_report
        if observer_finished or not observer_started or handle is None:
            return
        try:
            observer_report = observer.finish(context, handle, outcome)
        except Exception as error:  # noqa: BLE001 - preserve independent grading
            observer_report = ObserverReport(
                name=getattr(observer, "name", "unknown"),
                trust="unavailable",
                detail=f"observer finish failed: {type(error).__name__}",
            )
            emit(
                "participation_observer_error",
                {"phase": "finish", "error_type": type(error).__name__},
            )
        observer_finished = True

    def ensure_participation() -> dict[str, Any]:
        """Evaluate observer evidence once, degrading invalid evidence safely."""

        nonlocal participation, participation_emitted
        if participation is None:
            try:
                participation = evaluate_participation(
                    participation_requirements,
                    observer_report,
                    append_participation_event,
                    bindings={
                        "trial_id": resolved_id,
                        "task_digest": config["task"]["digest"],
                        "config_digest": config_digest,
                    },
                    platform=handle.facts.get("platform") if handle else None,
                )
            except Exception as error:  # noqa: BLE001 - cleanup must still run
                emit(
                    "participation_observer_error",
                    {"phase": "verify", "error_type": type(error).__name__},
                )
                fallback = ObserverReport(
                    name=getattr(observer, "name", "unknown"),
                    trust="unavailable",
                    detail=f"observer evidence was invalid: {type(error).__name__}",
                )
                participation = evaluate_participation(
                    participation_requirements,
                    fallback,
                    append_participation_event,
                    bindings={
                        "trial_id": resolved_id,
                        "task_digest": config["task"]["digest"],
                        "config_digest": config_digest,
                    },
                    platform=handle.facts.get("platform") if handle else None,
                )
        if not participation_emitted:
            emit("driver_participation_receipt", participation)
            participation_emitted = True
        return participation

    def ensure_participation_signature() -> dict[str, Any] | None:
        nonlocal participation_signature
        if participation_signing_key is None or participation_verifier_key is None:
            return None
        if participation_signature is not None:
            return participation_signature
        receipt = ensure_participation()
        bindings = {
            "trial_id": resolved_id,
            "task_digest": config["task"]["digest"],
            "config_digest": config_digest,
        }
        artifact = sign_receipt(
            receipt,
            bindings=bindings,
            private_key=participation_signing_key,
        )
        verified = verify_receipt(
            artifact,
            trusted_public_key=participation_verifier_key,
            expected_bindings=bindings,
        )
        if verified != receipt:
            raise HarnessFailure("signed participation receipt changed during verification")
        artifact_path = context.artifacts / "participation-receipt.sshsig.json"
        if artifact_path.parent.resolve() != context.artifacts.resolve():
            raise HarnessFailure("participation signature path escapes artifacts")
        _write_fresh_signed_artifact(artifact_path, artifact)
        participation_signature = {
            "path": "artifacts/participation-receipt.sshsig.json",
            "digest": digest_file(artifact_path),
            "key_id": verifier_key_id,
        }
        emit("driver_participation_signature", participation_signature)
        return participation_signature

    with (
        _destroy_credential_lease_on_exit(credential_lease) as destroy_credential_lease,
        InterruptFlag() as interrupt,
    ):
        try:
            machine.enter(State.SETUP)
            setup_started = True
            handle = environment.setup(context)
            handle = EnvironmentHandle(
                kind=handle.kind,
                root=handle.root,
                facts={
                    **handle.facts,
                    "fresh_harness_workspace": (
                        context.harness_workspace.is_dir()
                        and not any(context.harness_workspace.iterdir())
                    ),
                },
            )
            emit("env_ready", {"kind": handle.kind, "facts": dict(handle.facts)})
            interrupt.raise_if_requested()

            if participation_requirements:
                try:
                    observer.start(context, handle, participation_requirements)
                    observer_started = True
                    emit(
                        "participation_observer_started",
                        {
                            "observer": getattr(observer, "name", "unknown"),
                            "requirements": len(participation_requirements),
                        },
                    )
                except Exception as error:  # noqa: BLE001 - grading stays independent
                    observer_report = ObserverReport(
                        name=getattr(observer, "name", "unknown"),
                        trust="unavailable",
                        detail=f"observer start failed: {type(error).__name__}",
                    )
                    observer_finished = True
                    emit(
                        "participation_observer_error",
                        {"phase": "start", "error_type": type(error).__name__},
                    )

            machine.enter(State.AGENT)
            outcome = agent.run(context, handle, interrupt, agent_limit)
            emit("agent_outcome", asdict(outcome))
            interrupt.raise_if_requested()

            machine.enter(State.EXPORT)
            export = getattr(environment, "export", None)
            if callable(export):
                handle = export(context, handle, outcome)
            emit("workspace_exported", {"adapter": environment.name})
            finish_observer()
            ensure_participation()
            interrupt.raise_if_requested()

            machine.enter(State.EVALUATION)
            evaluation = evaluator.evaluate(context, handle, outcome, interrupt, evaluator_limit)
            emit("evaluation", asdict(evaluation))
            interrupt.raise_if_requested()
            if outcome.terminal_failure is not None:
                original_exit = exit_codes.HARNESS
                status = "infrastructure_error"
                error_detail = outcome.terminal_failure
                emit(
                    "error",
                    {
                        "status": status,
                        "terminal_failure": outcome.terminal_failure,
                    },
                )
        except HardAbort:
            hard_aborted = True
            try:
                destroy_credential_lease()
            except BaseException:
                pass
            if setup_started or handle is not None:
                try:
                    _bounded_emergency_cleanup(environment, context, handle)
                except BaseException:
                    pass
            try:
                events.close()
            except BaseException:
                pass
            raise
        except CbError as error:
            original_exit = error.exit_code
            status = error.status
            error_detail = str(error)
            exceptional_export = (
                isinstance(error, (DeadlineExceeded, BudgetExceeded, TrialInterrupted))
                and machine.state == State.AGENT
                and handle is not None
            )
            if exceptional_export:
                export = getattr(environment, "export", None)
                if callable(export):
                    try:
                        machine.enter(State.EXPORT)
                        exceptional_outcome = AgentOutcome(
                            completed=False,
                            exit_code=None,
                            duration_ms=clock.elapsed_ms(),
                            artifacts=(),
                        )
                        handle = export(context, handle, exceptional_outcome)
                        export_event = {
                            "adapter": environment.name,
                            "after_exception": True,
                            "error_status": status,
                        }
                        if isinstance(error, DeadlineExceeded):
                            export_event["after_timeout"] = True
                        emit(
                            "workspace_exported",
                            export_event,
                        )
                    except Exception as export_error:  # noqa: BLE001 - preserve primary
                        failure_event = {
                            "after_exception": True,
                            "error_status": status,
                            "error_type": type(export_error).__name__,
                        }
                        if isinstance(error, DeadlineExceeded):
                            failure_event["after_timeout"] = True
                        emit(
                            "workspace_export_failed",
                            failure_event,
                        )
            if status == "interrupted":
                emit("interrupted", {"message": error_detail})
            elif status == "timeout":
                emit("deadline_exceeded", {"message": error_detail})
            emit("error", {"status": status, "message": error_detail})
        except Exception as error:  # noqa: BLE001 - adapters are an exception boundary
            original_exit = exit_codes.HARNESS
            status = "harness_error"
            error_detail = f"{type(error).__name__}: {error}"
            emit("error", {"status": status, "message": error_detail})
        finally:
            if not hard_aborted and not events.closed:
                finish_observer()
                ensure_participation()
                try:
                    machine.enter(State.CLEANUP)
                    emit("cleanup_started", {"adapter": environment.name})
                    cleanup = environment.cleanup(
                        context,
                        handle,
                        timeout_seconds=float(
                            getattr(environment, "cleanup_timeout_seconds", 10.0)
                        ),
                    )
                    if not cleanup.ok:
                        emit(
                            "cleanup_failed",
                            {"message": cleanup.error or "cleanup failed"},
                        )
                except Exception as error:  # noqa: BLE001 - cleanup must never escape
                    cleanup = CleanupReport(ok=False, error=f"{type(error).__name__}: {error}")
                    emit("cleanup_failed", {"message": cleanup.error})

                if interrupt.count and original_exit == exit_codes.OK:
                    original_exit = exit_codes.INTERRUPTED
                    status = "interrupted"
                    error_detail = "trial interrupted"
                    emit("interrupted", {"message": error_detail})

                final_exit = exit_codes.dominant(original_exit, not cleanup.ok)
                final_status = "cleanup_error" if not cleanup.ok else status
                machine.enter(State.COLLECT)
                signature_error = False
                try:
                    ensure_participation_signature()
                except Exception as error:  # noqa: BLE001 - fail closed after cleanup
                    signature_error = True
                    emit(
                        "participation_signature_error",
                        {"error_type": type(error).__name__},
                    )
                if policy_controller is not None:
                    input_mutations = _mutated_inputs(
                        trial_dir, config_digest, inputs_manifest_digest
                    )
                    try:
                        for relative in input_mutations:
                            policy_controller.record_self_modification(
                                relative,
                                "pinned runtime input changed during execution",
                            )
                        policy_receipt = policy_controller.finalize(
                            config_digest=config_digest,
                            elapsed_ms=(
                                outcome.duration_ms if outcome is not None else clock.elapsed_ms()
                            ),
                            environment_facts=handle.facts if handle else None,
                        )
                    except Exception as error:  # noqa: BLE001 - retain graded result
                        emit(
                            "execution_policy_observer_error",
                            {"error_type": type(error).__name__},
                        )
                        unavailable_body = {
                            "status": "unavailable",
                            "eligible": False,
                            "trust": "unavailable",
                            "violations": ["policy_observer_error"],
                            "bindings": {
                                "trial_id": resolved_id,
                                "system_digest": system_digest,
                                "execution_policy_digest": execution_policy_digest,
                                "config_digest": config_digest,
                            },
                            "observed": None,
                        }
                        policy_receipt = {
                            **unavailable_body,
                            "receipt_digest": digest_json(unavailable_body),
                        }
                    emit("execution_policy_receipt", policy_receipt)
                else:
                    input_mutations = _mutated_inputs(
                        trial_dir, config_digest, inputs_manifest_digest
                    )

                certification_signature_error = False
                if environment_name == "lume-macos-certifying":
                    try:
                        certification_bindings = {
                            "trial_id": resolved_id,
                            "task_digest": config["task"]["digest"],
                            "system_digest": system_digest,
                            "execution_policy_digest": execution_policy_digest,
                            "config_digest": config_digest,
                            "inputs_manifest_digest": inputs_manifest_digest,
                            "seed_provenance_digest": (
                                handle.facts.get("seed_provenance_digest") if handle else None
                            ),
                            "agent_digest": config["agent_command"]["digest"],
                            "evaluator_digest": config["evaluator_command"]["digest"],
                        }
                        certification_receipt = build_certification_receipt(
                            bindings=certification_bindings,
                            apparatus=_certification_apparatus(
                                environment_name=environment_name,
                                environment_declared_certifying=bool(config["certifying"]),
                                apparatus_check=bool(config.get("apparatus_check", False)),
                                handle=handle,
                                cleanup=cleanup,
                                inputs_unchanged=not input_mutations,
                                evaluation=evaluation,
                                participation=participation,
                                participation_signature=participation_signature,
                                policy_receipt=policy_receipt,
                            ),
                            outcome=(asdict(evaluation) if evaluation else None),
                            participation=participation,
                            comparison=policy_receipt,
                        )
                        verify_certification_receipt(certification_receipt)
                        emit(
                            "apparatus_certification_receipt",
                            certification_receipt,
                        )
                        if participation_signing_key is None or participation_verifier_key is None:
                            raise HarnessFailure("certification receipt signer is unavailable")
                        artifact = sign_certification_receipt(
                            certification_receipt,
                            bindings=certification_bindings,
                            private_key=participation_signing_key,
                        )
                        verified = verify_certification_signature(
                            artifact,
                            trusted_public_key=participation_verifier_key,
                            expected_bindings=certification_bindings,
                        )
                        if verified != certification_receipt:
                            raise HarnessFailure(
                                "signed certification receipt changed during verification"
                            )
                        artifact_path = (
                            context.artifacts / "apparatus-certification-receipt.sshsig.json"
                        )
                        if artifact_path.parent.resolve() != context.artifacts.resolve():
                            raise HarnessFailure("certification signature path escapes artifacts")
                        _write_fresh_signed_artifact(artifact_path, artifact)
                        certification_signature = {
                            "path": ("artifacts/apparatus-certification-receipt.sshsig.json"),
                            "digest": digest_file(artifact_path),
                            "key_id": verifier_key_id,
                        }
                        emit(
                            "apparatus_certification_signature",
                            certification_signature,
                        )
                    except Exception as error:  # noqa: BLE001 - preserve outcome
                        certification_signature_error = True
                        certification_receipt = None
                        certification_signature = None
                        emit(
                            "apparatus_certification_error",
                            {"error_type": type(error).__name__},
                        )
                events.append(
                    "trial_finished",
                    {
                        "status": final_status,
                        "original_status": status,
                        "exit_code": final_exit,
                        "cleanup_ok": cleanup.ok,
                    },
                    sync=True,
                )
                machine.enter(State.DONE)
                events.close()
                certifying_environment_evidence = _has_certifying_environment_evidence(
                    environment_name, handle, cleanup
                )
                legacy_certifying = (
                    bool(config["certifying"])
                    and certifying_environment_evidence
                    and (policy_receipt is None or policy_receipt.get("eligible") is True)
                    and (
                        not participation["required"]
                        or (
                            participation["passed"] is True
                            and participation["observer"]["trust"] == "certifying"
                        )
                    )
                )
                final_certifying = (
                    bool(
                        certification_receipt
                        and certification_receipt.get("eligible") is True
                        and certification_signature is not None
                        and not certification_signature_error
                        and final_status == "completed"
                    )
                    if environment_name == "lume-macos-certifying"
                    else legacy_certifying
                )
                if debug_mode:
                    final_certifying = False
                result = {
                    "schema_version": output_schema_version,
                    "trial_id": resolved_id,
                    "task_id": task["id"],
                    "environment_certifying": bool(config["certifying"]),
                    "apparatus_check": bool(config.get("apparatus_check", False)),
                    "debug_mode": bool(config.get("debug_mode", False)),
                    "certifying": final_certifying,
                    "participation_signature": participation_signature,
                    "apparatus_certification": certification_receipt,
                    "apparatus_certification_signature": certification_signature,
                    "status": final_status,
                    "original_status": status,
                    "exit_code": final_exit,
                    "cleanup_ok": cleanup.ok,
                    "cleanup_error": cleanup.error,
                    "evaluation": asdict(evaluation) if evaluation else None,
                    "participation": participation,
                    "execution_policy": policy_receipt,
                    "comparison_eligible": (
                        policy_receipt["eligible"] and not debug_mode
                        if policy_receipt is not None
                        else None
                    ),
                    "error": error_detail,
                    "config_digest": config_digest,
                    "inputs_manifest_digest": inputs_manifest_digest,
                    "event_log_digest": digest_file(event_path),
                    "elapsed_ms": clock.elapsed_ms(),
                }
                if config.get("participation_verifier", {}).get("required") and (
                    signature_error or participation_signature is None
                ):
                    result["certifying"] = False
                write_result(trial_dir / "result.json", result)
                collected = (final_exit, trial_dir, result)

    if collected is None:
        raise HarnessFailure("trial engine exited without collecting a result")
    return collected
