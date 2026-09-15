"""Protected macOS guest execution and stopped-disk export."""

from __future__ import annotations

import gzip
import hashlib
import io
import json
import os
import re
import stat
import tarfile
import time
from collections.abc import Mapping
from dataclasses import asdict
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from cua_bench_runtime.adapters.agent_harnesses.production import (
    HarnessRenderContext,
    NativeMcpDriver,
)
from cua_bench_runtime.adapters.agent_harnesses.system import ProductionSystemPlan
from cua_bench_runtime.canon import canonical_json, digest_file, digest_json
from cua_bench_runtime.credential_lease import CredentialLease
from cua_bench_runtime.errors import DeadlineExceeded, HarnessFailure, ValidationFailure
from cua_bench_runtime.guest_launch import (
    CollectionBounds,
    GuestLaunchSpec,
    RenderedGuestLaunch,
    load_guest_launch,
)
from cua_bench_runtime.lume import CommandResult, LumeAttempt, LumeControlPlane
from cua_bench_runtime.model import AgentOutcome, CleanupReport, EnvironmentHandle, TrialContext
from cua_bench_runtime.provider_proxy import ProviderConnectProxy
from cua_bench_runtime.signals import InterruptFlag


MAX_PRODUCTION_TELEMETRY_EVENTS = 1024
MAX_PRODUCTION_TELEMETRY_LINE_BYTES = 64 * 1024
DEBUG_ARTIFACT = "agent.debug.json"


@dataclass(frozen=True)
class _ParsedProductionOutput:
    telemetry: tuple[dict[str, Any], ...]
    debug_events: tuple[dict[str, Any], ...]
    skipped_lines: int
    usage_event: dict[str, Any] | None
    terminal_failure: str | None


def _safe_tool_name(event: Mapping[str, Any], allowed_names: frozenset[str]) -> str | None:
    candidates: list[Any] = [event.get("tool"), event.get("tool_name")]
    for container_name in ("item", "part"):
        container = event.get(container_name)
        if isinstance(container, Mapping) and container.get("type") in {
            "mcp_tool_call",
            "tool",
            "tool_call",
            "tool_use",
        }:
            candidates.extend((container.get("tool"), container.get("name")))
    return next(
        (
            candidate
            for candidate in candidates
            if isinstance(candidate, str) and candidate in allowed_names
        ),
        None,
    )


def _parse_production_output(
    result: CommandResult, production: ProductionSystemPlan
) -> _ParsedProductionOutput:
    telemetry: list[dict[str, Any]] = []
    debug_events: list[dict[str, Any]] = []
    skipped_lines = 0
    allowed_tools = frozenset(production.tool_names)
    for line in result.stdout.splitlines():
        if len(telemetry) >= MAX_PRODUCTION_TELEMETRY_EVENTS:
            skipped_lines += 1
            continue
        if len(line.encode("utf-8")) > MAX_PRODUCTION_TELEMETRY_LINE_BYTES:
            skipped_lines += 1
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            skipped_lines += 1
            continue
        if not isinstance(event, dict):
            skipped_lines += 1
            continue
        normalized = asdict(production.harness.normalize_telemetry(event, production.model_route))
        telemetry.append(normalized)
        tool_name = _safe_tool_name(event, allowed_tools)
        debug_event = {
            "sequence": len(debug_events) + 1,
            "event_type": ("tool_activity" if tool_name is not None else normalized["event_type"]),
        }
        if tool_name is not None:
            debug_event["tool_name"] = tool_name
        debug_events.append(debug_event)
    terminal_failure = next(
        (
            str(item["terminal_failure"])
            for item in reversed(telemetry)
            if item["terminal_failure"] is not None
        ),
        None,
    )
    cumulative_usage = [
        (index, item)
        for index, item in enumerate(telemetry)
        if item["usage_is_cumulative_total"]
        and all(
            item[field] is not None
            for field in (
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
            )
        )
    ]
    # Some CLIs append an empty terminal record. Keep the greatest cumulative
    # total and use sequence only as a tie-breaker.
    usage_event = (
        max(
            cumulative_usage,
            key=lambda pair: (
                sum(
                    int(pair[1][field])
                    for field in (
                        "input_tokens",
                        "output_tokens",
                        "cache_read_tokens",
                        "cache_write_tokens",
                    )
                ),
                pair[0],
            ),
        )[1]
        if cumulative_usage
        else None
    )
    return _ParsedProductionOutput(
        telemetry=tuple(telemetry),
        debug_events=tuple(debug_events),
        skipped_lines=skipped_lines,
        usage_event=usage_event,
        terminal_failure=terminal_failure,
    )


def _output_evidence(result: CommandResult) -> dict[str, dict[str, Any]]:
    evidence = {
        "stdout": {
            "bytes": result.stdout_bytes,
            "sha256": f"sha256:{result.stdout_sha256}",
            "truncated": result.stdout_truncated,
        },
        "stderr": {
            "bytes": result.stderr_bytes,
            "sha256": f"sha256:{result.stderr_sha256}",
            "truncated": result.stderr_truncated,
        },
    }
    if any(
        not isinstance(stream["bytes"], int)
        or not re.fullmatch(r"sha256:[a-f0-9]{64}", str(stream["sha256"]))
        or not isinstance(stream["truncated"], bool)
        for stream in evidence.values()
    ):
        raise HarnessFailure("guest agent output evidence is unavailable")
    return evidence


def _desktop_fixture(task: dict[str, Any]) -> tuple[str, str, tuple[str, ...]] | None:
    """Return the task-owned protected desktop launch contract, when declared."""

    variants = task.get("variants", [])
    try:
        fixture = variants[0]["parameters"].get("protected_desktop")
    except (IndexError, KeyError, TypeError) as error:
        raise HarnessFailure("task variant parameters are invalid") from error
    if fixture is None:
        return None
    if not isinstance(fixture, dict) or set(fixture) != {
        "app_id",
        "store_relative",
        "launch_arguments",
    }:
        raise HarnessFailure("protected desktop contract is invalid")
    app_id = fixture["app_id"]
    store_relative = fixture["store_relative"]
    launch_arguments = fixture["launch_arguments"]
    if (
        not isinstance(app_id, str)
        or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", app_id)
        or not isinstance(store_relative, str)
        or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", store_relative)
        or not isinstance(launch_arguments, list)
        or len(launch_arguments) > 16
        or any(
            not isinstance(argument, str)
            or not re.fullmatch(
                r"--[a-z][a-z0-9-]{0,31}=[A-Za-z0-9._:@/+,-]{1,256}",
                argument,
            )
            or argument.startswith("--store=")
            for argument in launch_arguments
        )
    ):
        raise HarnessFailure("protected desktop contract is invalid")
    return store_relative, app_id, tuple(launch_arguments)


def _driver_identity_digest(facts: dict[str, Any]) -> str:
    return digest_json(
        {
            key: facts.get(key)
            for key in (
                "driver_version",
                "driver_realpath",
                "driver_sha256",
                "driver_team_id",
                "agent_isolation",
                "permissions",
            )
        }
    )


def _endpoint_binding_digest(evidence: dict[str, Any]) -> str | None:
    fields = (
        "mediator_sha256",
        "backend_parent_device",
        "backend_parent_inode",
        "backend_device",
        "backend_inode",
        "target_pid",
    )
    if any(evidence.get(field) is None for field in fields):
        return None
    if not isinstance(evidence.get("tool_contract_required"), bool):
        return None
    return digest_json(
        {
            **{field: evidence[field] for field in fields},
            "tool_contract_required": evidence["tool_contract_required"],
            "expected_tool_contract_sha256": evidence.get("expected_tool_contract_sha256"),
            "daemon_tool_list_envelope_required": evidence.get(
                "daemon_tool_list_envelope_required"
            ),
            "expected_daemon_tool_list_envelope_sha256": evidence.get(
                "expected_daemon_tool_list_envelope_sha256"
            ),
        }
    )


def _provider_proxy_summary(
    initial: dict[str, Any],
    sealed: dict[str, Any],
    artifact_sha256: str,
) -> dict[str, Any]:
    """Compose the exact content-free proxy evidence used for certification."""

    return {
        "enforcer": "host-cdb-connect-proxy",
        "schema_version": sealed.get("schema_version"),
        "trial_id": sealed.get("trial_id"),
        "endpoint": sealed.get("endpoint"),
        "allowed_client_ip": sealed.get("allowed_client_ip"),
        "provider_allowlist_sha256": sealed.get("allowed_authorities_digest"),
        "initial_client_binding_digest": initial.get("client_binding_digest"),
        "sealed_client_binding_digest": sealed.get("client_binding_digest"),
        "initial_implementation_digest": initial.get("implementation_digest"),
        "sealed_implementation_digest": sealed.get("implementation_digest"),
        "implementation_identity": sealed.get("implementation_identity"),
        "initial_evidence_digest": digest_json(initial),
        "sealed_evidence_digest": digest_json(sealed),
        "artifact_sha256": artifact_sha256,
        "accepted_connections": sealed.get("accepted_connections"),
        "rejected_connections": sealed.get("rejected_connections"),
        "bytes_guest_to_provider": sealed.get("bytes_guest_to_provider"),
        "bytes_provider_to_guest": sealed.get("bytes_provider_to_guest"),
        "transcript_chain_digest": sealed.get("transcript_chain_digest"),
        "active": sealed.get("active"),
        "sealed": sealed.get("sealed"),
    }


@dataclass(frozen=True)
class LumeSettings:
    binary: Path
    binary_sha256: str
    seed_vm: str
    seed_provenance_digest: str
    seed_manifest_path: Path
    driver_version: str
    driver_sha256: str
    driver_team_id: str
    ssh_user: str
    ssh_identity_file: Path
    ssh_known_hosts_file: Path
    ssh_known_hosts_sha256: str
    ssh_host_key_alias: str
    privileged_helper_path: PurePosixPath
    privileged_helper_sha256: str
    mediator_sha256: str
    privileged_sudoers_sha256: str
    pf_main_rules_sha256: str
    network_allowlist: tuple[str, ...]
    production_tools: dict[str, str]
    console_uid: int
    expected_pristine_fingerprint: str
    storage_root: Path
    source_name: str
    source_digest: str


def load_lume_settings(path: Path) -> LumeSettings:
    path = path.resolve()
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationFailure("Lume config is not valid UTF-8 JSON") from error
    required = {
        "schema_version",
        "binary",
        "binary_sha256",
        "seed_vm",
        "seed_provenance_digest",
        "seed_manifest_path",
        "driver_version",
        "driver_sha256",
        "driver_team_id",
        "ssh_user",
        "ssh_identity_file",
        "ssh_known_hosts_file",
        "ssh_known_hosts_sha256",
        "ssh_host_key_alias",
        "privileged_helper_path",
        "privileged_helper_sha256",
        "mediator_sha256",
        "privileged_sudoers_sha256",
        "pf_main_rules_sha256",
        "network_allowlist",
        "console_uid",
        "storage_root",
        "expected_pristine_fingerprint",
    }
    allowed = required | {"production_tools"}
    if not isinstance(document, dict) or set(document).difference(allowed):
        raise ValidationFailure("Lume config contains unknown fields")
    missing = sorted(required.difference(document))
    if missing:
        raise ValidationFailure(f"Lume config is missing: {missing[0]}")
    if document["schema_version"] != 3:
        raise ValidationFailure("Lume config schema_version must be 3")

    def absolute_file(key: str) -> Path:
        value = document[key]
        candidate = Path(value).resolve() if isinstance(value, str) else Path()
        if not isinstance(value, str) or not Path(value).is_absolute() or not candidate.is_file():
            raise ValidationFailure(f"Lume config {key} must be an absolute file")
        return candidate

    binary = absolute_file("binary")
    if digest_file(binary) != document["binary_sha256"]:
        raise ValidationFailure("Lume config binary digest mismatch")
    seed_manifest = absolute_file("seed_manifest_path")
    identity_file = absolute_file("ssh_identity_file")
    known_hosts_file = absolute_file("ssh_known_hosts_file")
    if stat.S_IMODE(identity_file.stat().st_mode) != 0o600:
        raise ValidationFailure("Lume SSH identity file must have mode 0600")
    if identity_file.stat().st_uid != os.geteuid():
        raise ValidationFailure("Lume SSH identity file must be owned by the runtime user")
    if stat.S_IMODE(known_hosts_file.stat().st_mode) & 0o022:
        raise ValidationFailure("Lume known_hosts file must not be group/world writable")
    if digest_file(known_hosts_file) != document["ssh_known_hosts_sha256"]:
        raise ValidationFailure("Lume known_hosts digest mismatch")
    helper_path = document["privileged_helper_path"]
    if not isinstance(helper_path, str) or not PurePosixPath(helper_path).is_absolute():
        raise ValidationFailure("Lume privileged helper path must be absolute")
    allowlist = document["network_allowlist"]
    if not isinstance(allowlist, list) or any(
        not isinstance(endpoint, str) or not endpoint for endpoint in allowlist
    ):
        raise ValidationFailure("Lume network_allowlist must be an array of endpoints")
    production_tools = document.get("production_tools", {})
    if (
        not isinstance(production_tools, dict)
        or not set(production_tools)
        <= {"codex", "claude", "opencode", "cua-driver", "codex-code-mode-host"}
        or any(
            not isinstance(name, str)
            or not isinstance(digest, str)
            or not re.fullmatch(r"sha256:[a-f0-9]{64}", digest)
            for name, digest in production_tools.items()
        )
    ):
        raise ValidationFailure("Lume config production_tools is invalid")
    storage_value = document["storage_root"]
    if not isinstance(storage_value, str) or not Path(storage_value).is_absolute():
        raise ValidationFailure("Lume config storage_root must be absolute")
    console_uid = document["console_uid"]
    if not isinstance(console_uid, int) or isinstance(console_uid, bool) or console_uid < 1:
        raise ValidationFailure("Lume config console_uid is invalid")
    fingerprint = document["expected_pristine_fingerprint"]
    if not isinstance(fingerprint, str) or not re.fullmatch(r"sha256:[a-f0-9]{64}", fingerprint):
        raise ValidationFailure("Lume config pristine fingerprint is invalid")
    return LumeSettings(
        binary=binary,
        binary_sha256=str(document["binary_sha256"]),
        seed_vm=str(document["seed_vm"]),
        seed_provenance_digest=str(document["seed_provenance_digest"]),
        seed_manifest_path=seed_manifest,
        driver_version=str(document["driver_version"]),
        driver_sha256=str(document["driver_sha256"]),
        driver_team_id=str(document["driver_team_id"]),
        ssh_user=str(document["ssh_user"]),
        ssh_identity_file=identity_file,
        ssh_known_hosts_file=known_hosts_file,
        ssh_known_hosts_sha256=str(document["ssh_known_hosts_sha256"]),
        ssh_host_key_alias=str(document["ssh_host_key_alias"]),
        privileged_helper_path=PurePosixPath(helper_path),
        privileged_helper_sha256=str(document["privileged_helper_sha256"]),
        mediator_sha256=str(document["mediator_sha256"]),
        privileged_sudoers_sha256=str(document["privileged_sudoers_sha256"]),
        pf_main_rules_sha256=str(document["pf_main_rules_sha256"]),
        network_allowlist=tuple(allowlist),
        production_tools=dict(sorted(production_tools.items())),
        console_uid=console_uid,
        expected_pristine_fingerprint=fingerprint,
        storage_root=Path(storage_value).resolve(),
        source_name=path.name,
        source_digest=digest_file(path),
    )


def build_control(settings: LumeSettings) -> LumeControlPlane:
    return LumeControlPlane(
        binary=settings.binary,
        binary_sha256=settings.binary_sha256,
        seed_vm=settings.seed_vm,
        seed_provenance_digest=settings.seed_provenance_digest,
        seed_manifest_path=settings.seed_manifest_path,
        driver_version=settings.driver_version,
        driver_sha256=settings.driver_sha256,
        driver_team_id=settings.driver_team_id,
        ssh_user=settings.ssh_user,
        ssh_identity_file=settings.ssh_identity_file,
        ssh_known_hosts_file=settings.ssh_known_hosts_file,
        ssh_known_hosts_sha256=settings.ssh_known_hosts_sha256,
        ssh_host_key_alias=settings.ssh_host_key_alias,
        privileged_helper_path=settings.privileged_helper_path,
        privileged_helper_sha256=settings.privileged_helper_sha256,
        mediator_sha256=settings.mediator_sha256,
        privileged_sudoers_sha256=settings.privileged_sudoers_sha256,
        pf_main_rules_sha256=settings.pf_main_rules_sha256,
        network_allowlist=settings.network_allowlist,
        production_tools=settings.production_tools,
        console_uid=settings.console_uid,
        expected_pristine_fingerprint=settings.expected_pristine_fingerprint,
        storage_root=settings.storage_root,
    )


def _archive_payload(
    context: TrialContext,
    agent_command: Path,
    launch: GuestLaunchSpec,
    production_plan: ProductionSystemPlan | None = None,
    selected_brief: PurePosixPath | None = None,
) -> tuple[bytes, str]:
    inputs = context.trial_dir / "inputs"
    declarations = context.task["variants"][0]["fixture_artifacts"]
    files: list[tuple[Path, str, int]] = []
    for artifact in declarations:
        relative = str(artifact["path"])
        source = (inputs / "artifacts" / relative).resolve()
        if not source.is_relative_to((inputs / "artifacts").resolve()):
            raise HarnessFailure("agent-visible task artifact escapes inputs")
        if (
            not source.is_file()
            or digest_file(source).removeprefix("sha256:") != artifact["sha256"]
        ):
            raise HarnessFailure(f"agent-visible task artifact changed: {relative}")
        files.append((source, f"task/{relative}", 0o444))
    if production_plan is None:
        files.append((agent_command, f"harness/agent/{agent_command.name}", 0o555))
    expected_brief = PurePosixPath("task/brief.md")
    if "agent_brief" in context.config:
        declaration = context.config["agent_brief"]
        source = inputs / "apparatus" / "agent-brief.md"
        if production_plan is None or declaration != {
            "path": "inputs/apparatus/agent-brief.md",
            "digest": digest_file(source),
        }:
            raise HarnessFailure("agent brief apparatus input changed")
        expected_brief = PurePosixPath("apparatus/agent-brief.md")
        files.append((source, str(expected_brief), 0o444))
    if selected_brief is None:
        selected_brief = expected_brief
    elif selected_brief != expected_brief:
        raise HarnessFailure("selected agent brief does not match trial configuration")
    files.append((launch.source, "harness/guest-launch.json", 0o444))
    for artifact in launch.build_artifacts:
        source = (launch.source.parent / artifact.path).resolve()
        files.append((source, f"harness/build/{artifact.path}", 0o444))
    guest_root = LumeControlPlane.attempt_root(context.trial_id)
    values = {
        "agent": str(guest_root / "harness/agent" / agent_command.name),
        "workspace": str(guest_root / "workspace"),
        "artifacts": str(guest_root / "artifacts"),
        "home": str(guest_root / "home"),
        "brief": str(guest_root / selected_brief),
        "driver_socket": str(LumeControlPlane.protected_driver_socket(context.trial_id)),
    }
    rendered, production_configs = _render_guest_launch(values, launch, production_plan)
    generated = {"harness/launch.json": canonical_json(rendered.document()) + b"\n"}
    for config_path, content in production_configs:
        relative = PurePosixPath(config_path).relative_to(guest_root)
        name = relative.as_posix()
        if name in generated:
            raise HarnessFailure(f"guest payload archive path is duplicated: {name}")
        generated[name] = content
    archive_buffer = io.BytesIO()
    with gzip.GzipFile(fileobj=archive_buffer, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for directory in (
                "task",
                "harness",
                "harness/agent",
                "harness/build",
                "home",
                "home/work",
                "workspace",
                "artifacts",
                "apparatus",
            ):
                info = tarfile.TarInfo(directory)
                info.type = tarfile.DIRTYPE
                info.mode = 0o700
                info.mtime = 0
                archive.addfile(info)
            seen: set[str] = set()
            for source, name, mode in sorted(files, key=lambda item: item[1]):
                if name in seen or name.startswith("/") or ".." in PurePosixPath(name).parts:
                    raise HarnessFailure(f"guest payload archive path is unsafe: {name}")
                seen.add(name)
                content = source.read_bytes()
                info = tarfile.TarInfo(name)
                info.size = len(content)
                info.mode = mode
                info.mtime = 0
                archive.addfile(info, io.BytesIO(content))
            for name, content in sorted(generated.items()):
                if name in seen:
                    raise HarnessFailure(f"guest payload archive path is duplicated: {name}")
                info = tarfile.TarInfo(name)
                info.size = len(content)
                info.mode = 0o440
                info.mtime = 0
                archive.addfile(info, io.BytesIO(content))
    payload = archive_buffer.getvalue()
    return payload, hashlib.sha256(payload).hexdigest()


def _render_guest_launch(
    values: dict[str, str],
    launch: GuestLaunchSpec,
    production_plan: ProductionSystemPlan | None,
) -> tuple[RenderedGuestLaunch, tuple[tuple[str, bytes], ...]]:
    if production_plan is None:
        return launch.render(values), ()
    contract = production_plan.harness.render(
        HarnessRenderContext(
            home=PurePosixPath(values["home"]),
            workspace=PurePosixPath(values["workspace"]),
            artifacts=PurePosixPath(values["artifacts"]),
            brief=PurePosixPath(values["brief"]),
            model_route=production_plan.model_route,
            driver=NativeMcpDriver(
                socket_path=PurePosixPath(values["driver_socket"]),
            ),
        ),
        proxy=production_plan.proxy,
        skill_files=production_plan.skill_files,
    )
    rendered = RenderedGuestLaunch(
        argv=contract.argv,
        cwd=str(contract.cwd),
        environment=contract.environment,
        stdin_path=str(contract.stdin_path),
        harness_kind=production_plan.kind.value,
        executable_sha256=production_plan.guest_executable_sha256,
        credential_names=production_plan.credential_environment,
        support_executables=production_plan.support_executables,
    )
    configs = tuple((str(config.path), config.content) for config in contract.config_files)
    return rendered, configs


def _validate_support_executable_installation(
    production_plan: ProductionSystemPlan, guest_facts: Mapping[str, Any]
) -> None:
    if not production_plan.support_executables:
        return
    expected_installation = {
        PurePosixPath(path).name: "sha256:" + digest
        for path, digest in production_plan.support_executables
    }
    observed_installation = guest_facts.get("production_tools")
    if not isinstance(observed_installation, dict) or any(
        not isinstance(observed_installation.get(name), dict)
        or observed_installation[name].get("path") != f"/usr/local/bin/{name}"
        or observed_installation[name].get("sha256") != digest
        or observed_installation[name].get("owner") != "root:wheel"
        or observed_installation[name].get("mode") != "0755"
        for name, digest in expected_installation.items()
    ):
        raise HarnessFailure("production executable installation does not match the frozen build")


class LumeMacosEnvironment:
    cleanup_timeout_seconds = 180.0
    # Revoking the host proxy is immediate; allow the guest helper enough time
    # to confirm the credential-bearing process has exited on a hard abort.
    hard_abort_cleanup_timeout_seconds = 20.0

    def __init__(
        self,
        control: LumeControlPlane,
        launch: GuestLaunchSpec,
        agent_command: Path,
        *,
        certifying: bool = False,
        production_plan: ProductionSystemPlan | None = None,
        debug_mode: bool = False,
    ) -> None:
        if debug_mode and production_plan is None:
            raise ValueError("debug mode requires a production system plan")
        self.name = "lume-macos-certifying" if certifying else "lume-macos"
        self.certifying = certifying
        self.control = control
        self.launch = launch
        self.agent_command = agent_command
        self.production_plan = production_plan
        self.debug_mode = debug_mode
        self.attempt: LumeAttempt | None = None
        self.pending_vm_name: str | None = None
        self.guest_root: PurePosixPath | None = None
        self.selected_brief: PurePosixPath | None = None
        self.exported = False
        self.agent_terminated = False
        self.network_active = False
        self.mediator_started = False
        self.protected_store_prepared = False
        self.task_app_evidence: dict[str, Any] | None = None
        self.mediator_start_evidence: dict[str, Any] | None = None
        self.protected_report: dict[str, Any] | None = None
        self.protected_report_path: Path | None = None
        self.protected_log_path: Path | None = None
        self.participation_transport_error: str | None = None
        self.provider_proxy: ProviderConnectProxy | None = None
        self.provider_proxy_initial: dict[str, Any] | None = None
        self.provider_proxy_sealed: dict[str, Any] | None = None
        self.provider_proxy_error: str | None = None
        self.debug_document: dict[str, Any] | None = None

    def capture_debug_output(
        self,
        context: TrialContext,
        result: CommandResult,
        parsed: _ParsedProductionOutput,
        *,
        classification: str,
    ) -> None:
        if not self.debug_mode:
            return
        token_totals = (
            {
                "input": int(parsed.usage_event["input_tokens"]),
                "output": int(parsed.usage_event["output_tokens"]),
                "cache_read": int(parsed.usage_event["cache_read_tokens"]),
                "cache_write": int(parsed.usage_event["cache_write_tokens"]),
            }
            if parsed.usage_event is not None
            else None
        )
        terminal_failure = parsed.terminal_failure
        if classification != "timeout" and result.returncode != 0 and terminal_failure is None:
            terminal_failure = "harness_failed"
        self.debug_document = {
            "schema_version": 1,
            "mode": "protected-content-free",
            "events": list(parsed.debug_events),
            "parsed_event_count": len(parsed.debug_events),
            "skipped_line_count": parsed.skipped_lines,
            "token_totals": token_totals,
            "termination": {
                "classification": classification,
                "exit_code": result.returncode,
                "terminal_failure": terminal_failure,
            },
            "provider_activity": None,
            "output": _output_evidence(result),
            "raw_output_persisted": False,
        }
        self._persist_debug(context)

    def capture_debug_unavailable(
        self,
        context: TrialContext,
        *,
        classification: str,
        error_type: str,
    ) -> None:
        if not self.debug_mode:
            return
        self.debug_document = {
            "schema_version": 1,
            "mode": "protected-content-free",
            "events": [],
            "parsed_event_count": 0,
            "skipped_line_count": 0,
            "token_totals": None,
            "termination": {
                "classification": classification,
                "exit_code": None,
                "terminal_failure": "debug_capture_unavailable",
                "error_type": error_type,
            },
            "provider_activity": None,
            "output": None,
            "raw_output_persisted": False,
        }
        self._persist_debug(context)

    def _persist_debug(
        self,
        context: TrialContext,
        provider_proxy: Mapping[str, Any] | None = None,
    ) -> None:
        if self.debug_document is None:
            return
        if provider_proxy is not None:
            self.debug_document["provider_activity"] = {
                field: provider_proxy[field]
                for field in (
                    "accepted_connections",
                    "rejected_connections",
                    "bytes_guest_to_provider",
                    "bytes_provider_to_guest",
                )
            }
        (context.artifacts / DEBUG_ARTIFACT).write_bytes(
            canonical_json(self.debug_document) + b"\n"
        )

    def setup(self, context: TrialContext) -> EnvironmentHandle:
        self.selected_brief = PurePosixPath(
            "apparatus/agent-brief.md" if "agent_brief" in context.config else "task/brief.md"
        )
        self.pending_vm_name = self.control.attempt_name(context.trial_id)
        self.attempt = self.control.provision(context.trial_id)
        if self.production_plan is not None:
            _validate_support_executable_installation(
                self.production_plan, self.attempt.guest_facts
            )
        policy_network = (
            context.policy.policy.get("network", {}) if context.policy is not None else {}
        )
        if self.production_plan is not None:
            proxy = self.production_plan.proxy
            if (
                policy_network.get("mode") != "allowlist"
                or policy_network.get("proxy_endpoint") != proxy.endpoint
                or policy_network.get("provider_allowlist_sha256")
                != proxy.provider_allowlist_sha256
                or policy_network.get("proxy_implementation_sha256") != proxy.implementation_sha256
            ):
                raise HarnessFailure(
                    "production provider proxy is not bound to the execution policy"
                )
            configured_allowlist = getattr(self.control, "network_allowlist", None)
            if configured_allowlist != (proxy.endpoint,):
                raise HarnessFailure(
                    "production provider proxy is not the sole guest network endpoint"
                )
            listen_host, port_text = proxy.endpoint.rsplit("@", 1)
            authorities = tuple(
                entry.rsplit("@", 1)[0] + ":" + entry.rsplit("@", 1)[1]
                for entry in proxy.provider_allowlist
            )
            self.provider_proxy = ProviderConnectProxy(
                listen_host,
                int(port_text),
                authorities,
                context.trial_id,
                self.attempt.ip_address,
            )
            self.provider_proxy_initial = self.provider_proxy.start()
            if (
                self.provider_proxy_initial.get("endpoint") != proxy.url
                or self.provider_proxy_initial.get("allowed_client_ip") != self.attempt.ip_address
                or self.provider_proxy_initial.get("allowed_authorities_digest")
                != "sha256:" + proxy.provider_allowlist_sha256
                or self.provider_proxy_initial.get("implementation_digest")
                != "sha256:" + proxy.implementation_sha256
                or self.provider_proxy_initial.get("active") is not True
                or self.provider_proxy_initial.get("sealed") is not False
            ):
                raise HarnessFailure("production provider proxy startup evidence mismatch")
        payload, payload_digest = _archive_payload(
            context,
            self.agent_command,
            self.launch,
            self.production_plan,
            self.selected_brief,
        )
        self.guest_root = self.control.stage_archive(
            self.attempt.vm_name,
            context.trial_id,
            payload,
            payload_digest,
        )
        network_mode = policy_network.get("mode", "full")
        network_evidence: dict[str, Any] = {
            "enforcer": "unavailable",
            "mode": "unknown",
            "allowlist_sha256": None,
            "evidence_digest": None,
        }
        if network_mode in {"none", "allowlist"}:
            network_evidence = self.control.apply_network(
                self.attempt.vm_name,
                self.guest_root,
                network_mode,
                policy_network.get("allowlist_sha256"),
            )
            self.network_active = True
        reset = self.control.run_reset(self.attempt.vm_name, self.guest_root)
        desktop_fixture = _desktop_fixture(context.task)
        shared_path: str | None = None
        if desktop_fixture is not None:
            shared_path, app_id, launch_arguments = desktop_fixture
            if self.certifying:
                store_evidence = self.control.prepare_task_store(
                    self.attempt.vm_name, self.guest_root, shared_path
                )
                self.protected_store_prepared = True
                self.task_app_evidence = self.control.launch_task_app(
                    self.attempt.vm_name,
                    self.guest_root,
                    app_id,
                    shared_path,
                    launch_arguments,
                    "protected-console-only",
                )
                self.mediator_start_evidence = self.control.start_mediator(
                    self.attempt.vm_name,
                    self.guest_root,
                    str(context.task["id"]),
                    (
                        self.production_plan.daemon_tool_schemas_sha256
                        if self.production_plan is not None
                        else None
                    ),
                    (
                        self.production_plan.daemon_tools_list_envelope_sha256
                        if self.production_plan is not None
                        else None
                    ),
                )
                if self.mediator_start_evidence.get("target_pid") != self.task_app_evidence.get(
                    "target_pid"
                ):
                    raise HarnessFailure("protected mediator target binding mismatch")
                mediator_evidence = self.mediator_start_evidence
                self.mediator_started = True
            else:
                self.control.share_console_path(self.attempt.vm_name, self.guest_root, shared_path)
                store_evidence = {"mode": "shared-agent-console"}
                self.task_app_evidence = self.control.launch_task_app(
                    self.attempt.vm_name,
                    self.guest_root,
                    app_id,
                    shared_path,
                    launch_arguments,
                    "shared-agent-console",
                )
                mediator_evidence = {"mediator_sha256": None}
        else:
            store_evidence = {"mode": "not-required"}
            mediator_evidence = {"mediator_sha256": None}
        human_input_evidence = self.attempt.apparatus_evidence.get("human_input", {})
        helper_evidence = self.attempt.apparatus_evidence.get("privileged_helper", {})
        policy = context.policy.policy if context.policy is not None else {}
        facts = {
            "variant": str(context.task["variants"][0]["id"]),
            "platform": "macos",
            "seed_provenance_digest": self.attempt.seed_provenance_digest,
            "pristine_fingerprint": self.attempt.pristine_fingerprint,
            "os_version": self.attempt.guest_facts.get("os_version"),
            "os_build": self.attempt.guest_facts.get("os_build"),
            "driver_version": self.attempt.guest_facts.get("driver_version"),
            "driver_sha256": self.attempt.guest_facts.get("driver_sha256"),
            "driver_team_id": self.attempt.guest_facts.get("driver_team_id"),
            "initial_driver_identity_digest": _driver_identity_digest(self.attempt.guest_facts),
            "target_reset": True,
            "reset_verified": reset.get("ok") is True,
            "cache_state": "empty",
            "persistent_state": "absent",
            "applied_network_mode": network_evidence["mode"],
            "applied_network_allowlist_sha256": network_evidence["allowlist_sha256"],
            "applied_permission_policy_sha256": policy.get("permissions", {}).get("policy_sha256"),
            "credential_state_profile_sha256": policy.get("credential_state_profile", {}).get(
                "sha256"
            ),
            "applications": self.attempt.guest_facts.get("applications"),
            "production_tools": self.attempt.guest_facts.get("production_tools"),
            "display": self.attempt.guest_facts.get("display_summary"),
            "network_enforcer": network_evidence["enforcer"],
            "network_evidence_digest": network_evidence["evidence_digest"],
            "human_input_channel": "closed-no-vnc",
            "human_input_enforcer": human_input_evidence.get("enforcer"),
            "human_input_evidence_digest": human_input_evidence.get("evidence_digest"),
            "privileged_helper_enforcer": helper_evidence.get("enforcer"),
            "privileged_helper_sha256": helper_evidence.get("helper_sha256"),
            "apparatus_enforcement_required": True,
            "enforcement": {
                "network": network_evidence,
                "provider_proxy": (
                    dict(self.provider_proxy_initial)
                    if self.provider_proxy_initial is not None
                    else None
                ),
                "human_input": dict(human_input_evidence),
                "privileged_helper": dict(helper_evidence),
                "lume_binary": dict(self.attempt.apparatus_evidence.get("lume_binary", {})),
            },
            "task_store_mode": store_evidence["mode"],
            "task_store_relative": shared_path,
            "task_app_target_pid": (
                self.task_app_evidence.get("target_pid")
                if self.task_app_evidence is not None
                else None
            ),
            "task_app_process_identity": (
                self.task_app_evidence.get("target_process_identity")
                if self.task_app_evidence is not None
                else None
            ),
            "task_store_path_sha256": (
                self.task_app_evidence.get("store_path_sha256")
                if self.task_app_evidence is not None
                else None
            ),
            "mediator_enforcer": (
                "guest-root-cdb-helper" if self.mediator_started else "unavailable"
            ),
            "mediator_sha256": mediator_evidence["mediator_sha256"],
            "expected_daemon_tool_schemas_sha256": mediator_evidence.get(
                "expected_tool_contract_sha256"
            ),
            "expected_daemon_tool_list_envelope_sha256": mediator_evidence.get(
                "expected_daemon_tool_list_envelope_sha256"
            ),
            "protected_endpoint_binding_digest": _endpoint_binding_digest(mediator_evidence),
            "certifying": self.certifying,
            "production_harness": (
                self.production_plan.kind.value if self.production_plan is not None else None
            ),
            "production_route_id": (
                self.production_plan.model_route.route_id
                if self.production_plan is not None
                else None
            ),
            "production_policy_digest": (
                self.production_plan.policy_digest if self.production_plan is not None else None
            ),
            "production_executable_sha256": (
                self.production_plan.guest_executable_sha256
                if self.production_plan is not None
                else None
            ),
            "production_tool_inventory_sha256": (
                self.production_plan.tool_inventory_sha256
                if self.production_plan is not None
                else None
            ),
            "expected_mcp_tool_schemas_sha256": (
                self.production_plan.mcp_tool_schemas_sha256
                if self.production_plan is not None
                else None
            ),
            "expected_daemon_tool_count": (
                self.production_plan.daemon_tool_count if self.production_plan is not None else None
            ),
            "expected_mcp_tool_count": (
                self.production_plan.mcp_tool_count if self.production_plan is not None else None
            ),
        }
        return EnvironmentHandle(
            kind=self.name,
            root=context.artifacts / "workspace",
            facts=facts,
        )

    def rendered_launch(self) -> Any:
        if self.guest_root is None or self.selected_brief is None:
            raise HarnessFailure("guest attempt has not been staged")
        values = {
            "agent": str(self.guest_root / "harness/agent" / self.agent_command.name),
            "workspace": str(self.guest_root / "workspace"),
            "artifacts": str(self.guest_root / "artifacts"),
            "home": str(self.guest_root / "home"),
            "brief": str(self.guest_root / self.selected_brief),
            "driver_socket": str(LumeControlPlane.protected_driver_socket(self.guest_root.name)),
        }
        rendered, _configs = _render_guest_launch(values, self.launch, self.production_plan)
        return rendered

    def export(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome,
    ) -> EnvironmentHandle:
        del outcome
        if self.exported:
            return handle
        if self.attempt is None or self.guest_root is None:
            raise HarnessFailure("guest attempt is unavailable for export")
        frozen = self.control.kill_agent(self.attempt.vm_name, self.guest_root.name)
        self.agent_terminated = True
        agent_processes_frozen = frozen.get("no_agent_processes") is True
        provider_proxy_summary: dict[str, Any] | None = None
        if self.provider_proxy is not None:
            try:
                self.provider_proxy_sealed = self.provider_proxy.seal()
                if self.provider_proxy_initial is None:
                    raise HarnessFailure(
                        "production provider proxy startup evidence is unavailable"
                    )
                proxy_artifact = context.artifacts / "provider-proxy.evidence.json"
                proxy_artifact.write_bytes(canonical_json(self.provider_proxy_sealed) + b"\n")
                provider_proxy_summary = _provider_proxy_summary(
                    self.provider_proxy_initial,
                    self.provider_proxy_sealed,
                    digest_file(proxy_artifact),
                )
            except Exception as error:  # noqa: BLE001 - export and grading continue
                self.provider_proxy_error = type(error).__name__
            else:
                self.provider_proxy = None
        self._persist_debug(context, provider_proxy_summary)
        post_run_facts = self.control.guest_facts(self.attempt.vm_name)
        post_run_fingerprint = digest_json(post_run_facts)
        post_run_driver_identity_digest = _driver_identity_digest(post_run_facts)
        initial_network = handle.facts.get("enforcement", {}).get("network", {})
        post_run_network: dict[str, Any] | None = None
        if self.network_active:
            post_run_network = self.control.verify_network(
                self.attempt.vm_name,
                self.guest_root,
                initial_network,
            )
        final_human_input = self.control.host_human_input_evidence(
            self.attempt.vm_name,
            self.control.inspect(self.attempt.vm_name),
        )
        initial_human_input = handle.facts.get("enforcement", {}).get("human_input", {})
        if dict(final_human_input) != dict(initial_human_input):
            raise HarnessFailure("protected human-input enforcement changed during attempt")
        if self.mediator_started:
            try:
                self.protected_report = self.control.stop_and_seal_mediator(
                    self.attempt.vm_name, self.guest_root
                )
                if self.task_app_evidence is None or self.mediator_start_evidence is None:
                    raise HarnessFailure("protected mediator setup evidence is unavailable")
                if self.protected_report.get("target_pid") != self.task_app_evidence.get(
                    "target_pid"
                ) or any(
                    self.protected_report.get(field) != self.mediator_start_evidence.get(field)
                    for field in (
                        "backend_parent_device",
                        "backend_parent_inode",
                        "backend_device",
                        "backend_inode",
                        "expected_tool_contract_sha256",
                        "tool_contract_required",
                        "expected_daemon_tool_list_envelope_sha256",
                        "daemon_tool_list_envelope_required",
                    )
                ):
                    raise HarnessFailure("protected mediator seal binding mismatch")
                expected_tool_contract = self.mediator_start_evidence.get(
                    "expected_tool_contract_sha256"
                )
                expected_daemon_envelope = self.mediator_start_evidence.get(
                    "expected_daemon_tool_list_envelope_sha256"
                )
                if self.production_plan is not None and (
                    self.protected_report.get("tool_contract_required") is not True
                    or self.protected_report.get("expected_tool_contract_sha256")
                    != expected_tool_contract
                    or self.protected_report.get("daemon_tool_list_envelope_required") is not True
                    or self.protected_report.get("expected_daemon_tool_list_envelope_sha256")
                    != expected_daemon_envelope
                    or self.protected_report.get("daemon_tool_list_envelope_validated") is not True
                    or self.protected_report.get("observed_daemon_tool_list_envelope_sha256")
                    != expected_daemon_envelope
                    or self.protected_report.get("tool_contract_validated") is not True
                    or self.protected_report.get("observed_tool_contract_sha256")
                    != expected_tool_contract
                ):
                    raise HarnessFailure("protected mediator tool contract validation failed")
            except Exception as error:  # noqa: BLE001 - outcome grading must continue
                self.protected_report = None
                self.participation_transport_error = type(error).__name__
            finally:
                self.mediator_started = False
        if self.network_active:
            self.control.remove_network(self.attempt.vm_name, self.guest_root)
            self.network_active = False
        self.control.stop(self.attempt.vm_name, 90.0)
        destination = context.artifacts / "workspace"
        report = self.control.collect_workspace(
            self.attempt.vm_name,
            self.guest_root / "workspace",
            destination,
            self.launch.collection,
        )
        manifest = context.artifacts / "collection.manifest.json"
        manifest.write_bytes(canonical_json(report.document()) + b"\n")
        protected_report_digest = None
        protected_log_digest = None
        protected_collection_manifest_digest = None
        protected_collection_read_only = False
        if self.protected_store_prepared:
            protected_destination = context.artifacts / "protected"
            protected_collection = self.control.collect_guest_tree(
                self.attempt.vm_name,
                PurePosixPath("/private/var/db/cdb-protected") / self.guest_root.name,
                protected_destination,
                CollectionBounds(
                    max_files=128,
                    max_total_bytes=32 * 1024 * 1024,
                    max_file_bytes=16 * 1024 * 1024,
                    max_depth=8,
                ),
            )
            protected_manifest = context.artifacts / "protected-collection.manifest.json"
            protected_manifest.write_bytes(canonical_json(protected_collection.document()) + b"\n")
            protected_collection_manifest_digest = digest_file(protected_manifest)
            protected_collection_read_only = True
            candidate_log = protected_destination / "mediator.ndjson"
            if candidate_log.is_file() and not candidate_log.is_symlink():
                self.protected_log_path = candidate_log
                protected_log_digest = digest_file(candidate_log)
            elif self.protected_report is not None:
                self.participation_transport_error = "sealed_log_missing"
                self.protected_report = None
            candidate_report = protected_destination / "participation.sealed.json"
            if self.protected_report is not None:
                if candidate_report.is_file() and not candidate_report.is_symlink():
                    self.protected_report_path = candidate_report
                    protected_report_digest = digest_file(candidate_report)
            task_store_relative = handle.facts.get("task_store_relative")
            if not isinstance(task_store_relative, str):
                raise HarnessFailure("protected task-store binding is unavailable")
            untrusted_store = destination / task_store_relative
            protected_store = protected_destination / task_store_relative
            if (
                not untrusted_store.is_dir()
                or untrusted_store.is_symlink()
                or not protected_store.is_dir()
                or protected_store.is_symlink()
            ):
                raise HarnessFailure("protected task state is unavailable after collection")
            untrusted_store.rename(context.artifacts / "untrusted-task-store")
            protected_store.rename(destination / task_store_relative)
        self.exported = True
        return EnvironmentHandle(
            kind=handle.kind,
            root=destination,
            facts={
                **handle.facts,
                "vm_stopped_before_collection": True,
                "collection_manifest_digest": digest_file(manifest),
                "protected_collection_manifest_digest": (protected_collection_manifest_digest),
                "protected_collection_read_only": protected_collection_read_only,
                "protected_log_digest": protected_log_digest,
                "protected_report_digest": protected_report_digest,
                "participation_transport_error": self.participation_transport_error,
                "provider_proxy_error": self.provider_proxy_error,
                "provider_proxy": provider_proxy_summary,
                "enforcement": {
                    **handle.facts.get("enforcement", {}),
                    "provider_proxy": provider_proxy_summary,
                },
                "agent_processes_frozen": agent_processes_frozen,
                "post_run_fingerprint": post_run_fingerprint,
                "post_run_matches_pristine": (
                    post_run_fingerprint == handle.facts.get("pristine_fingerprint")
                ),
                "post_run_driver_identity_digest": (post_run_driver_identity_digest),
                "post_run_network_evidence_digest": (
                    post_run_network.get("evidence_digest")
                    if post_run_network is not None
                    else None
                ),
                "post_run_human_input_evidence_digest": final_human_input.get("evidence_digest"),
                "sealed_endpoint_binding_digest": _endpoint_binding_digest(
                    self.protected_report or {}
                ),
                "protected_log_tail": (
                    self.protected_report.get("event_log_tail")
                    if self.protected_report is not None
                    else None
                ),
                "protected_log_records": (
                    self.protected_report.get("records")
                    if self.protected_report is not None
                    else None
                ),
                "protected_transport_integrity": (
                    self.protected_report.get("transport_integrity")
                    if self.protected_report is not None
                    else None
                ),
                "protected_evidence_complete": (
                    self.protected_report.get("evidence_complete")
                    if self.protected_report is not None
                    else None
                ),
                "protected_off_target_activity": (
                    self.protected_report.get("off_target_activity")
                    if self.protected_report is not None
                    else None
                ),
                "protected_tool_contract_validated": (
                    self.protected_report.get("tool_contract_validated")
                    if self.protected_report is not None
                    else None
                ),
                "observed_daemon_tool_schemas_sha256": (
                    self.protected_report.get("observed_tool_contract_sha256")
                    if self.protected_report is not None
                    else None
                ),
                "observed_daemon_tool_list_envelope_sha256": (
                    self.protected_report.get("observed_daemon_tool_list_envelope_sha256")
                    if self.protected_report is not None
                    else None
                ),
                "protected_daemon_tool_list_envelope_validated": (
                    self.protected_report.get("daemon_tool_list_envelope_validated")
                    if self.protected_report is not None
                    else None
                ),
                "protected_daemon_tool_count": (
                    self.protected_report.get("tool_contract_tool_count")
                    if self.protected_report is not None
                    else None
                ),
            },
        )

    def cleanup(
        self,
        context: TrialContext,
        handle: EnvironmentHandle | None,
        timeout_seconds: float,
    ) -> CleanupReport:
        del context, handle, timeout_seconds
        errors: list[str] = []
        if self.provider_proxy is not None:
            try:
                # Revoke the only permitted provider route before any guest
                # cleanup RPC can block.  close() below retains the normal
                # quiescence/evidence barrier after containment.
                self.provider_proxy.abort()
            except Exception as error:  # noqa: BLE001 - cleanup continues
                errors.append(f"provider proxy abort: {type(error).__name__}: {error}")
        vm_name = self.attempt.vm_name if self.attempt is not None else self.pending_vm_name
        if vm_name is None:
            if self.provider_proxy is not None:
                try:
                    self.provider_proxy.close()
                    self.provider_proxy = None
                except Exception as error:  # noqa: BLE001 - cleanup continues
                    errors.append(f"provider proxy cleanup: {type(error).__name__}: {error}")
            return CleanupReport(
                ok=not errors,
                error="; ".join(errors) if errors else None,
            )
        # A proxy seal drains active CONNECT tunnels.  The route is already
        # revoked above, so contain the credential-bearing guest before the
        # quiescence barrier; proxy evidence is sealed during normal export.
        # Normal export freezes the agent before it stops the VM and starts
        # read-only collection. Do not issue a second SSH kill against that
        # stopped guest, even when a later collection step fails.
        agent_terminated = self.guest_root is None or self.agent_terminated
        if self.guest_root is not None and not self.agent_terminated:
            try:
                self.control.kill_agent(vm_name, self.guest_root.name)
                self.agent_terminated = True
                agent_terminated = True
            except Exception as error:  # noqa: BLE001 - cleanup continues
                errors.append(f"agent cleanup: {type(error).__name__}: {error}")
        try:
            # Never release PF containment while a credential-bearing process
            # may still be alive.  If termination failed, VM destruction runs
            # with the guest policy and revoked host proxy still in force.
            if agent_terminated and self.network_active and self.guest_root is not None:
                self.control.remove_network(vm_name, self.guest_root)
                self.network_active = False
        except Exception as error:  # noqa: BLE001 - cleanup continues to destruction
            errors.append(f"network cleanup: {type(error).__name__}: {error}")
        try:
            self.control.destroy(vm_name)
            self.network_active = False
        except Exception as error:  # noqa: BLE001 - cleanup report is the boundary
            errors.append(f"VM cleanup: {type(error).__name__}: {error}")
        # Do not attempt a late mediator seal after containment: it requires a
        # live guest, and an unsealed report is non-certifying by construction.
        if self.mediator_started:
            self.mediator_started = False
            self.protected_report = None
            self.participation_transport_error = (
                self.participation_transport_error or "cleanup_destroyed_before_mediator_seal"
            )
        if self.provider_proxy is not None:
            try:
                self.provider_proxy.close()
                self.provider_proxy = None
            except Exception as error:  # noqa: BLE001 - containment is complete
                errors.append(f"provider proxy cleanup: {type(error).__name__}: {error}")
        return CleanupReport(
            ok=not errors,
            reclaimed=(vm_name,),
            error="; ".join(errors) if errors else None,
        )


class LumeGuestHarness:
    name = "lume-guest"

    def __init__(
        self,
        environment: LumeMacosEnvironment,
        agent_command: Path,
        expected_digest: str,
        credential_lease: CredentialLease | None = None,
    ) -> None:
        self.environment = environment
        self.agent_command = agent_command
        self.expected_digest = expected_digest
        self.credential_lease = credential_lease

    def run(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> AgentOutcome:
        del handle
        if digest_file(self.agent_command) != self.expected_digest:
            raise ValidationFailure("materialized guest agent command digest changed")
        if self.environment.attempt is None:
            raise HarnessFailure("guest attempt is unavailable")
        interrupt.raise_if_requested()
        launch = self.environment.rendered_launch()
        started = time.monotonic()
        context.emit(
            "process_spawned",
            {"role": "agent", "argv0": Path(launch.argv[0]).name},
        )
        credential_environment: dict[str, str] | None = None
        try:
            if self.environment.production_plan is not None:
                if self.environment.production_plan.credential_environment:
                    if self.credential_lease is None:
                        raise HarnessFailure("production credential lease is unavailable")
                    credential_environment = self.credential_lease.consume(
                        provider=self.environment.production_plan.model_route.provider,
                        harness=self.environment.production_plan.kind.value,
                    )
                elif self.credential_lease is not None:
                    raise HarnessFailure(
                        "credential lease is forbidden for an anonymous production route"
                    )
            elif self.credential_lease is not None:
                raise HarnessFailure("credential lease is forbidden for a legacy launch")
            try:
                result = self.environment.control.run_agent(
                    self.environment.attempt.vm_name,
                    launch,
                    timeout_seconds,
                    credential_environment=credential_environment,
                )
            except DeadlineExceeded:
                if (
                    getattr(self.environment, "debug_mode", False)
                    and self.environment.production_plan
                ):
                    try:
                        debug_result = self.environment.control.agent_output(
                            self.environment.attempt.vm_name,
                            launch,
                        )
                        parsed_debug = _parse_production_output(
                            debug_result, self.environment.production_plan
                        )
                        self.environment.capture_debug_output(
                            context,
                            debug_result,
                            parsed_debug,
                            classification="timeout",
                        )
                    except Exception as error:  # noqa: BLE001 - preserve timeout
                        self.environment.capture_debug_unavailable(
                            context,
                            classification="timeout",
                            error_type=type(error).__name__,
                        )
                raise
        finally:
            if credential_environment is not None:
                credential_environment.clear()
            if self.credential_lease is not None:
                self.credential_lease.destroy()
        duration_ms = round((time.monotonic() - started) * 1000)
        production = self.environment.production_plan
        persisted_artifacts: list[str] = ["agent.output.json"]
        if production is None:
            (context.artifacts / "agent.stdout").write_text(
                result.stdout[-65536:], encoding="utf-8"
            )
            (context.artifacts / "agent.stderr").write_text(
                result.stderr[-65536:], encoding="utf-8"
            )
            persisted_artifacts.extend(("agent.stderr", "agent.stdout"))
        output_evidence = _output_evidence(result)
        (context.artifacts / "agent.output.json").write_bytes(
            canonical_json(output_evidence) + b"\n"
        )
        if production is not None:
            parsed = _parse_production_output(result, production)
            telemetry = list(parsed.telemetry)
            telemetry_evidence = {
                "events": telemetry,
                "parsed_event_count": len(telemetry),
                "skipped_line_count": parsed.skipped_lines,
                "raw_stdout_bytes": result.stdout_bytes,
                "raw_stdout_sha256": f"sha256:{result.stdout_sha256}",
                "raw_stderr_bytes": result.stderr_bytes,
                "raw_stderr_sha256": f"sha256:{result.stderr_sha256}",
                "raw_output_persisted": False,
            }
            (context.artifacts / "agent.telemetry.json").write_bytes(
                canonical_json(telemetry_evidence) + b"\n"
            )
            persisted_artifacts.append("agent.telemetry.json")
            terminal_failure = parsed.terminal_failure
            usage_event = parsed.usage_event
            if getattr(self.environment, "debug_mode", False):
                self.environment.capture_debug_output(
                    context,
                    result,
                    parsed,
                    classification=("completed" if result.returncode == 0 else "failed"),
                )
            if context.policy is not None:
                route = production.model_route
                context.policy.record_model_call(
                    route_id=route.route_id,
                    role=route.role,
                    provider=route.provider,
                    model=route.model,
                    snapshot=route.snapshot,
                    service_tier=route.service_tier,
                    tokens=(
                        {
                            "input": int(usage_event["input_tokens"]),
                            "output": int(usage_event["output_tokens"]),
                            "cache_read": int(usage_event["cache_read_tokens"]),
                            "cache_write": int(usage_event["cache_write_tokens"]),
                        }
                        if usage_event is not None
                        else None
                    ),
                    cost_usd=None,
                    trust="non_certifying",
                    includes_subagents=(
                        usage_event["includes_subagents"] if usage_event is not None else None
                    ),
                )
        else:
            terminal_failure = None
        if result.returncode != 0 and terminal_failure is None:
            terminal_failure = "harness_failed"
        process_exit = {
            "role": "agent",
            "exit_code": result.returncode,
            "truncated": bool(result.stdout_truncated or result.stderr_truncated),
            "output": output_evidence,
        }
        if terminal_failure is not None:
            process_exit["terminal_failure"] = terminal_failure
        context.emit("process_exited", process_exit)
        interrupt.raise_if_requested()
        return AgentOutcome(
            completed=result.returncode == 0,
            exit_code=result.returncode,
            duration_ms=duration_ms,
            artifacts=tuple(persisted_artifacts),
            terminal_failure=terminal_failure,
        )
