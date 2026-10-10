"""Protected host control plane for disposable macOS Lume attempts."""

from __future__ import annotations

import base64
import hashlib
import ipaddress
import json
import os
import plistlib
import re
import shlex
import signal
import stat
import subprocess
import tempfile
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from cua_bench_runtime.canon import canonical_json, digest_file, digest_json
from cua_bench_runtime.collect import CollectionReport, collect_tree
from cua_bench_runtime.errors import DeadlineExceeded, HarnessFailure, ValidationFailure
from cua_bench_runtime.guest_launch import CollectionBounds, RenderedGuestLaunch
from cua_bench_runtime.process import clean_environment

VM_NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
IMAGE_DIGEST = re.compile(r"^sha256:[a-f0-9]{64}$")
INSPECT_PARSE_ATTEMPTS = 3
DEFAULT_DRIVER_PATH = PurePosixPath("/Users/lume/.local/bin/cua-driver")
DEFAULT_PRIVILEGED_HELPER_PATH = PurePosixPath("/usr/local/libexec/cdb-helper")
LAUNCH_STAGE_CODES = frozenset(
    {
        "launch-stage-agent-account",
        "launch-stage-codex-auth-path",
        "launch-stage-codex-auth-validate",
        "launch-stage-credential-policy",
        "launch-stage-credential-read",
        "launch-stage-envelope-load",
        "launch-stage-fork",
        "launch-stage-output-init",
        "launch-stage-pipe",
        "launch-stage-state-commit",
        "launch-stage-state-load",
        "launch-stage-stdin-open",
    }
)


def credential_launch_stage(stderr: str) -> str | None:
    for code in LAUNCH_STAGE_CODES:
        if stderr in {f"cdb-helper: {code}", f"cdb-helper: {code}\n"}:
            return code
    return None


@dataclass(frozen=True)
class CommandResult:
    returncode: int | None
    stdout: str
    stderr: str
    stdout_bytes: int | None = None
    stdout_sha256: str | None = None
    stdout_truncated: bool | None = None
    stderr_bytes: int | None = None
    stderr_sha256: str | None = None
    stderr_truncated: bool | None = None


@dataclass(frozen=True)
class LumeAttempt:
    vm_name: str
    ip_address: str
    seed_provenance_digest: str
    pristine_fingerprint: str
    guest_facts: Mapping[str, Any]
    apparatus_evidence: Mapping[str, Any]


Runner = Callable[[Sequence[str], float], CommandResult]
InputRunner = Callable[[Sequence[str], float, bytes], CommandResult]


def _subprocess_group_options() -> dict[str, Any]:
    if os.name == "nt":
        return {"creationflags": getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0x00000200)}
    return {"start_new_session": True}


def subprocess_runner(argv: Sequence[str], timeout_seconds: float) -> CommandResult:
    try:
        completed = subprocess.run(
            list(argv),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=clean_environment(),
            shell=False,
            timeout=timeout_seconds,
            check=False,
            **_subprocess_group_options(),
        )
    except subprocess.TimeoutExpired:
        return CommandResult(124, "", "host deadline exceeded")
    except OSError as error:
        raise HarnessFailure(f"Lume command failed to run: {argv[0]}") from error
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def subprocess_input_runner(
    argv: Sequence[str], timeout_seconds: float, input_bytes: bytes
) -> CommandResult:
    try:
        completed = subprocess.run(
            list(argv),
            input=input_bytes,
            capture_output=True,
            text=False,
            env=clean_environment(),
            shell=False,
            timeout=timeout_seconds,
            check=False,
            **_subprocess_group_options(),
        )
    except subprocess.TimeoutExpired:
        return CommandResult(124, "", "host deadline exceeded")
    except OSError as error:
        raise HarnessFailure(f"Lume command failed to run: {argv[0]}") from error
    return CommandResult(
        completed.returncode,
        completed.stdout.decode("utf-8", errors="replace"),
        completed.stderr.decode("utf-8", errors="replace"),
    )


class LumeControlPlane:
    """Create one stopped-seed clone per attempt and attest its guest state."""

    def __init__(
        self,
        *,
        binary: Path,
        binary_sha256: str,
        seed_vm: str,
        seed_provenance_digest: str,
        seed_manifest_path: Path,
        driver_path: PurePosixPath | str = DEFAULT_DRIVER_PATH,
        driver_version: str | None = None,
        driver_sha256: str | None = None,
        driver_team_id: str = "YCK386LBJ7",
        ssh_user: str = "lume",
        ssh_identity_file: Path,
        ssh_known_hosts_file: Path,
        ssh_known_hosts_sha256: str,
        ssh_host_key_alias: str,
        privileged_helper_path: PurePosixPath | str = DEFAULT_PRIVILEGED_HELPER_PATH,
        privileged_helper_sha256: str,
        mediator_sha256: str,
        privileged_sudoers_sha256: str,
        pf_main_rules_sha256: str,
        network_allowlist: Sequence[str] = (),
        production_tools: Mapping[str, str] | None = None,
        console_uid: int = 501,
        expected_pristine_fingerprint: str,
        storage_root: Path | None = None,
        runner: Runner = subprocess_runner,
        input_runner: InputRunner = subprocess_input_runner,
        poll_interval_seconds: float = 1.0,
    ) -> None:
        if not binary.is_absolute() or not binary.is_file():
            raise ValidationFailure("Lume binary path must be an absolute file")
        if not IMAGE_DIGEST.fullmatch(binary_sha256):
            raise ValidationFailure("Lume binary digest must be sha256-prefixed")
        if digest_file(binary) != binary_sha256:
            raise ValidationFailure("Lume binary digest mismatch")
        self.binary = binary.resolve()
        self.binary_sha256 = binary_sha256
        self.seed_vm = self._name(seed_vm, "seed VM")
        if not IMAGE_DIGEST.fullmatch(seed_provenance_digest):
            raise ValidationFailure("Lume seed provenance digest must be sha256-prefixed")
        self.seed_provenance_digest = seed_provenance_digest
        if not seed_manifest_path.is_absolute():
            raise ValidationFailure("Lume seed manifest path must be absolute")
        self.seed_manifest_path = seed_manifest_path
        driver_path = PurePosixPath(driver_path)
        if not driver_path.is_absolute():
            raise ValidationFailure("guest Cua Driver path must be absolute")
        self.driver_path = driver_path
        if not isinstance(driver_version, str) or not driver_version:
            raise ValidationFailure("guest Cua Driver version pin is required")
        self.driver_version = driver_version
        if not isinstance(driver_sha256, str) or not IMAGE_DIGEST.fullmatch(driver_sha256):
            raise ValidationFailure("guest Cua Driver digest must be sha256-prefixed")
        self.driver_sha256 = driver_sha256
        if not re.fullmatch(r"[A-Z0-9]{10}", driver_team_id):
            raise ValidationFailure("guest Cua Driver Team ID is invalid")
        self.driver_team_id = driver_team_id
        if not re.fullmatch(r"[a-z_][a-z0-9_-]{0,31}", ssh_user):
            raise ValidationFailure("Lume SSH user is invalid")
        if not ssh_identity_file.is_absolute() or not ssh_identity_file.is_file():
            raise ValidationFailure("Lume SSH identity must be an absolute file")
        if os.name != "nt" and stat.S_IMODE(ssh_identity_file.stat().st_mode) != 0o600:
            raise ValidationFailure("Lume SSH identity must have mode 0600")
        if os.name != "nt" and ssh_identity_file.stat().st_uid != os.geteuid():
            raise ValidationFailure("Lume SSH identity must be owned by the runtime user")
        if not ssh_known_hosts_file.is_absolute() or not ssh_known_hosts_file.is_file():
            raise ValidationFailure("Lume known_hosts must be an absolute file")
        if os.name != "nt" and stat.S_IMODE(ssh_known_hosts_file.stat().st_mode) & 0o022:
            raise ValidationFailure("Lume known_hosts must not be group/world writable")
        if not IMAGE_DIGEST.fullmatch(ssh_known_hosts_sha256):
            raise ValidationFailure("Lume known_hosts digest must be sha256-prefixed")
        if digest_file(ssh_known_hosts_file) != ssh_known_hosts_sha256:
            raise ValidationFailure("Lume known_hosts digest mismatch")
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", ssh_host_key_alias):
            raise ValidationFailure("Lume SSH host-key alias is invalid")
        privileged_helper_path = PurePosixPath(privileged_helper_path)
        if not privileged_helper_path.is_absolute():
            raise ValidationFailure("guest privileged helper path must be absolute")
        if not isinstance(privileged_helper_sha256, str) or not IMAGE_DIGEST.fullmatch(
            privileged_helper_sha256
        ):
            raise ValidationFailure("guest privileged helper digest must be sha256-prefixed")
        if not isinstance(privileged_sudoers_sha256, str) or not IMAGE_DIGEST.fullmatch(
            privileged_sudoers_sha256
        ):
            raise ValidationFailure("guest privileged sudoers digest must be sha256-prefixed")
        if not isinstance(pf_main_rules_sha256, str) or not IMAGE_DIGEST.fullmatch(
            pf_main_rules_sha256
        ):
            raise ValidationFailure("guest main PF ruleset digest must be sha256-prefixed")
        if ssh_user != "lume":
            raise ValidationFailure("protected macOS control account must be lume")
        if not isinstance(console_uid, int) or console_uid < 1:
            raise ValidationFailure("Lume console UID is invalid")
        self.ssh_user = ssh_user
        self.ssh_identity_file = ssh_identity_file.resolve()
        self.ssh_known_hosts_file = ssh_known_hosts_file.resolve()
        self.ssh_known_hosts_sha256 = ssh_known_hosts_sha256
        self.ssh_host_key_alias = ssh_host_key_alias
        self.privileged_helper_path = privileged_helper_path
        self.privileged_helper_sha256 = privileged_helper_sha256
        if not isinstance(mediator_sha256, str) or not IMAGE_DIGEST.fullmatch(mediator_sha256):
            raise ValidationFailure("guest protected mediator digest must be sha256-prefixed")
        self.mediator_sha256 = mediator_sha256
        self.privileged_sudoers_sha256 = privileged_sudoers_sha256
        self.pf_main_rules_sha256 = pf_main_rules_sha256
        normalized_allowlist: list[str] = []
        for endpoint in network_allowlist:
            if not isinstance(endpoint, str) or endpoint.count("@") != 1:
                raise ValidationFailure("network allowlist endpoint is invalid")
            address_text, port_text = endpoint.rsplit("@", 1)
            try:
                address = str(ipaddress.ip_address(address_text))
                port = int(port_text)
            except (ValueError, TypeError) as error:
                raise ValidationFailure("network allowlist endpoint is invalid") from error
            if (
                not 1 <= port <= 65535
                or port in {53, 5353}
                or ipaddress.ip_address(address).is_loopback
            ):
                raise ValidationFailure("network allowlist endpoint is invalid")
            normalized_allowlist.append(f"{address}@{port}")
        self.network_allowlist = tuple(sorted(set(normalized_allowlist)))
        raw_production_tools = production_tools or {}
        allowed_production_tools = {
            "codex",
            "claude",
            "opencode",
            "cua-driver",
            "codex-code-mode-host",
        }
        if (
            not isinstance(raw_production_tools, Mapping)
            or not set(raw_production_tools) <= allowed_production_tools
            or any(
                not isinstance(name, str)
                or not isinstance(digest, str)
                or not IMAGE_DIGEST.fullmatch(digest)
                for name, digest in raw_production_tools.items()
            )
        ):
            raise ValidationFailure("guest production tool pins are invalid")
        self.production_tools = dict(sorted(raw_production_tools.items()))
        self.console_uid = console_uid
        if not IMAGE_DIGEST.fullmatch(expected_pristine_fingerprint):
            raise ValidationFailure("Lume expected pristine fingerprint must be sha256-prefixed")
        self.expected_pristine_fingerprint = expected_pristine_fingerprint
        self.storage_root = (
            storage_root.resolve()
            if storage_root is not None
            else (Path.home() / ".lume").resolve()
        )
        if not self.storage_root.is_absolute():
            raise ValidationFailure("Lume storage root must be absolute")
        self.runner = runner
        self.input_runner = input_runner
        self.poll_interval_seconds = poll_interval_seconds
        self._vm_addresses: dict[str, str] = {}

    @staticmethod
    def _name(value: str, label: str) -> str:
        if not VM_NAME.fullmatch(value):
            raise ValidationFailure(f"unsafe {label} name: {value}")
        return value

    def _run(self, arguments: Sequence[str], timeout_seconds: float = 60.0) -> str:
        result = self.runner([str(self.binary), *arguments], timeout_seconds)
        if result.returncode != 0:
            detail = (result.stderr.strip() or result.stdout.strip())[-1000:]
            raise HarnessFailure(f"lume {arguments[0]} failed with {result.returncode}: {detail}")
        return result.stdout

    def inspect(self, vm_name: str) -> dict[str, Any]:
        vm_name = self._name(vm_name, "VM")
        for attempt in range(INSPECT_PARSE_ATTEMPTS):
            output = self._run(("get", vm_name, "--format", "json"))
            try:
                document = json.loads(output)
                records = document if isinstance(document, list) else [document]
                if len(records) != 1 or not isinstance(records[0], dict):
                    raise ValueError("expected exactly one VM record")
                record = records[0]
                break
            except (json.JSONDecodeError, ValueError) as error:
                if attempt + 1 == INSPECT_PARSE_ATTEMPTS:
                    raise HarnessFailure(f"lume get returned invalid JSON for {vm_name}") from error
                time.sleep(min(max(self.poll_interval_seconds, 0.0), 1.0))
        if record.get("name") != vm_name:
            raise HarnessFailure(f"lume get returned the wrong VM for {vm_name}")
        return record

    def exists(self, vm_name: str) -> bool:
        """Return absence only after a successful, parseable inventory read."""

        vm_name = self._name(vm_name, "VM")
        for attempt in range(INSPECT_PARSE_ATTEMPTS):
            output = self._run(("ls", "--format", "json"))
            try:
                records = json.loads(output)
                if not isinstance(records, list) or any(
                    not isinstance(record, dict) or not isinstance(record.get("name"), str)
                    for record in records
                ):
                    raise ValueError("expected a list of named VM records")
                return any(record["name"] == vm_name for record in records)
            except (json.JSONDecodeError, ValueError) as error:
                if attempt + 1 == INSPECT_PARSE_ATTEMPTS:
                    detail = (
                        "invalid JSON"
                        if isinstance(error, json.JSONDecodeError)
                        else "invalid VM records"
                    )
                    raise HarnessFailure(f"lume ls returned {detail}") from error
                time.sleep(min(max(self.poll_interval_seconds, 0.0), 1.0))
        raise AssertionError("unreachable")

    def verify_seed_image(self) -> None:
        try:
            actual = self.seed_manifest_path.read_text(encoding="ascii").strip()
        except (OSError, UnicodeError) as error:
            raise HarnessFailure("cannot read the Lume seed provenance manifest") from error
        if actual != self.seed_provenance_digest:
            raise HarnessFailure(
                "Lume seed provenance digest does not match the finalized seed seal"
            )

    def _wait_ready(self, vm_name: str, timeout_seconds: float) -> dict[str, Any]:
        deadline = time.monotonic() + timeout_seconds
        last: dict[str, Any] = {}
        stopped_since: float | None = None
        while time.monotonic() < deadline:
            try:
                last = self.inspect(vm_name)
            except HarnessFailure:
                time.sleep(self.poll_interval_seconds)
                continue
            if last.get("status") == "stopped":
                stopped_since = stopped_since or time.monotonic()
                stopped_limit = min(10.0, max(0.001, timeout_seconds / 2))
                if time.monotonic() - stopped_since >= stopped_limit:
                    raise HarnessFailure(f"Lume VM {vm_name} remained stopped after launch")
            else:
                stopped_since = None
            if (
                last.get("status") == "running"
                and last.get("sshAvailable") is True
                and isinstance(last.get("ipAddress"), str)
                and last.get("vncUrl") is None
                and last.get("networkMode") == "nat"
            ):
                return last
            time.sleep(self.poll_interval_seconds)
        if last.get("status") == "stopped":
            raise HarnessFailure(f"Lume VM {vm_name} remained stopped after launch")
        raise HarnessFailure(
            f"Lume VM {vm_name} did not become SSH-ready; last status={last.get('status')}"
        )

    def ssh(self, vm_name: str, command: str, timeout_seconds: float = 60.0) -> str:
        result = self.ssh_result(vm_name, command, timeout_seconds)
        if result.returncode:
            detail = (result.stderr.strip() or result.stdout.strip())[-1000:]
            raise HarnessFailure(f"protected SSH command failed with {result.returncode}: {detail}")
        return result.stdout

    def _ssh_arguments(
        self,
        vm_name: str,
        command: str,
        timeout_seconds: float,
        *,
        input_enabled: bool,
    ) -> tuple[str, ...]:
        vm_name = self._name(vm_name, "VM")
        if digest_file(self.ssh_known_hosts_file) != self.ssh_known_hosts_sha256:
            raise HarnessFailure("Lume known_hosts changed during the attempt")
        address = self._vm_addresses.get(vm_name)
        if address is None:
            record = self.inspect(vm_name)
            candidate = record.get("ipAddress")
            if not isinstance(candidate, str) or not candidate:
                raise HarnessFailure("Lume VM has no SSH address")
            address = candidate
            self._vm_addresses[vm_name] = address
        arguments = ["/usr/bin/ssh", "-F", "/dev/null"]
        if not input_enabled:
            arguments.append("-n")
        arguments.extend(
            (
                "-i",
                str(self.ssh_identity_file),
                "-o",
                "BatchMode=yes",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "PasswordAuthentication=no",
                "-o",
                "KbdInteractiveAuthentication=no",
                "-o",
                "IdentityAgent=none",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                f"UserKnownHostsFile={self.ssh_known_hosts_file}",
                "-o",
                "GlobalKnownHostsFile=/dev/null",
                "-o",
                "UpdateHostKeys=no",
                "-o",
                f"HostKeyAlias={self.ssh_host_key_alias}",
                "-o",
                "LogLevel=ERROR",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                f"ConnectTimeout={max(1, min(60, int(timeout_seconds)))}",
                f"{self.ssh_user}@{address}",
                command,
            )
        )
        return tuple(arguments)

    def ssh_result(
        self, vm_name: str, command: str, timeout_seconds: float = 60.0
    ) -> CommandResult:
        result = self.runner(
            self._ssh_arguments(vm_name, command, timeout_seconds, input_enabled=False),
            timeout_seconds + 5.0,
        )
        return result

    def ssh_result_with_input(
        self,
        vm_name: str,
        command: str,
        input_bytes: bytes,
        timeout_seconds: float = 60.0,
    ) -> CommandResult:
        if not isinstance(input_bytes, bytes) or not input_bytes or len(input_bytes) > 32768:
            raise ValidationFailure("protected SSH input size is invalid")
        return self.input_runner(
            self._ssh_arguments(vm_name, command, timeout_seconds, input_enabled=True),
            timeout_seconds + 5.0,
            input_bytes,
        )

    def helper_result(
        self,
        vm_name: str,
        verb: str,
        arguments: Sequence[str] = (),
        timeout_seconds: float = 60.0,
    ) -> CommandResult:
        if verb not in {
            "stage-attempt",
            "reset-attempt",
            "share-console-path",
            "prepare-task-store",
            "launch-task-app",
            "mediator-start",
            "mediator-stop-and-seal",
            "launch-agent",
            "launch-agent-with-lease",
            "wait-agent",
            "kill-agent",
            "network-apply",
            "network-status",
            "network-remove",
            "shutdown",
            "start-driver",
            "installation-facts",
        }:
            raise ValidationFailure("unsupported privileged helper verb")
        if any(not isinstance(value, str) or not value or "\x00" in value for value in arguments):
            raise ValidationFailure("invalid privileged helper argument")
        command = " ".join(
            shlex.quote(value)
            for value in (
                "/usr/bin/sudo",
                "-n",
                "--",
                str(self.privileged_helper_path),
                verb,
                *arguments,
            )
        )
        return self.ssh_result(vm_name, command, timeout_seconds)

    def helper_result_with_input(
        self,
        vm_name: str,
        verb: str,
        arguments: Sequence[str],
        input_bytes: bytes,
        timeout_seconds: float = 60.0,
    ) -> CommandResult:
        if verb != "launch-agent-with-lease":
            raise ValidationFailure("privileged helper input verb is unsupported")
        if any(not isinstance(value, str) or not value or "\x00" in value for value in arguments):
            raise ValidationFailure("invalid privileged helper argument")
        command = " ".join(
            shlex.quote(value)
            for value in (
                "/usr/bin/sudo",
                "-n",
                "--",
                str(self.privileged_helper_path),
                verb,
                *arguments,
            )
        )
        return self.ssh_result_with_input(vm_name, command, input_bytes, timeout_seconds)

    def helper(
        self,
        vm_name: str,
        verb: str,
        arguments: Sequence[str] = (),
        timeout_seconds: float = 60.0,
    ) -> str:
        result = self.helper_result(vm_name, verb, arguments, timeout_seconds)
        if result.returncode:
            detail = (result.stderr.strip() or result.stdout.strip())[-1000:]
            raise HarnessFailure(
                f"privileged helper {verb} failed with {result.returncode}: {detail}"
            )
        return result.stdout

    @staticmethod
    def _helper_document(output: str, verb: str) -> dict[str, Any]:
        try:
            document = json.loads(output.splitlines()[-1])
        except (IndexError, json.JSONDecodeError) as error:
            raise HarnessFailure(f"privileged helper {verb} returned invalid evidence") from error
        if not isinstance(document, dict):
            raise HarnessFailure(f"privileged helper {verb} returned invalid evidence")
        return document

    def apply_network(
        self,
        vm_name: str,
        root: PurePosixPath,
        mode: str,
        expected_allowlist_sha256: str | None,
    ) -> dict[str, Any]:
        if mode not in {"none", "allowlist"}:
            raise ValidationFailure("protected macOS network mode must be none or allowlist")
        entries = self.network_allowlist if mode == "allowlist" else ()
        output = self.helper(
            vm_name,
            "network-apply",
            (
                root.name,
                mode,
                self.pf_main_rules_sha256.removeprefix("sha256:"),
                *entries,
            ),
            30.0,
        )
        document = self._helper_document(output, "network-apply")
        if (
            document.get("verb") != "network-apply"
            or document.get("attempt") != root.name
            or document.get("mode") != mode
        ):
            raise HarnessFailure("privileged helper network evidence binding mismatch")
        allowlist_digest = document.get("allowlist_sha256")
        if mode == "allowlist" and allowlist_digest != expected_allowlist_sha256:
            raise HarnessFailure("applied network allowlist digest mismatch")
        body = {
            "enforcer": "guest-root-pf-anchor",
            "attempt": root.name,
            "vm_name": self._name(vm_name, "VM"),
            "mode": mode,
            "allowlist_sha256": allowlist_digest if mode == "allowlist" else None,
            "rules_sha256": document.get("rules_sha256"),
            "ruleset_main_sha256": document.get("ruleset_main_sha256"),
            "anchor_rules_sha256": document.get("anchor_rules_sha256"),
            "pf_enabled": document.get("pf_enabled"),
            "residual_channels": document.get("residual_channels"),
            "global_protocol_blocks": document.get("global_protocol_blocks"),
        }
        if not all(
            re.fullmatch(r"[a-f0-9]{64}", str(body[field]))
            for field in (
                "rules_sha256",
                "ruleset_main_sha256",
                "anchor_rules_sha256",
            )
        ):
            raise HarnessFailure("privileged helper network rules digest is invalid")
        if body["pf_enabled"] is not True or body["residual_channels"] != [
            "unix-domain-sockets",
            "shared-filesystem",
            "console-user-gui-egress",
            "deferred-scheduling",
        ]:
            raise HarnessFailure("privileged helper network enforcement is incomplete")
        if body["global_protocol_blocks"] != [
            "dns",
            "mdns",
            "icmp",
            "ipv6-icmp",
        ]:
            raise HarnessFailure("privileged helper global network scope is incomplete")
        return {**body, "evidence_digest": digest_json(body)}

    def remove_network(self, vm_name: str, root: PurePosixPath) -> dict[str, Any]:
        output = self.helper(vm_name, "network-remove", (root.name,), 30.0)
        document = self._helper_document(output, "network-remove")
        if document.get("verb") != "network-remove" or document.get("attempt") != root.name:
            raise HarnessFailure("privileged helper cleanup evidence binding mismatch")
        return document

    def verify_network(
        self,
        vm_name: str,
        root: PurePosixPath,
        expected: Mapping[str, Any],
    ) -> dict[str, Any]:
        """Verify that the initially bound PF evidence is still exact post-agent."""

        output = self.helper(vm_name, "network-status", (root.name,), 30.0)
        document = self._helper_document(output, "network-status")
        if document.get("verb") != "network-status" or document.get("attempt") != root.name:
            raise HarnessFailure("privileged helper network status binding mismatch")
        body = {
            "enforcer": "guest-root-pf-anchor",
            "attempt": root.name,
            "vm_name": self._name(vm_name, "VM"),
            "mode": document.get("mode"),
            "allowlist_sha256": (
                document.get("allowlist_sha256") if document.get("mode") == "allowlist" else None
            ),
            "rules_sha256": document.get("rules_sha256"),
            "ruleset_main_sha256": document.get("ruleset_main_sha256"),
            "anchor_rules_sha256": document.get("anchor_rules_sha256"),
            "pf_enabled": document.get("pf_enabled"),
            "residual_channels": document.get("residual_channels"),
            "global_protocol_blocks": document.get("global_protocol_blocks"),
        }
        current = {**body, "evidence_digest": digest_json(body)}
        if dict(expected) != current:
            raise HarnessFailure("protected network enforcement changed during attempt")
        return current

    @staticmethod
    def attempt_id(trial_id: str) -> str:
        suffix = re.sub(r"[^a-z0-9]+", "-", trial_id.casefold()).strip("-")
        if not suffix:
            raise ValidationFailure("trial id cannot produce a guest attempt path")
        return suffix[:59].rstrip("-")

    @classmethod
    def attempt_root(cls, trial_id: str) -> PurePosixPath:
        return PurePosixPath("/Users/Shared/cdb-attempts") / cls.attempt_id(trial_id)

    @classmethod
    def protected_driver_socket(cls, trial_id: str) -> PurePosixPath:
        return (
            PurePosixPath("/private/var/run/cdb-mediator")
            / cls.attempt_id(trial_id)
            / "driver.sock"
        )

    @classmethod
    def attempt_name(cls, trial_id: str) -> str:
        return cls._name(f"cdb-{cls.attempt_id(trial_id)}", "attempt VM")

    def stage_archive(
        self,
        vm_name: str,
        trial_id: str,
        archive: bytes,
        archive_sha256: str,
        *,
        max_bytes: int = 32 * 1024 * 1024,
    ) -> PurePosixPath:
        if not archive or len(archive) > max_bytes:
            raise ValidationFailure("guest staging archive size is out of bounds")
        if not re.fullmatch(r"[a-f0-9]{64}", archive_sha256):
            raise ValidationFailure("guest staging archive digest is invalid")
        vm_name = self._name(vm_name, "VM")
        root = self.attempt_root(trial_id)
        attempt = root.name
        address = self._vm_addresses.get(vm_name)
        if address is None:
            raise HarnessFailure(f"Lume VM {vm_name} has no verified SSH address")
        if digest_file(self.ssh_known_hosts_file) != self.ssh_known_hosts_sha256:
            raise HarnessFailure("Lume known_hosts changed during the attempt")
        descriptor, local_name = tempfile.mkstemp(prefix="cdb-stage-", suffix=".tar.gz")
        local_path = Path(local_name)
        try:
            with os.fdopen(descriptor, "wb") as output:
                output.write(archive)
                output.flush()
                os.fsync(output.fileno())
            os.chmod(local_path, 0o600)
            remote = (
                f"{self.ssh_user}@{address}:"
                f"/Users/lume/Library/Caches/cdb-inbox/cdb-{attempt}-payload.tar.gz"
            )
            result = self.runner(
                (
                    "/usr/bin/scp",
                    "-F",
                    "/dev/null",
                    "-q",
                    "-i",
                    str(self.ssh_identity_file),
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "IdentitiesOnly=yes",
                    "-o",
                    "PasswordAuthentication=no",
                    "-o",
                    "KbdInteractiveAuthentication=no",
                    "-o",
                    "IdentityAgent=none",
                    "-o",
                    "ForwardAgent=no",
                    "-o",
                    "ForwardX11=no",
                    "-o",
                    "PermitLocalCommand=no",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                    f"UserKnownHostsFile={self.ssh_known_hosts_file}",
                    "-o",
                    "GlobalKnownHostsFile=/dev/null",
                    "-o",
                    "UpdateHostKeys=no",
                    "-o",
                    f"HostKeyAlias={self.ssh_host_key_alias}",
                    "-o",
                    "LogLevel=ERROR",
                    "-o",
                    "ConnectTimeout=60",
                    str(local_path),
                    remote,
                ),
                120.0,
            )
            if result.returncode:
                detail = (result.stderr.strip() or result.stdout.strip())[-1000:]
                raise HarnessFailure(f"guest payload transfer failed: {detail}")
            staged = self._helper_document(
                self.helper(
                    vm_name,
                    "stage-attempt",
                    (attempt, archive_sha256),
                    120.0,
                ),
                "stage-attempt",
            )
            if (
                staged.get("verb") != "stage-attempt"
                or staged.get("attempt") != attempt
                or staged.get("archive_sha256") != archive_sha256
            ):
                raise HarnessFailure("privileged helper staging evidence binding mismatch")
        finally:
            local_path.unlink(missing_ok=True)
        return root

    def run_reset(self, vm_name: str, root: PurePosixPath) -> dict[str, Any]:
        output = self.helper(vm_name, "reset-attempt", (root.name,), 120.0)
        try:
            line = next(
                candidate
                for candidate in reversed(output.splitlines())
                if candidate.lstrip().startswith("{")
            )
            document = json.loads(line)
        except (StopIteration, json.JSONDecodeError) as error:
            raise HarnessFailure("guest reset verification returned invalid JSON") from error
        if (
            document.get("verb") != "reset-attempt"
            or document.get("attempt") != root.name
            or not isinstance(document.get("verification"), dict)
        ):
            raise HarnessFailure("guest reset evidence binding mismatch")
        facts = document["verification"]
        if facts.get("ok") is not True:
            raise HarnessFailure("guest reset verification failed")
        return facts

    def run_agent(
        self,
        vm_name: str,
        launch: RenderedGuestLaunch,
        timeout_seconds: float,
        credential_environment: Mapping[str, str] | None = None,
    ) -> CommandResult:
        attempt = self._agent_attempt(launch)
        launch_verb = (
            "launch-agent-with-lease" if credential_environment is not None else "launch-agent"
        )
        if credential_environment is not None:
            credential_input = canonical_json(dict(sorted(credential_environment.items())))
            launch_result = self.helper_result_with_input(
                vm_name,
                launch_verb,
                (attempt,),
                credential_input,
                15.0,
            )
            if launch_result.returncode:
                stage = credential_launch_stage(launch_result.stderr)
                stage_detail = f" at {stage}" if stage is not None else ""
                raise HarnessFailure(
                    f"privileged credential-bearing agent launch failed with "
                    f"{launch_result.returncode}{stage_detail}"
                )
            launch_document = self._helper_document(launch_result.stdout, launch_verb)
        else:
            launch_document = self._helper_document(
                self.helper(vm_name, launch_verb, (attempt,), 15.0),
                launch_verb,
            )
        if (
            launch_document.get("verb") != "launch-agent"
            or launch_document.get("attempt") != attempt
            or launch_document.get("completed") is not False
        ):
            raise HarnessFailure("privileged helper did not launch the agent")
        expected_launch_digest = self._agent_launch_digest(launch)
        if launch_document.get("launch_envelope_sha256") != expected_launch_digest:
            raise HarnessFailure("privileged helper launch envelope digest mismatch")
        wait_seconds = max(0, min(86400, int(timeout_seconds)))
        wait_result = self.helper_result(
            vm_name,
            "wait-agent",
            (attempt, str(wait_seconds)),
            timeout_seconds + 10.0,
        )
        if wait_result.returncode == 124:
            self.kill_agent(vm_name, attempt)
            raise DeadlineExceeded(f"agent exceeded {timeout_seconds:g} seconds")
        if wait_result.returncode:
            detail = (wait_result.stderr.strip() or wait_result.stdout.strip())[-1000:]
            raise HarnessFailure(
                f"privileged helper wait-agent failed with {wait_result.returncode}: {detail}"
            )
        result = self._helper_document(wait_result.stdout, "wait-agent")
        if result.get("completed") is not True:
            self.kill_agent(vm_name, attempt)
            raise DeadlineExceeded(f"agent exceeded {timeout_seconds:g} seconds")
        return self._agent_output_result(
            result,
            attempt=attempt,
            expected_launch_digest=expected_launch_digest,
            allow_missing_exit=False,
        )

    def agent_output(
        self,
        vm_name: str,
        launch: RenderedGuestLaunch,
    ) -> CommandResult:
        """Read bounded output after the agent has reached a terminal state."""

        attempt = self._agent_attempt(launch)
        wait_result = self.helper_result(
            vm_name,
            "wait-agent",
            (attempt, "0"),
            15.0,
        )
        if wait_result.returncode:
            raise HarnessFailure(
                f"privileged helper output recovery failed with {wait_result.returncode}"
            )
        return self._agent_output_result(
            self._helper_document(wait_result.stdout, "wait-agent"),
            attempt=attempt,
            expected_launch_digest=self._agent_launch_digest(launch),
            allow_missing_exit=True,
        )

    @staticmethod
    def _agent_attempt(launch: RenderedGuestLaunch) -> str:
        cwd = PurePosixPath(launch.cwd)
        try:
            return cwd.relative_to(PurePosixPath("/Users/Shared/cdb-attempts")).parts[0]
        except (ValueError, IndexError) as error:
            raise HarnessFailure("rendered launch escaped the attempt root") from error

    @staticmethod
    def _agent_launch_digest(launch: RenderedGuestLaunch) -> str:
        return hashlib.sha256(canonical_json(launch.document()) + b"\n").hexdigest()

    @staticmethod
    def _agent_output_result(
        result: Mapping[str, Any],
        *,
        attempt: str,
        expected_launch_digest: str,
        allow_missing_exit: bool,
    ) -> CommandResult:
        if result.get("launch_envelope_sha256") != expected_launch_digest:
            raise HarnessFailure("privileged helper wait evidence launch binding mismatch")
        if isinstance(result.get("supervisor_error"), str):
            raise HarnessFailure(
                f"privileged agent supervisor failed: {result['supervisor_error']}"
            )
        if result.get("completed") is not True:
            raise HarnessFailure("privileged helper output is not terminal")
        if result.get("verb") != "wait-agent" or result.get("attempt") != attempt:
            raise HarnessFailure("privileged helper wait evidence binding mismatch")
        try:
            stdout_raw = base64.b64decode(result["stdout_b64"], validate=True)
            stderr_raw = base64.b64decode(result["stderr_b64"], validate=True)
            returncode = result["exit_code"]
        except (KeyError, TypeError, ValueError) as error:
            raise HarnessFailure("privileged helper returned malformed agent output") from error
        if not isinstance(returncode, int) and not (
            allow_missing_exit and returncode is None and result.get("killed") is True
        ):
            raise HarnessFailure("privileged helper returned malformed agent status")
        for name, content in (("stdout", stdout_raw), ("stderr", stderr_raw)):
            length = result.get(f"{name}_bytes")
            digest = result.get(f"{name}_sha256")
            truncated = result.get(f"{name}_truncated")
            if (
                not isinstance(length, int)
                or length < len(content)
                or not re.fullmatch(r"[a-f0-9]{64}", str(digest))
                or not isinstance(truncated, bool)
                or truncated != (length > 65536)
            ):
                raise HarnessFailure("privileged helper returned malformed output evidence")
            if not truncated and hashlib.sha256(content).hexdigest() != digest:
                raise HarnessFailure("privileged helper output digest mismatch")
        stdout = stdout_raw.decode("utf-8", errors="replace")
        stderr = stderr_raw.decode("utf-8", errors="replace")
        return CommandResult(
            returncode,
            stdout,
            stderr,
            stdout_bytes=result["stdout_bytes"],
            stdout_sha256=result["stdout_sha256"],
            stdout_truncated=result["stdout_truncated"],
            stderr_bytes=result["stderr_bytes"],
            stderr_sha256=result["stderr_sha256"],
            stderr_truncated=result["stderr_truncated"],
        )

    def kill_agent(self, vm_name: str, attempt: str) -> dict[str, Any]:
        attempt = self._name(attempt, "attempt")
        document = self._helper_document(
            self.helper(vm_name, "kill-agent", (attempt,), 15.0),
            "kill-agent",
        )
        if (
            document.get("verb") != "kill-agent"
            or document.get("attempt") != attempt
            or document.get("completed") is not True
            or document.get("no_agent_processes") is not True
        ):
            raise HarnessFailure("privileged helper agent termination was incomplete")
        return document

    def stop(self, vm_name: str, timeout_seconds: float = 60.0) -> None:
        vm_name = self._name(vm_name, "VM")
        status = self.inspect(vm_name).get("status")
        if status != "stopped":
            result = self.runner([str(self.binary), "stop", vm_name], timeout_seconds)
            if result.returncode == -signal.SIGINT:
                # Lume 0.5.3 can select its own config-file handle and send
                # SIGINT to the stop command. Recover through the authenticated
                # guest only after rebinding the current VM address; the fresh
                # stopped observation below remains the certification barrier.
                record = self.inspect(vm_name)
                status = record.get("status")
                if status == "running":
                    address = record.get("ipAddress")
                    try:
                        parsed_address = ipaddress.ip_address(str(address))
                    except ValueError as error:
                        raise HarnessFailure(
                            "interrupted Lume stop has no current VM address"
                        ) from error
                    if (
                        record.get("sshAvailable") is not True
                        or parsed_address.version != 4
                        or str(parsed_address) != address
                    ):
                        raise HarnessFailure("interrupted Lume stop has no current SSH identity")
                    self._vm_addresses[vm_name] = address
                    self.helper_result(
                        vm_name,
                        "shutdown",
                        timeout_seconds=min(timeout_seconds, 30.0),
                    )
                elif status != "stopped":
                    raise HarnessFailure("interrupted Lume stop left the VM in an unsafe state")
            elif result.returncode != 0:
                detail = (result.stderr.strip() or result.stdout.strip())[-1000:]
                raise HarnessFailure(f"lume stop failed with {result.returncode}: {detail}")
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            if self.inspect(vm_name).get("status") == "stopped":
                return
            time.sleep(self.poll_interval_seconds)
        raise HarnessFailure(f"Lume VM {vm_name} did not stop before collection")

    def collect_workspace(
        self,
        vm_name: str,
        guest_workspace: PurePosixPath,
        destination: Path,
        bounds: CollectionBounds,
    ) -> CollectionReport:
        return self.collect_guest_tree(vm_name, guest_workspace, destination, bounds)

    def collect_guest_tree(
        self,
        vm_name: str,
        guest_root: PurePosixPath,
        destination: Path,
        bounds: CollectionBounds,
    ) -> CollectionReport:
        vm_name = self._name(vm_name, "VM")
        if self.inspect(vm_name).get("status") != "stopped":
            raise HarnessFailure("Lume VM must be stopped before collection")
        vm_root = (self.storage_root / vm_name).resolve()
        if not vm_root.is_relative_to(self.storage_root):
            raise HarnessFailure("Lume VM storage path escapes storage root")
        disk = vm_root / "disk.img"
        if not disk.is_file():
            raise HarnessFailure("Lume VM disk image is missing")
        try:
            completed = subprocess.run(
                [
                    "/usr/bin/hdiutil",
                    "attach",
                    "-readonly",
                    "-nobrowse",
                    "-plist",
                    str(disk),
                ],
                capture_output=True,
                check=False,
                timeout=60.0,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise HarnessFailure("read-only Lume disk attachment failed") from error
        if completed.returncode != 0:
            raise HarnessFailure("read-only Lume disk attachment failed")
        devices: list[str] = []
        try:
            document = plistlib.loads(completed.stdout)
            entities = document.get("system-entities", [])
            devices = sorted(
                {
                    item.get("dev-entry")
                    for item in entities
                    if isinstance(item, dict) and item.get("dev-entry")
                },
                key=len,
            )
            candidates = [
                Path(item["mount-point"]).resolve(strict=True)
                for item in entities
                if isinstance(item, dict) and item.get("mount-point")
            ]
            relative = Path(*guest_root.parts[1:])
            source = next(
                (mount / relative for mount in candidates if (mount / relative).is_dir()),
                None,
            )
            if source is None:
                raise HarnessFailure("guest workspace is absent from attached data volume")
            flags = os.statvfs(source).f_flag
            if hasattr(os, "ST_RDONLY") and not flags & os.ST_RDONLY:
                raise HarnessFailure("guest data volume is not mounted read-only")
            mount = next(candidate for candidate in candidates if source.is_relative_to(candidate))
            return collect_tree(
                source,
                destination,
                bounds,
                trusted_root=mount,
            )
        except (
            OSError,
            plistlib.InvalidFileException,
            KeyError,
            StopIteration,
            TypeError,
        ) as error:
            raise HarnessFailure("hdiutil returned invalid attachment metadata") from error
        finally:
            if not devices:
                devices = self._attached_devices_for_disk(disk)
            if not devices:
                raise HarnessFailure("read-only Lume disk device could not be identified")
            self._detach_device(devices[0])

    @staticmethod
    def _attached_devices_for_disk(disk: Path) -> list[str]:
        try:
            completed = subprocess.run(
                ["/usr/bin/hdiutil", "info", "-plist"],
                capture_output=True,
                check=False,
                timeout=30.0,
            )
            if completed.returncode != 0:
                return []
            document = plistlib.loads(completed.stdout)
        except (OSError, subprocess.TimeoutExpired, plistlib.InvalidFileException):
            return []
        for image in document.get("images", []):
            if not isinstance(image, dict) or image.get("image-path") != str(disk):
                continue
            return sorted(
                {
                    entity.get("dev-entry")
                    for entity in image.get("system-entities", [])
                    if isinstance(entity, dict) and entity.get("dev-entry")
                },
                key=len,
            )
        return []

    @staticmethod
    def _detach_device(device: str) -> None:
        for arguments in (
            ["/usr/bin/hdiutil", "detach", device],
            ["/usr/bin/hdiutil", "detach", "-force", device],
        ):
            try:
                completed = subprocess.run(
                    arguments,
                    capture_output=True,
                    check=False,
                    timeout=60.0,
                )
            except (OSError, subprocess.TimeoutExpired):
                continue
            if completed.returncode == 0:
                return
        raise HarnessFailure("read-only Lume disk detachment failed")

    def start_driver(self, vm_name: str) -> None:
        """Launch the signed app in the logged-in Aqua domain so TCC attribution is exact."""

        started = self._helper_document(
            self.helper(vm_name, "start-driver", timeout_seconds=60.0),
            "start-driver",
        )
        if started.get("verb") != "start-driver" or started.get("console_uid") != self.console_uid:
            raise HarnessFailure("privileged helper driver launch evidence mismatch")
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline:
            try:
                status = json.loads(
                    self.ssh(
                        vm_name,
                        f"{shlex.quote(str(self.driver_path))} permissions status --json",
                        15.0,
                    )
                )
            except (HarnessFailure, json.JSONDecodeError):
                status = {}
            if (
                status.get("accessibility") is True
                and status.get("screen_recording") is True
                and status.get("source", {}).get("attribution") == "driver-daemon"
            ):
                return
            time.sleep(self.poll_interval_seconds)
        raise HarnessFailure("guest Cua Driver did not start with its app-owned TCC identity")

    def share_console_path(self, vm_name: str, root: PurePosixPath, relative: str) -> None:
        document = self._helper_document(
            self.helper(
                vm_name,
                "share-console-path",
                (root.name, relative),
                timeout_seconds=30.0,
            ),
            "share-console-path",
        )
        if document != {
            "verb": "share-console-path",
            "attempt": root.name,
            "relative": relative,
            "mode": "shared-agent-console",
        }:
            raise HarnessFailure("privileged helper console-share evidence mismatch")

    def prepare_task_store(
        self, vm_name: str, root: PurePosixPath, relative: str
    ) -> dict[str, Any]:
        document = self._helper_document(
            self.helper(
                vm_name,
                "prepare-task-store",
                (root.name, relative),
                timeout_seconds=30.0,
            ),
            "prepare-task-store",
        )
        if (
            document.get("verb") != "prepare-task-store"
            or document.get("attempt") != root.name
            or document.get("relative") != relative
            or document.get("mode") != "protected-console-only"
            or not isinstance(document.get("files"), int)
            or not isinstance(document.get("bytes"), int)
        ):
            raise HarnessFailure("privileged helper task-store evidence mismatch")
        return document

    def start_mediator(
        self,
        vm_name: str,
        root: PurePosixPath,
        task_id: str,
        expected_tool_contract_sha256: str | None = None,
        expected_daemon_tool_list_envelope_sha256: str | None = None,
    ) -> dict[str, Any]:
        if not re.fullmatch(r"[a-z0-9]+(?:[._-][a-z0-9]+)+", task_id):
            raise HarnessFailure("protected mediator task id is invalid")
        if expected_tool_contract_sha256 is not None and not re.fullmatch(
            r"[a-f0-9]{64}", expected_tool_contract_sha256
        ):
            raise HarnessFailure("expected tool contract digest is invalid")
        if expected_daemon_tool_list_envelope_sha256 is not None and not re.fullmatch(
            r"[a-f0-9]{64}", expected_daemon_tool_list_envelope_sha256
        ):
            raise HarnessFailure("expected daemon tool-list envelope digest is invalid")
        if (
            expected_daemon_tool_list_envelope_sha256 is not None
            and expected_tool_contract_sha256 is None
        ):
            raise HarnessFailure("daemon tool-list envelope binding requires tool contract binding")
        helper_arguments = (root.name, task_id)
        if expected_tool_contract_sha256 is not None:
            helper_arguments = (root.name, task_id, expected_tool_contract_sha256)
        if expected_daemon_tool_list_envelope_sha256 is not None:
            helper_arguments = (
                root.name,
                task_id,
                expected_tool_contract_sha256,
                expected_daemon_tool_list_envelope_sha256,
            )
        document = self._helper_document(
            self.helper(
                vm_name,
                "mediator-start",
                helper_arguments,
                timeout_seconds=30.0,
            ),
            "mediator-start",
        )
        if (
            document.get("verb") != "mediator-start"
            or document.get("attempt") != root.name
            or document.get("task_id") != task_id
            or document.get("expected_tool_contract_sha256") != expected_tool_contract_sha256
            or document.get("tool_contract_required")
            is not (expected_tool_contract_sha256 is not None)
            or document.get("expected_daemon_tool_list_envelope_sha256")
            != expected_daemon_tool_list_envelope_sha256
            or document.get("daemon_tool_list_envelope_required")
            is not (expected_daemon_tool_list_envelope_sha256 is not None)
            or document.get("worker_uid") != self.console_uid
            or document.get("frontend_mode") != "0660"
            or document.get("frontend_owner_uid") != self.console_uid
            or not isinstance(document.get("agent_uid"), int)
            or document.get("agent_uid") == self.console_uid
            or not re.fullmatch(r"[a-f0-9]{64}", str(document.get("mediator_sha256")))
            or document.get("backend_owner_uid") != self.console_uid
            or any(
                not isinstance(document.get(field), int)
                or isinstance(document.get(field), bool)
                or document.get(field) <= 0
                for field in (
                    "backend_parent_device",
                    "backend_parent_inode",
                    "backend_device",
                    "backend_inode",
                    "target_pid",
                )
            )
        ):
            raise HarnessFailure("protected mediator start evidence mismatch")
        return document

    def stop_and_seal_mediator(self, vm_name: str, root: PurePosixPath) -> dict[str, Any]:
        document = self._helper_document(
            self.helper(
                vm_name,
                "mediator-stop-and-seal",
                (root.name,),
                timeout_seconds=30.0,
            ),
            "mediator-stop-and-seal",
        )
        if (
            document.get("verb") != "mediator-stop-and-seal"
            or document.get("attempt") != root.name
            or document.get("worker_uid") != self.console_uid
            or not isinstance(document.get("events"), list)
            or not isinstance(document.get("records"), int)
            or document.get("records", 0) < 1
            or not isinstance(document.get("transport_integrity"), bool)
            or not isinstance(document.get("evidence_complete"), bool)
            or not re.fullmatch(r"[a-f0-9]{64}", str(document.get("event_log_sha256")))
            or not re.fullmatch(r"[a-f0-9]{64}", str(document.get("event_log_tail")))
            or not re.fullmatch(r"[a-f0-9]{64}", str(document.get("report_digest")))
            or not isinstance(document.get("off_target_activity"), bool)
            or not isinstance(document.get("tool_contract_validated"), bool)
            or not isinstance(document.get("tool_contract_required"), bool)
            or not isinstance(document.get("daemon_tool_list_envelope_validated"), bool)
            or not isinstance(document.get("daemon_tool_list_envelope_required"), bool)
            or (
                document.get("tool_contract_required")
                and not re.fullmatch(
                    r"[a-f0-9]{64}",
                    str(document.get("expected_tool_contract_sha256")),
                )
            )
            or (
                document.get("daemon_tool_list_envelope_required")
                and not re.fullmatch(
                    r"[a-f0-9]{64}",
                    str(document.get("expected_daemon_tool_list_envelope_sha256")),
                )
            )
            or (
                document.get("observed_daemon_tool_list_envelope_sha256") is not None
                and not re.fullmatch(
                    r"[a-f0-9]{64}",
                    str(document.get("observed_daemon_tool_list_envelope_sha256")),
                )
            )
            or (
                document.get("observed_tool_contract_sha256") is not None
                and not re.fullmatch(
                    r"[a-f0-9]{64}",
                    str(document.get("observed_tool_contract_sha256")),
                )
            )
            or (
                document.get("tool_contract_tool_count") is not None
                and (
                    not isinstance(document.get("tool_contract_tool_count"), int)
                    or isinstance(document.get("tool_contract_tool_count"), bool)
                    or document.get("tool_contract_tool_count") < 1
                )
            )
            or any(
                not isinstance(document.get(field), int)
                or isinstance(document.get(field), bool)
                or document.get(field) <= 0
                for field in (
                    "backend_parent_device",
                    "backend_parent_inode",
                    "backend_device",
                    "backend_inode",
                    "target_pid",
                )
            )
        ):
            raise HarnessFailure("protected mediator seal evidence mismatch")
        return document

    def launch_task_app(
        self,
        vm_name: str,
        root: PurePosixPath,
        app_id: str,
        store_relative: str,
        launch_arguments: Sequence[str],
        store_mode: str = "protected-console-only",
    ) -> dict[str, Any]:
        if store_mode not in {"protected-console-only", "shared-agent-console"}:
            raise ValidationFailure("unsupported task app store mode")
        if (
            not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", app_id)
            or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", store_relative)
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
            raise ValidationFailure("invalid protected task app configuration")
        launch_arguments_json = json.dumps(
            list(launch_arguments), sort_keys=True, separators=(",", ":")
        )
        document = self._helper_document(
            self.helper(
                vm_name,
                "launch-task-app",
                (
                    root.name,
                    app_id,
                    store_relative,
                    launch_arguments_json,
                    store_mode,
                ),
                timeout_seconds=60.0,
            ),
            "launch-task-app",
        )
        if (
            document.get("verb") != "launch-task-app"
            or document.get("attempt") != root.name
            or document.get("app_id") != app_id
            or document.get("store_relative") != store_relative
            or document.get("store_mode") != store_mode
            or document.get("launch_arguments_sha256")
            != hashlib.sha256(launch_arguments_json.encode()).hexdigest()
            or not re.fullmatch(r"[a-f0-9]{64}", str(document.get("store_path_sha256")))
            or not re.fullmatch(r"[a-f0-9]{64}", str(document.get("target_process_identity")))
            or any(
                not isinstance(document.get(field), int)
                or isinstance(document.get(field), bool)
                or document.get(field) <= 0
                for field in ("store_device", "store_inode", "target_pid")
            )
        ):
            raise HarnessFailure("privileged helper task-app launch evidence mismatch")
        return document

    def verify_privileged_helper(self, vm_name: str) -> dict[str, Any]:
        path = shlex.quote(str(self.privileged_helper_path))
        mediator_path = "/usr/local/libexec/cdb-driver-mediator"
        output = self.ssh(
            vm_name,
            f"/usr/bin/stat -f '%Su %Sg %OLp' {path} {mediator_path} && "
            f"/usr/bin/shasum -a 256 {path} {mediator_path}",
            30.0,
        )
        lines = [line.strip() for line in output.splitlines() if line.strip()]
        if len(lines) != 4:
            raise HarnessFailure("guest privileged helper verification was malformed")
        owner = lines[0].split()
        mediator_owner = lines[1].split()
        digest = lines[2].split()[0] if lines[2].split() else ""
        mediator_digest = lines[3].split()[0] if lines[3].split() else ""
        if owner != ["root", "wheel", "555"] or mediator_owner != ["root", "wheel", "555"]:
            raise HarnessFailure("guest privileged helper ownership or mode mismatch")
        if f"sha256:{digest}" != self.privileged_helper_sha256:
            raise HarnessFailure("guest privileged helper digest mismatch")
        if f"sha256:{mediator_digest}" != self.mediator_sha256:
            raise HarnessFailure("guest protected mediator digest mismatch")
        installation = self._helper_document(
            self.helper(vm_name, "installation-facts", (), 30.0),
            "installation-facts",
        )
        pf_main_rules_sha256 = installation.get("pf_main_rules_sha256")
        if not isinstance(pf_main_rules_sha256, str) or not re.fullmatch(
            r"[a-f0-9]{64}", pf_main_rules_sha256
        ):
            raise HarnessFailure("guest main PF ruleset digest is malformed")
        if f"sha256:{pf_main_rules_sha256}" != self.pf_main_rules_sha256:
            raise HarnessFailure("guest main PF ruleset digest mismatch")
        expected = {
            "verb": "installation-facts",
            "helper_sha256": self.privileged_helper_sha256.removeprefix("sha256:"),
            "helper_mode": "0555",
            "mediator_sha256": self.mediator_sha256.removeprefix("sha256:"),
            "mediator_mode": "0555",
            "sudoers_sha256": self.privileged_sudoers_sha256.removeprefix("sha256:"),
            "sudoers_mode": "0440",
            "pf_main_rules_sha256": self.pf_main_rules_sha256.removeprefix("sha256:"),
            "agent_passwordless_sudo_denied": True,
            "agent_admin_group_member": False,
            "agent_sudo_policy_denied": True,
            "ssh_password_authentication": False,
            "ssh_keyboard_interactive_authentication": False,
        }
        if any(installation.get(key) != value for key, value in expected.items()):
            raise HarnessFailure("guest protected helper installation evidence mismatch")
        body = {
            "enforcer": "guest-root-cdb-helper",
            "vm_name": self._name(vm_name, "VM"),
            "helper_sha256": self.privileged_helper_sha256,
            "mediator_sha256": self.mediator_sha256,
            "sudoers_sha256": self.privileged_sudoers_sha256,
            "pf_main_rules_sha256": self.pf_main_rules_sha256,
            "owner": "root:wheel",
            "mode": "0555",
            "sudoers_mode": "0440",
            "agent_passwordless_sudo_denied": True,
            "agent_admin_group_member": False,
            "agent_sudo_policy_denied": True,
            "ssh_password_authentication": False,
            "ssh_keyboard_interactive_authentication": False,
        }
        return {**body, "evidence_digest": digest_json(body)}

    def host_human_input_evidence(self, vm_name: str, ready: Mapping[str, Any]) -> dict[str, Any]:
        if ready.get("vncUrl") is not None:
            raise HarnessFailure("Lume exposed a VNC URL during a protected attempt")
        config = (self.storage_root / vm_name / "config.json").resolve()
        vm_root = (self.storage_root / vm_name).resolve()
        if not config.is_relative_to(vm_root):
            raise HarnessFailure("Lume config path escaped the attempt VM")
        holder = self.runner(("/usr/sbin/lsof", "-n", "-P", "-w", "-t", str(config)), 10.0)
        try:
            pids = {int(line) for line in holder.stdout.splitlines() if line.strip()}
        except ValueError as error:
            raise HarnessFailure("Lume process-owner probe was malformed") from error
        if holder.returncode != 0 or len(pids) != 1:
            raise HarnessFailure("could not bind the protected attempt to one Lume process")
        pid = next(iter(pids))
        listeners = self.runner(
            (
                "/usr/sbin/lsof",
                "-n",
                "-P",
                "-a",
                "-p",
                str(pid),
                "-iTCP",
                "-sTCP:LISTEN",
            ),
            10.0,
        )
        if listeners.returncode not in {0, 1}:
            raise HarnessFailure("Lume listener probe failed")
        if listeners.stdout.strip():
            raise HarnessFailure("Lume process owns a human-input TCP listener")
        body = {
            "enforcer": "host-lume-vnc-disabled",
            "vm_name": vm_name,
            "vnc_url": None,
            "listening_tcp_sockets": 0,
        }
        return {**body, "evidence_digest": digest_json(body)}

    def guest_facts(self, vm_name: str) -> dict[str, Any]:
        """Read stable facts only; omit IPs, clocks, process IDs, and VNC secrets."""

        script = (
            r"""
import base64, glob, grp, hashlib, json, os, platform, plistlib, pwd, stat, subprocess

def run(*argv):
    return subprocess.run(argv, capture_output=True, text=True, check=True).stdout.strip()

apps = []
for path in sorted(glob.glob('/Applications/*.app')):
    try:
        with open(path + '/Contents/Info.plist', 'rb') as source:
            info = plistlib.load(source)
        bundle = info.get('CFBundleIdentifier')
        version = info.get('CFBundleShortVersionString') or info.get('CFBundleVersion')
        if bundle and version:
            apps.append({'id': str(bundle), 'version': str(version)})
    except (OSError, plistlib.InvalidFileException):
        pass

driver = __DRIVER_PATH__
driver_realpath = os.path.realpath(driver)
permissions = json.loads(run(driver, 'permissions', 'status', '--json'))
display = json.loads(run('/usr/sbin/system_profiler', 'SPDisplaysDataType', '-json'))
capture = json.loads(run(driver, 'call', 'get_desktop_state', '{}'))
display_records = display.get('SPDisplaysDataType', [])
retina = any(
    item.get('spdisplays_retina') == 'spdisplays_yes'
    for record in display_records
    for item in record.get('spdisplays_ndrvs', [])
)
subprocess.run(
    ['/usr/bin/codesign', '--verify', '--strict', '--deep', '/Applications/CuaDriver.app'],
    capture_output=True, text=True, check=True,
)
with open(driver_realpath, 'rb') as source:
    driver_hash = hashlib.sha256()
    while True:
        chunk = source.read(1024 * 1024)
        if not chunk:
            break
        driver_hash.update(chunk)
    driver_digest = 'sha256:' + driver_hash.hexdigest()
signature = subprocess.run(
    ['/usr/bin/codesign', '-dv', '--verbose=2', '/Applications/CuaDriver.app'],
    capture_output=True, text=True, check=True,
).stderr
team_id = next(
    (line.split('=', 1)[1] for line in signature.splitlines() if line.startswith('TeamIdentifier=')),
    None,
)
agent = pwd.getpwnam('cdb-agent')
admin = grp.getgrnam('admin')
agent_groups = os.getgrouplist(agent.pw_name, agent.pw_gid)
agent_is_admin = admin.gr_gid in agent_groups
socket_path = '/Users/lume/Library/Caches/cua-driver/cua-driver.sock'
socket_status = os.stat(socket_path)
socket_mode = stat.S_IMODE(socket_status.st_mode)
agent_can_access_socket = (
    (agent.pw_uid == socket_status.st_uid and bool(socket_mode & 0o600))
    or (socket_status.st_gid in agent_groups and bool(socket_mode & 0o060))
    or bool(socket_mode & 0o006)
)
production_tools = {}
production_expected = __PRODUCTION_TOOLS__
for name, expected_digest in sorted(production_expected.items()):
    path = '/usr/local/bin/' + name
    expected_mode = 0o755 if name == 'codex-code-mode-host' else 0o555
    status = os.stat(path, follow_symlinks=False)
    if (
        not stat.S_ISREG(status.st_mode)
        or status.st_uid != 0
        or status.st_gid != 0
        or stat.S_IMODE(status.st_mode) != expected_mode
    ):
        raise RuntimeError('production tool installation is unsafe: ' + name)
    digest = hashlib.sha256()
    with open(path, 'rb') as source:
        while True:
            chunk = source.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    actual_digest = 'sha256:' + digest.hexdigest()
    if actual_digest != expected_digest:
        raise RuntimeError('production tool digest mismatch: ' + name)
    production_tools[name] = {
        'path': path,
        'sha256': actual_digest,
        'owner': 'root:wheel',
        'mode': format(expected_mode, '04o'),
    }
with open('/etc/hosts', 'rb') as source:
    network_hosts_sha256 = 'sha256:' + hashlib.sha256(source.read()).hexdigest()
print(json.dumps({
    'os_version': run('/usr/bin/sw_vers', '-productVersion'),
    'os_build': run('/usr/bin/sw_vers', '-buildVersion'),
    'architecture': platform.machine(),
    'driver_version': run(driver, '--version'),
    'driver_realpath': driver_realpath,
    'driver_sha256': driver_digest,
    'driver_team_id': team_id,
    'console_user': run('/usr/bin/stat', '-f', '%Su', '/dev/console'),
    'agent_isolation': {
        'user': agent.pw_name,
        'is_admin': agent_is_admin,
        'driver_socket_owner_uid': socket_status.st_uid,
        'driver_socket_mode': format(socket_mode, '04o'),
        'agent_can_access_driver_socket': agent_can_access_socket,
    },
    'permissions': {
        'accessibility': permissions.get('accessibility'),
        'screen_recording': permissions.get('screen_recording'),
        'screen_recording_capturable': permissions.get('screen_recording_capturable'),
        'direct_capture_status': permissions.get('direct_capture_status'),
    },
    'applications': apps,
    'production_tools': production_tools,
    'network_hosts_sha256': network_hosts_sha256,
    'display': display,
    'display_summary': {
        'width_px': capture.get('screenshot_width'),
        'height_px': capture.get('screenshot_height'),
        'scale': 2 if retina else 1,
    },
    'live_capture': {
        'mime_type': capture.get('screenshot_mime_type'),
        'width': capture.get('screenshot_width'),
        'height': capture.get('screenshot_height'),
        'png_decodes': bool(base64.b64decode(capture.get('screenshot_png_b64', ''), validate=True)),
    },
}, sort_keys=True, separators=(',', ':')))
""".replace("__DRIVER_PATH__", repr(str(self.driver_path)))
            .replace("__PRODUCTION_TOOLS__", repr(self.production_tools))
            .strip()
        )
        import base64

        encoded = base64.b64encode(script.encode()).decode("ascii")
        output = self.ssh(
            vm_name,
            f"/usr/bin/python3 -c \"import base64;exec(base64.b64decode('{encoded}'))\"",
            120.0,
        )
        try:
            line = next(
                candidate
                for candidate in reversed(output.splitlines())
                if candidate.lstrip().startswith("{")
            )
            facts = json.loads(line)
        except (StopIteration, json.JSONDecodeError) as error:
            raise HarnessFailure("guest fact probe returned invalid JSON") from error
        if not isinstance(facts, dict):
            raise HarnessFailure("guest fact probe did not return an object")
        permissions = facts.get("permissions", {})
        if permissions.get("accessibility") is not True:
            raise HarnessFailure("guest Cua Driver lacks Accessibility permission")
        if permissions.get("screen_recording") is not True:
            raise HarnessFailure("guest Cua Driver lacks Screen Recording permission")
        capture = facts.get("live_capture", {})
        if not (
            capture.get("mime_type") == "image/png"
            and isinstance(capture.get("width"), int)
            and capture["width"] > 0
            and isinstance(capture.get("height"), int)
            and capture["height"] > 0
            and capture.get("png_decodes") is True
        ):
            raise HarnessFailure("guest Cua Driver cannot perform a live desktop capture")
        if facts.get("driver_team_id") != self.driver_team_id:
            raise HarnessFailure("guest Cua Driver signing identity mismatch")
        if facts.get("driver_realpath") != "/Applications/CuaDriver.app/Contents/MacOS/cua-driver":
            raise HarnessFailure("guest Cua Driver CLI does not resolve to the signed app")
        if facts.get("driver_version") != self.driver_version:
            raise HarnessFailure("guest Cua Driver version mismatch")
        if facts.get("driver_sha256") != self.driver_sha256:
            raise HarnessFailure("guest Cua Driver binary digest mismatch")
        if not re.fullmatch(r"sha256:[a-f0-9]{64}", str(facts.get("network_hosts_sha256"))):
            raise HarnessFailure("guest network host mapping digest is unavailable")
        isolation = facts.get("agent_isolation", {})
        if isolation.get("user") != "cdb-agent" or isolation.get("is_admin") is not False:
            raise HarnessFailure("guest benchmark agent identity is not a standard user")
        if isolation.get("agent_can_access_driver_socket") is not False:
            raise HarnessFailure("guest benchmark agent can access the protected driver socket")
        return facts

    def provision(self, trial_id: str, timeout_seconds: float = 300.0) -> LumeAttempt:
        vm_name = self.attempt_name(trial_id)
        self.verify_seed_image()
        seed = self.inspect(self.seed_vm)
        if seed.get("status") != "stopped":
            raise HarnessFailure("Lume seed VM must be stopped before cloning")
        self._run(("clone", self.seed_vm, vm_name), timeout_seconds)
        try:
            self._run(
                (
                    "run",
                    vm_name,
                    "--display",
                    "none",
                    "--vnc",
                    "disabled",
                    "--detach",
                    "--network",
                    "nat",
                ),
                timeout_seconds,
            )
            ready = self._wait_ready(vm_name, timeout_seconds)
            self._vm_addresses[vm_name] = str(ready["ipAddress"])
            helper_evidence = self.verify_privileged_helper(vm_name)
            human_input_evidence = self.host_human_input_evidence(vm_name, ready)
            self.start_driver(vm_name)
            facts = self.guest_facts(vm_name)
            fingerprint = digest_json(facts)
            if fingerprint != self.expected_pristine_fingerprint:
                raise HarnessFailure(
                    "fresh Lume clone does not match the frozen pristine fingerprint"
                )
            return LumeAttempt(
                vm_name=vm_name,
                ip_address=str(ready["ipAddress"]),
                seed_provenance_digest=self.seed_provenance_digest,
                pristine_fingerprint=fingerprint,
                guest_facts=facts,
                apparatus_evidence={
                    "lume_binary": {
                        "sha256": self.binary_sha256,
                    },
                    "privileged_helper": helper_evidence,
                    "human_input": human_input_evidence,
                },
            )
        except BaseException:
            self.destroy(vm_name)
            raise

    def destroy(self, vm_name: str) -> None:
        vm_name = self._name(vm_name, "VM")
        vm_root = (self.storage_root / vm_name).resolve()
        if not vm_root.is_relative_to(self.storage_root):
            raise HarnessFailure("Lume VM storage path escapes storage root")
        if not vm_root.exists() and not self.exists(vm_name):
            return
        status: str | None = None
        for _ in range(3):
            try:
                status = str(self.inspect(vm_name).get("status"))
                break
            except HarnessFailure:
                time.sleep(max(self.poll_interval_seconds, 0.1))
        if status != "stopped":
            self.stop(vm_name, 60.0)
        self._run(("delete", vm_name, "--force"), 120.0)
        self._vm_addresses.pop(vm_name, None)
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            if not vm_root.exists() and not self.exists(vm_name):
                return
            time.sleep(self.poll_interval_seconds)
        raise HarnessFailure(f"Lume VM {vm_name} still exists after deletion")
