from __future__ import annotations

import base64
import hashlib
import json
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from unittest import mock

from cua_bench_runtime.canon import canonical_json, digest_file, digest_json
from cua_bench_runtime.errors import DeadlineExceeded, HarnessFailure, ValidationFailure
from cua_bench_runtime.guest_launch import RenderedGuestLaunch
from cua_bench_runtime.lume import (
    CommandResult,
    LumeControlPlane,
    subprocess_input_runner,
    subprocess_runner,
)


FACTS = {
    "os_version": "26.5.2",
    "os_build": "25F84",
    "architecture": "arm64",
    "driver_version": "cua-driver 0.19.4",
    "driver_realpath": "/Applications/CuaDriver.app/Contents/MacOS/cua-driver",
    "driver_sha256": "sha256:" + "d" * 64,
    "driver_team_id": "YCK386LBJ7",
    "console_user": "lume",
    "agent_isolation": {
        "user": "cdb-agent",
        "is_admin": False,
        "driver_socket_owner_uid": 501,
        "driver_socket_mode": "0600",
        "agent_can_access_driver_socket": False,
    },
    "permissions": {
        "accessibility": True,
        "screen_recording": True,
        "screen_recording_capturable": True,
        "direct_capture_status": "ready",
    },
    "applications": [{"id": "com.example.App", "version": "1.0"}],
    "network_hosts_sha256": "sha256:" + "b" * 64,
    "display": {"SPDisplaysDataType": []},
    "live_capture": {
        "mime_type": "image/png",
        "width": 1024,
        "height": 768,
        "png_decodes": True,
    },
}
HELPER_DIGEST = "sha256:" + "e" * 64
MEDIATOR_DIGEST = "sha256:" + "d" * 64
SUDOERS_DIGEST = "sha256:" + "f" * 64
PF_MAIN_DIGEST = "sha256:" + "a" * 64


class LumeSubprocessRunnerTests(unittest.TestCase):
    @mock.patch("cua_bench_runtime.lume.subprocess.run")
    def test_runner_starts_a_new_posix_session(self, run: mock.Mock) -> None:
        run.return_value = mock.Mock(returncode=0, stdout="ok", stderr="")
        with mock.patch("cua_bench_runtime.lume.os.name", "posix"):
            result = subprocess_runner(["/bin/true"], 5.0)
        self.assertEqual(result, CommandResult(0, "ok", ""))
        self.assertIs(run.call_args.kwargs["start_new_session"], True)
        self.assertNotIn("creationflags", run.call_args.kwargs)

    @mock.patch("cua_bench_runtime.lume.subprocess.run")
    def test_input_runner_starts_a_new_windows_process_group(self, run: mock.Mock) -> None:
        run.return_value = mock.Mock(returncode=0, stdout=b"ok", stderr=b"")
        with (
            mock.patch("cua_bench_runtime.lume.os.name", "nt"),
            mock.patch.object(subprocess, "CREATE_NEW_PROCESS_GROUP", 0x200, create=True),
        ):
            result = subprocess_input_runner(["lume.exe"], 5.0, b"input")
        self.assertEqual(result, CommandResult(0, "ok", ""))
        self.assertEqual(run.call_args.kwargs["creationflags"], 0x200)
        self.assertNotIn("start_new_session", run.call_args.kwargs)

    @unittest.skipIf(os.name == "nt", "POSIX session semantics")
    def test_runner_process_has_a_distinct_session_from_parent(self) -> None:
        result = subprocess_runner([sys.executable, "-c", "import os; print(os.getsid(0))"], 5.0)
        self.assertEqual(result.returncode, 0)
        self.assertNotEqual(int(result.stdout.strip()), os.getsid(0))


def rendered_launch() -> RenderedGuestLaunch:
    return RenderedGuestLaunch(
        argv=("/bin/true",),
        cwd="/Users/Shared/cdb-attempts/trial-alpha-0/workspace",
        environment={},
    )


class FakeLume:
    def __init__(
        self,
        *,
        seed_status: str = "stopped",
        launch_stays_stopped: bool = False,
    ) -> None:
        self.seed_status = seed_status
        self.launch_stays_stopped = launch_stays_stopped
        self.commands: list[list[str]] = []
        self.attempt_exists = False
        self.attempt_status = "stopped"
        self.fail_inventory = False
        self.pf_main_rules_sha256: object = PF_MAIN_DIGEST.removeprefix("sha256:")
        launch_body = {
            "argv": ["/bin/true"],
            "cwd": "/Users/Shared/cdb-attempts/trial-alpha-0/workspace",
            "environment": {},
        }
        self.launch_digest = hashlib.sha256(canonical_json(launch_body) + b"\n").hexdigest()

    def __call__(self, argv, timeout_seconds) -> CommandResult:
        del timeout_seconds
        command = list(argv)
        self.commands.append(command)
        if command[0] == "/usr/bin/scp":
            return CommandResult(0, "", "")
        if command[0] == "/usr/sbin/lsof":
            if "-t" in command:
                return CommandResult(0, "4242\n", "")
            return CommandResult(1, "", "")
        if command[0] == "/usr/bin/ssh":
            remote = command[-1]
            if "/usr/bin/stat" in remote and "/usr/bin/shasum" in remote:
                return CommandResult(
                    0,
                    f"root wheel 555\nroot wheel 555\n"
                    f"{'e' * 64}  /usr/local/libexec/cdb-helper\n"
                    f"{'d' * 64}  /usr/local/libexec/cdb-driver-mediator\n",
                    "",
                )
            if "stage-attempt" in remote:
                parts = remote.split()
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "stage-attempt",
                            "attempt": parts[-2],
                            "archive_sha256": parts[-1],
                        }
                    )
                    + "\n",
                    "",
                )
            if "start-driver" in remote:
                return CommandResult(0, '{"verb":"start-driver","console_uid":501}\n', "")
            if "share-console-path" in remote:
                return CommandResult(
                    0,
                    '{"attempt":"trial-alpha-0","mode":"shared-agent-console",'
                    '"relative":"task-store","verb":"share-console-path"}\n',
                    "",
                )
            if "prepare-task-store" in remote:
                return CommandResult(
                    0,
                    '{"attempt":"trial-alpha-0","bytes":128,"files":2,'
                    '"mode":"protected-console-only","relative":"task-store",'
                    '"verb":"prepare-task-store"}\n',
                    "",
                )
            if "launch-task-app" in remote:
                store_mode = (
                    "shared-agent-console"
                    if "shared-agent-console" in remote
                    else "protected-console-only"
                )
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "launch-task-app",
                            "attempt": "trial-alpha-0",
                            "app_id": "example-desk",
                            "store_relative": "task-store",
                            "launch_arguments_sha256": hashlib.sha256(
                                b'["--record=ITEM-1042"]'
                            ).hexdigest(),
                            "store_mode": store_mode,
                            "store_path_sha256": "a" * 64,
                            "store_device": 1,
                            "store_inode": 2,
                            "target_pid": 746,
                            "target_process_identity": "b" * 64,
                        }
                    )
                    + "\n",
                    "",
                )
            if "installation-facts" in remote:
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "installation-facts",
                            "helper_sha256": "e" * 64,
                            "helper_mode": "0555",
                            "mediator_sha256": "d" * 64,
                            "mediator_mode": "0555",
                            "sudoers_sha256": "f" * 64,
                            "sudoers_mode": "0440",
                            "pf_main_rules_sha256": self.pf_main_rules_sha256,
                            "agent_passwordless_sudo_denied": True,
                            "agent_admin_group_member": False,
                            "agent_sudo_policy_denied": True,
                            "ssh_password_authentication": False,
                            "ssh_keyboard_interactive_authentication": False,
                        }
                    )
                    + "\n",
                    "",
                )
            if "launch-agent" in remote:
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "launch-agent",
                            "attempt": "trial-alpha-0",
                            "completed": False,
                            "supervisor_pid": 42,
                            "launch_envelope_sha256": self.launch_digest,
                        }
                    )
                    + "\n",
                    "",
                )
            if "wait-agent" in remote:
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "wait-agent",
                            "attempt": "trial-alpha-0",
                            "completed": True,
                            "exit_code": 0,
                            "stdout_b64": "b2sK",
                            "stdout_bytes": 3,
                            "stdout_sha256": hashlib.sha256(b"ok\n").hexdigest(),
                            "stdout_truncated": False,
                            "stderr_b64": "",
                            "stderr_bytes": 0,
                            "stderr_sha256": hashlib.sha256(b"").hexdigest(),
                            "stderr_truncated": False,
                            "launch_envelope_sha256": self.launch_digest,
                        }
                    )
                    + "\n",
                    "",
                )
            if "kill-agent" in remote:
                return CommandResult(
                    0,
                    '{"verb":"kill-agent","attempt":"trial-alpha-0","completed":true,"killed":true,"no_agent_processes":true}\n',
                    "",
                )
            if "network-apply" in remote:
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "network-apply",
                            "attempt": "trial-alpha-0",
                            "mode": "none",
                            "allowlist_sha256": "f" * 64,
                            "rules_sha256": "c" * 64,
                            "ruleset_main_sha256": "a" * 64,
                            "anchor_rules_sha256": "b" * 64,
                            "pf_enabled": True,
                            "residual_channels": [
                                "unix-domain-sockets",
                                "shared-filesystem",
                                "console-user-gui-egress",
                                "deferred-scheduling",
                            ],
                            "global_protocol_blocks": [
                                "dns",
                                "mdns",
                                "icmp",
                                "ipv6-icmp",
                            ],
                        }
                    )
                    + "\n",
                    "",
                )
            if "network-remove" in remote:
                return CommandResult(
                    0,
                    '{"verb":"network-remove","attempt":"trial-alpha-0","removed":true}\n',
                    "",
                )
            if "network-status" in remote:
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "verb": "network-status",
                            "attempt": "trial-alpha-0",
                            "mode": "none",
                            "allowlist_sha256": "f" * 64,
                            "rules_sha256": "c" * 64,
                            "ruleset_main_sha256": "a" * 64,
                            "anchor_rules_sha256": "b" * 64,
                            "pf_enabled": True,
                            "residual_channels": [
                                "unix-domain-sockets",
                                "shared-filesystem",
                                "console-user-gui-egress",
                                "deferred-scheduling",
                            ],
                            "global_protocol_blocks": [
                                "dns",
                                "mdns",
                                "icmp",
                                "ipv6-icmp",
                            ],
                        }
                    )
                    + "\n",
                    "",
                )
            if "permissions status --json" in remote:
                return CommandResult(
                    0,
                    json.dumps(
                        {
                            "accessibility": True,
                            "screen_recording": True,
                            "source": {"attribution": "driver-daemon"},
                        }
                    )
                    + "\n",
                    "",
                )
            return CommandResult(0, json.dumps(FACTS) + "\n", "")
        operation = command[1]
        if operation == "ls":
            if self.fail_inventory:
                return CommandResult(7, "", "inventory unavailable")
            records = [{"name": "seed-vm", "status": self.seed_status}]
            if self.attempt_exists:
                records.append({"name": "cdb-trial-alpha-0", "status": self.attempt_status})
            return CommandResult(0, json.dumps(records), "")
        if operation == "get":
            name = command[2]
            if name == "seed-vm":
                record = {"name": name, "status": self.seed_status}
            else:
                record = {
                    "name": name,
                    "status": self.attempt_status,
                    "sshAvailable": self.attempt_status == "running",
                    "ipAddress": "192.0.2.10" if self.attempt_status == "running" else None,
                    "vncUrl": None,
                    "networkMode": "nat",
                }
            return CommandResult(0, json.dumps([record]), "")
        if operation == "clone":
            self.attempt_exists = True
            self.attempt_status = "stopped"
        elif operation == "run":
            if not self.launch_stays_stopped:
                self.attempt_status = "running"
        elif operation == "stop":
            self.attempt_status = "stopped"
        elif operation == "delete":
            self.attempt_exists = False
        return CommandResult(0, "", "")


def protected_connection(manifest: Path) -> dict:
    identity = manifest.parent / "id_ed25519"
    known_hosts = manifest.parent / "known_hosts"
    identity.write_text("test-private-key", encoding="ascii")
    identity.chmod(0o600)
    known_hosts.write_text("cdb-seed ssh-ed25519 test-key\n", encoding="ascii")
    known_hosts.chmod(0o644)
    return {
        "ssh_identity_file": identity.resolve(),
        "ssh_known_hosts_file": known_hosts.resolve(),
        "ssh_known_hosts_sha256": digest_file(known_hosts),
        "ssh_host_key_alias": "cdb-seed",
        "privileged_helper_sha256": HELPER_DIGEST,
        "mediator_sha256": MEDIATOR_DIGEST,
        "privileged_sudoers_sha256": SUDOERS_DIGEST,
        "pf_main_rules_sha256": PF_MAIN_DIGEST,
        "expected_pristine_fingerprint": digest_json(FACTS),
    }


def control(
    fake: FakeLume,
    manifest: Path,
    fingerprint: str | None = None,
    *,
    input_runner=None,
) -> LumeControlPlane:
    binary = manifest.parent / "lume-test-bin"
    binary.write_bytes(b"test-lume-binary")
    arguments = dict(
        binary=binary,
        binary_sha256=digest_file(binary),
        seed_vm="seed-vm",
        seed_provenance_digest="sha256:" + "a" * 64,
        seed_manifest_path=manifest,
        driver_version="cua-driver 0.19.4",
        driver_sha256="sha256:" + "d" * 64,
        **protected_connection(manifest),
        runner=fake,
        poll_interval_seconds=0,
    )
    if fingerprint is not None:
        arguments["expected_pristine_fingerprint"] = fingerprint
    if input_runner is not None:
        arguments["input_runner"] = input_runner
    return LumeControlPlane(**arguments)


class LumeControlPlaneTests(unittest.TestCase):
    def test_mediator_start_binds_expected_tool_contract_argument_and_evidence(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        expected = "c" * 64
        evidence = {
            "verb": "mediator-start",
            "attempt": "trial-alpha",
            "worker_uid": 501,
            "agent_uid": 502,
            "frontend_mode": "0660",
            "frontend_owner_uid": 501,
            "mediator_sha256": "d" * 64,
            "backend_owner_uid": 501,
            "backend_parent_device": 1,
            "backend_parent_inode": 2,
            "backend_device": 3,
            "backend_inode": 4,
            "target_pid": 5,
            "task_id": "synthetic-task.v1",
            "expected_tool_contract_sha256": expected,
            "tool_contract_required": True,
            "expected_daemon_tool_list_envelope_sha256": None,
            "daemon_tool_list_envelope_required": False,
        }
        with mock.patch.object(
            controller, "helper", return_value=json.dumps(evidence) + "\n"
        ) as helper:
            self.assertEqual(
                controller.start_mediator(
                    "cdb-trial-alpha-0",
                    PurePosixPath("/attempts/trial-alpha"),
                    "synthetic-task.v1",
                    expected,
                ),
                evidence,
            )
        helper.assert_called_once_with(
            "cdb-trial-alpha-0",
            "mediator-start",
            ("trial-alpha", "synthetic-task.v1", expected),
            timeout_seconds=30.0,
        )
        with self.assertRaisesRegex(HarnessFailure, "digest is invalid"):
            controller.start_mediator(
                "cdb-trial-alpha-0",
                PurePosixPath("/attempts/trial-alpha"),
                "synthetic-task.v1",
                "C" * 64,
            )

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.manifest = Path(self.temporary.name) / ".manifest-digest"
        self.manifest.write_text("sha256:" + "a" * 64 + "\n", encoding="ascii")
        self.binary = Path(self.temporary.name) / "lume-test-bin"
        self.binary.write_bytes(b"test-lume-binary")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _interrupted_stop_control(
        self,
        stop_returncode: int = -signal.SIGINT,
        *,
        shutdown_stops: bool = True,
        ssh_available: bool = True,
        transient_invalid_gets_after_stop: int = 0,
    ) -> tuple[LumeControlPlane, dict[str, object], list[list[str]]]:
        vm_name = "cdb-trial-alpha-0"
        storage_root = Path(self.temporary.name) / "lume"
        vm_root = storage_root / vm_name
        vm_root.mkdir(parents=True)
        config = vm_root / "config.json"
        config.write_text("{}\n", encoding="ascii")
        state: dict[str, object] = {
            "status": "running",
            "exists": True,
            "invalid_gets": 0,
        }
        commands: list[list[str]] = []

        def runner(argv, timeout_seconds) -> CommandResult:
            del timeout_seconds
            command = list(argv)
            commands.append(command)
            if command[0] == "/usr/bin/ssh":
                if shutdown_stops:
                    state["status"] = "stopped"
                return CommandResult(255, "", "connection closed")
            operation = command[1]
            if operation == "get":
                invalid_gets = int(state["invalid_gets"])
                if invalid_gets:
                    state["invalid_gets"] = invalid_gets - 1
                    return CommandResult(0, "{", "")
                return CommandResult(
                    0,
                    json.dumps(
                        [
                            {
                                "name": vm_name,
                                "status": state["status"],
                                "ipAddress": (
                                    "192.0.2.10" if state["status"] == "running" else None
                                ),
                                "sshAvailable": (
                                    ssh_available if state["status"] == "running" else None
                                ),
                            }
                        ]
                    ),
                    "",
                )
            if operation == "stop":
                state["invalid_gets"] = transient_invalid_gets_after_stop
                return CommandResult(stop_returncode, "", "")
            if operation == "delete":
                if state["status"] != "stopped":
                    raise AssertionError("delete called before verified stop")
                state["exists"] = False
                config.unlink()
                vm_root.rmdir()
                return CommandResult(0, "", "")
            if operation == "ls":
                records = [{"name": vm_name, "status": state["status"]}] if state["exists"] else []
                return CommandResult(0, json.dumps(records), "")
            raise AssertionError(f"unexpected command: {command}")

        controller = LumeControlPlane(
            binary=self.binary,
            binary_sha256=digest_file(self.binary),
            seed_vm="seed-vm",
            seed_provenance_digest="sha256:" + "a" * 64,
            seed_manifest_path=self.manifest,
            storage_root=storage_root,
            driver_version="cua-driver 0.19.4",
            driver_sha256="sha256:" + "d" * 64,
            **protected_connection(self.manifest),
            runner=runner,
            poll_interval_seconds=0,
        )
        return controller, state, commands

    def test_provision_clones_stopped_seed_and_attests_guest(self) -> None:
        fake = FakeLume()
        expected = digest_json(FACTS)
        attempt = control(fake, self.manifest, expected).provision("trial.alpha.0")
        self.assertEqual(attempt.vm_name, "cdb-trial-alpha-0")
        self.assertEqual(attempt.pristine_fingerprint, expected)
        self.assertEqual(attempt.seed_provenance_digest, "sha256:" + "a" * 64)
        self.assertEqual(attempt.ip_address, "192.0.2.10")
        self.assertEqual(
            attempt.apparatus_evidence["privileged_helper"]["pf_main_rules_sha256"],
            PF_MAIN_DIGEST,
        )
        self.assertIn(
            [str(self.binary.resolve()), "clone", "seed-vm", "cdb-trial-alpha-0"],
            fake.commands,
        )
        run = next(command for command in fake.commands if command[1] == "run")
        self.assertEqual(
            run,
            [
                str(self.binary.resolve()),
                "run",
                "cdb-trial-alpha-0",
                "--display",
                "none",
                "--vnc",
                "disabled",
                "--detach",
                "--network",
                "nat",
            ],
        )

    def test_provision_rejects_main_pf_ruleset_digest_drift_early(self) -> None:
        fake = FakeLume()
        fake.pf_main_rules_sha256 = "b" * 64
        with self.assertRaisesRegex(HarnessFailure, "main PF ruleset digest mismatch"):
            control(fake, self.manifest).provision("trial.alpha.0")
        remotes = [command[-1] for command in fake.commands if command[0] == "/usr/bin/ssh"]
        self.assertFalse(any("start-driver" in remote for remote in remotes))
        self.assertFalse(fake.attempt_exists)

    def test_provision_rejects_malformed_main_pf_ruleset_digest(self) -> None:
        for malformed in ("A" * 64, PF_MAIN_DIGEST, None):
            with self.subTest(malformed=malformed):
                fake = FakeLume()
                fake.pf_main_rules_sha256 = malformed
                with self.assertRaisesRegex(HarnessFailure, "main PF ruleset digest is malformed"):
                    control(fake, self.manifest).provision("trial.alpha.0")
                self.assertFalse(fake.attempt_exists)

    def test_mismatched_pristine_fingerprint_destroys_clone(self) -> None:
        fake = FakeLume()
        with self.assertRaisesRegex(HarnessFailure, "pristine fingerprint"):
            control(fake, self.manifest, "sha256:" + "b" * 64).provision("trial.alpha.0")
        self.assertFalse(fake.attempt_exists)
        self.assertTrue(any(command[1] == "delete" for command in fake.commands))

    def test_running_seed_is_rejected_before_clone(self) -> None:
        fake = FakeLume(seed_status="running")
        with self.assertRaisesRegex(HarnessFailure, "must be stopped"):
            control(fake, self.manifest).provision("trial.alpha.0")
        self.assertFalse(any(command[1] == "clone" for command in fake.commands))

    def test_zero_exit_launch_that_remains_stopped_fails_and_cleans_up(self) -> None:
        fake = FakeLume(launch_stays_stopped=True)
        with self.assertRaisesRegex(HarnessFailure, "remained stopped"):
            control(fake, self.manifest).provision("trial.alpha.0", timeout_seconds=0.01)
        self.assertFalse(fake.attempt_exists)

    def test_unsafe_names_and_digest_are_rejected(self) -> None:
        fake = FakeLume()
        for seed in ("../seed", "UPPER", "name_with_underscore", "x" * 64):
            with self.subTest(seed=seed), self.assertRaises(ValidationFailure):
                LumeControlPlane(
                    binary=self.binary,
                    binary_sha256=digest_file(self.binary),
                    seed_vm=seed,
                    seed_provenance_digest="sha256:" + "a" * 64,
                    seed_manifest_path=self.manifest,
                    **protected_connection(self.manifest),
                    runner=fake,
                )
        with self.assertRaisesRegex(ValidationFailure, "seed provenance digest"):
            LumeControlPlane(
                binary=self.binary,
                binary_sha256=digest_file(self.binary),
                seed_vm="seed-vm",
                seed_provenance_digest="latest",
                seed_manifest_path=self.manifest,
                **protected_connection(self.manifest),
                runner=fake,
            )
        connection = protected_connection(self.manifest)
        connection["expected_pristine_fingerprint"] = "latest"
        with self.assertRaisesRegex(ValidationFailure, "pristine fingerprint"):
            LumeControlPlane(
                binary=self.binary,
                binary_sha256=digest_file(self.binary),
                seed_vm="seed-vm",
                seed_provenance_digest="sha256:" + "a" * 64,
                seed_manifest_path=self.manifest,
                driver_version="cua-driver 0.19.4",
                driver_sha256="sha256:" + "d" * 64,
                **connection,
                runner=fake,
            )

    def test_seed_provenance_digest_mismatch_is_rejected_before_clone(self) -> None:
        fake = FakeLume()
        self.manifest.write_text("sha256:" + "b" * 64 + "\n", encoding="ascii")
        with self.assertRaisesRegex(HarnessFailure, "finalized seed seal"):
            control(fake, self.manifest).provision("trial.alpha.0")
        self.assertFalse(any(command[1] == "clone" for command in fake.commands))

    def test_inspect_retries_only_bounded_transient_parse_failures(self) -> None:
        fake = FakeLume()
        calls = 0

        def transient(argv, timeout_seconds):
            nonlocal calls
            if argv[1:4] == ["get", "cdb-trial-alpha-0", "--format"]:
                calls += 1
                if calls < 3:
                    return CommandResult(0, "", "")
            return fake(argv, timeout_seconds)

        record = control(transient, self.manifest).inspect("cdb-trial-alpha-0")
        self.assertEqual(record["name"], "cdb-trial-alpha-0")
        self.assertEqual(calls, 3)

        calls = 0

        def persistent(argv, timeout_seconds):
            del timeout_seconds
            nonlocal calls
            if argv[1:4] == ["get", "cdb-trial-alpha-0", "--format"]:
                calls += 1
                return CommandResult(0, "[]", "")
            return fake(argv, 0)

        with self.assertRaisesRegex(HarnessFailure, "returned invalid JSON"):
            control(persistent, self.manifest).inspect("cdb-trial-alpha-0")
        self.assertEqual(calls, 3)

    def test_exists_retries_only_bounded_transient_parse_failures(self) -> None:
        fake = FakeLume()
        fake.attempt_exists = True
        calls = 0

        def transient(argv, timeout_seconds):
            nonlocal calls
            if argv[1:4] == ["ls", "--format", "json"]:
                calls += 1
                if calls < 3:
                    return CommandResult(0, "", "")
            return fake(argv, timeout_seconds)

        self.assertTrue(control(transient, self.manifest).exists("cdb-trial-alpha-0"))
        self.assertEqual(calls, 3)

        calls = 0

        def persistent(argv, timeout_seconds):
            del timeout_seconds
            nonlocal calls
            if argv[1:4] == ["ls", "--format", "json"]:
                calls += 1
                return CommandResult(0, "{}", "")
            return fake(argv, 0)

        with self.assertRaisesRegex(HarnessFailure, "invalid VM records"):
            control(persistent, self.manifest).exists("cdb-trial-alpha-0")
        self.assertEqual(calls, 3)

    def test_command_failure_is_bounded_and_sanitized(self) -> None:
        def failing(argv, timeout_seconds) -> CommandResult:
            del argv, timeout_seconds
            return CommandResult(9, "", "failed without a secret")

        controller = LumeControlPlane(
            binary=self.binary,
            binary_sha256=digest_file(self.binary),
            seed_vm="seed-vm",
            seed_provenance_digest="sha256:" + "a" * 64,
            seed_manifest_path=self.manifest,
            driver_version="cua-driver 0.19.4",
            driver_sha256="sha256:" + "d" * 64,
            **protected_connection(self.manifest),
            runner=failing,
        )
        with self.assertRaisesRegex(HarnessFailure, "failed with 9"):
            controller.inspect("seed-vm")

    def test_interrupted_lume_stop_uses_bound_guest_shutdown(self) -> None:
        controller, state, commands = self._interrupted_stop_control()
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.99"
        controller.stop("cdb-trial-alpha-0", timeout_seconds=0.1)
        self.assertEqual(state["status"], "stopped")
        ssh = [command for command in commands if command[0] == "/usr/bin/ssh"]
        self.assertEqual(len(ssh), 1)
        self.assertEqual(
            ssh[0][-1],
            "/usr/bin/sudo -n -- /usr/local/libexec/cdb-helper shutdown",
        )
        self.assertEqual(ssh[0][-2], "lume@192.0.2.10")

    def test_interrupted_stop_retries_transient_invalid_inspection(self) -> None:
        controller, state, commands = self._interrupted_stop_control(
            transient_invalid_gets_after_stop=1
        )

        controller.stop("cdb-trial-alpha-0", timeout_seconds=0.1)

        self.assertEqual(state["status"], "stopped")
        gets = [
            command for command in commands if command[:2] == [str(self.binary.resolve()), "get"]
        ]
        self.assertGreaterEqual(len(gets), 3)
        self.assertEqual(sum(command[0] == "/usr/bin/ssh" for command in commands), 1)

    def test_interrupted_stop_requires_fresh_ssh_availability(self) -> None:
        controller, _state, commands = self._interrupted_stop_control(ssh_available=False)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.99"
        with self.assertRaisesRegex(HarnessFailure, "current SSH identity"):
            controller.stop("cdb-trial-alpha-0", timeout_seconds=0.1)
        self.assertFalse(any(command[0] == "/usr/bin/ssh" for command in commands))

    def test_non_sigint_lume_stop_failure_never_requests_guest_shutdown(self) -> None:
        controller, _state, _commands = self._interrupted_stop_control(9)
        with self.assertRaisesRegex(HarnessFailure, "lume stop failed with 9"):
            controller.stop("cdb-trial-alpha-0", timeout_seconds=0.1)
        self.assertFalse(any(command[0] == "/usr/bin/ssh" for command in _commands))

    def test_destroy_recovers_interrupted_stop_before_exact_delete(self) -> None:
        controller, state, commands = self._interrupted_stop_control()

        controller.destroy("cdb-trial-alpha-0")
        operations = [
            command[1]
            for command in commands
            if command and command[0] == str(self.binary.resolve())
        ]
        self.assertLess(operations.index("stop"), operations.index("delete"))
        self.assertFalse(state["exists"])

    def test_interrupted_stop_fails_closed_if_guest_does_not_stop(self) -> None:
        controller, _state, commands = self._interrupted_stop_control(shutdown_stops=False)
        with self.assertRaisesRegex(HarnessFailure, "did not stop before collection"):
            controller.stop("cdb-trial-alpha-0", timeout_seconds=0.001)
        self.assertEqual(sum(command[0] == "/usr/bin/ssh" for command in commands), 1)

    def test_sigint_stop_that_already_stopped_never_uses_ssh(self) -> None:
        controller, state, commands = self._interrupted_stop_control()
        original = controller.runner

        def stopped_during_cli(argv, timeout_seconds) -> CommandResult:
            result = original(argv, timeout_seconds)
            command = list(argv)
            if command[0] == str(self.binary.resolve()) and command[1] == "stop":
                state["status"] = "stopped"
            return result

        controller.runner = stopped_during_cli
        controller.stop("cdb-trial-alpha-0", timeout_seconds=0.1)
        self.assertFalse(any(command[0] == "/usr/bin/ssh" for command in commands))

    def test_destroy_is_idempotent_only_after_verified_absence(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        controller.destroy("cdb-trial-alpha-0")
        self.assertFalse(any(command[1] == "delete" for command in fake.commands))

        fake.fail_inventory = True
        with self.assertRaisesRegex(HarnessFailure, "inventory unavailable"):
            controller.destroy("cdb-trial-alpha-0")

    def test_private_key_content_never_enters_process_arguments(self) -> None:
        secret = "private-key-content-must-stay-in-file"

        def echoing(argv, timeout_seconds) -> CommandResult:
            del timeout_seconds
            self.assertNotIn(secret, "\n".join(argv))
            return CommandResult(9, "", "synthetic failure")

        connection = protected_connection(self.manifest)
        connection["ssh_identity_file"].write_text(secret, encoding="ascii")
        connection["ssh_identity_file"].chmod(0o600)

        controller = LumeControlPlane(
            binary=self.binary,
            binary_sha256=digest_file(self.binary),
            seed_vm="seed-vm",
            seed_provenance_digest="sha256:" + "a" * 64,
            seed_manifest_path=self.manifest,
            driver_version="cua-driver 0.19.4",
            driver_sha256="sha256:" + "d" * 64,
            **connection,
            runner=echoing,
        )
        with self.assertRaises(HarnessFailure) as raised:
            controller.inspect("seed-vm")
        self.assertNotIn(secret, str(raised.exception))
        controller._vm_addresses["seed-vm"] = "192.0.2.10"
        result = controller.ssh_result("seed-vm", "false")
        self.assertNotIn(secret, result.stdout + result.stderr)

    def test_known_hosts_change_is_rejected_before_transport(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        controller.ssh_known_hosts_file.write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(HarnessFailure, "known_hosts changed"):
            controller.ssh("cdb-trial-alpha-0", "/usr/bin/true")
        self.assertFalse(any(command[0] == "/usr/bin/ssh" for command in fake.commands))

    def test_payload_uses_private_inbox_scp_without_bytes_in_arguments(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        payload = b"payload-bytes-must-not-enter-argv"
        root = controller.stage_archive(
            "cdb-trial-alpha-0",
            "trial.alpha.0",
            payload,
            hashlib.sha256(payload).hexdigest(),
        )
        self.assertEqual(root.name, "trial-alpha-0")
        scp = next(command for command in fake.commands if command[0] == "/usr/bin/scp")
        self.assertIn(
            "/Users/lume/Library/Caches/cdb-inbox/cdb-trial-alpha-0-payload.tar.gz",
            scp[-1],
        )
        self.assertNotIn(payload.decode(), "\n".join(scp))

    def test_network_policy_uses_fixed_helper_verb_and_binds_evidence(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        root = controller.attempt_root("trial.alpha.0")
        evidence = controller.apply_network("cdb-trial-alpha-0", root, "none", None)
        self.assertEqual(evidence["mode"], "none")
        self.assertEqual(evidence["enforcer"], "guest-root-pf-anchor")
        self.assertRegex(evidence["evidence_digest"], r"^sha256:[a-f0-9]{64}$")
        remote = next(
            command[-1]
            for command in fake.commands
            if command[0] == "/usr/bin/ssh" and "network-apply" in command[-1]
        )
        self.assertIn("cdb-helper network-apply trial-alpha-0 none", remote)
        self.assertNotIn("sudo -S", remote)
        self.assertNotIn("/bin/sh -c", remote)
        verified = controller.verify_network("cdb-trial-alpha-0", root, evidence)
        self.assertEqual(verified, evidence)
        removed = controller.remove_network("cdb-trial-alpha-0", root)
        self.assertTrue(removed["removed"])

    def test_console_share_and_task_app_use_fixed_helper_verbs(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        root = controller.attempt_root("trial.alpha.0")
        controller.share_console_path("cdb-trial-alpha-0", root, "task-store")
        controller.launch_task_app(
            "cdb-trial-alpha-0",
            root,
            "example-desk",
            "task-store",
            ("--record=ITEM-1042",),
        )
        remotes = [command[-1] for command in fake.commands if command[0] == "/usr/bin/ssh"]
        self.assertTrue(
            any(
                "cdb-helper share-console-path trial-alpha-0 task-store" in command
                for command in remotes
            )
        )
        self.assertTrue(
            any(
                "cdb-helper launch-task-app trial-alpha-0 example-desk task-store "
                "'[\"--record=ITEM-1042\"]' protected-console-only" in command
                for command in remotes
            )
        )

    def test_human_input_evidence_requires_null_vnc_and_no_listener(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        evidence = controller.host_human_input_evidence("cdb-trial-alpha-0", {"vncUrl": None})
        self.assertEqual(evidence["listening_tcp_sockets"], 0)
        self.assertRegex(evidence["evidence_digest"], r"^sha256:[a-f0-9]{64}$")
        with self.assertRaisesRegex(HarnessFailure, "VNC URL"):
            controller.host_human_input_evidence(
                "cdb-trial-alpha-0", {"vncUrl": "vnc://127.0.0.1:5900"}
            )

    def test_agent_run_uses_detached_fixed_verbs_and_decodes_output(self) -> None:
        fake = FakeLume()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        launch = rendered_launch()
        result = controller.run_agent("cdb-trial-alpha-0", launch, 30.0)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "ok\n", ""))
        remotes = [command[-1] for command in fake.commands if command[0] == "/usr/bin/ssh"]
        self.assertTrue(any("launch-agent" in command for command in remotes))
        self.assertTrue(any("wait-agent" in command for command in remotes))
        self.assertFalse(any("run-agent" in command for command in remotes))

    def test_credential_launch_uses_stdin_and_sanitizes_failures(self) -> None:
        secret = b"sk-test-secret-never-in-argv"
        fake = FakeLume()
        launch = RenderedGuestLaunch(
            argv=("/usr/local/bin/codex", "exec", "-"),
            cwd="/Users/Shared/cdb-attempts/trial-alpha-0/workspace",
            environment={"HOME": "/Users/Shared/cdb-attempts/trial-alpha-0/home"},
            stdin_path="/Users/Shared/cdb-attempts/trial-alpha-0/task/brief.md",
            harness_kind="codex",
            executable_sha256="a" * 64,
            credential_names=("OPENAI_API_KEY",),
        )
        fake.launch_digest = hashlib.sha256(canonical_json(launch.document()) + b"\n").hexdigest()
        calls = []

        def input_runner(argv, timeout_seconds, input_bytes):
            del timeout_seconds
            calls.append((tuple(argv), input_bytes))
            return CommandResult(
                0,
                json.dumps(
                    {
                        "verb": "launch-agent",
                        "attempt": "trial-alpha-0",
                        "completed": False,
                        "supervisor_pid": 42,
                        "launch_envelope_sha256": fake.launch_digest,
                    }
                )
                + "\n",
                "",
            )

        controller = control(fake, self.manifest, input_runner=input_runner)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        result = controller.run_agent(
            "cdb-trial-alpha-0",
            launch,
            30.0,
            credential_environment={"OPENAI_API_KEY": secret.decode()},
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(calls), 1)
        argv, stdin = calls[0]
        self.assertEqual(stdin, b'{"OPENAI_API_KEY":"sk-test-secret-never-in-argv"}')
        self.assertNotIn("-n", argv)
        self.assertNotIn(secret.decode(), " ".join(argv))
        self.assertIn("launch-agent-with-lease", argv[-1])

        def failed_input_runner(argv, timeout_seconds, input_bytes):
            del argv, timeout_seconds, input_bytes
            return CommandResult(1, "", secret.decode())

        failed = control(fake, self.manifest, input_runner=failed_input_runner)
        failed._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        with self.assertRaises(HarnessFailure) as raised:
            failed.run_agent(
                "cdb-trial-alpha-0",
                launch,
                30.0,
                credential_environment={"OPENAI_API_KEY": secret.decode()},
            )
        self.assertNotIn(secret.decode(), str(raised.exception))

        def staged_failure(argv, timeout_seconds, input_bytes):
            del argv, timeout_seconds, input_bytes
            return CommandResult(64, "", "cdb-helper: launch-stage-codex-auth-validate\n")

        staged = control(fake, self.manifest, input_runner=staged_failure)
        staged._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        with self.assertRaisesRegex(
            HarnessFailure, r"with 64 at launch-stage-codex-auth-validate$"
        ):
            staged.run_agent(
                "cdb-trial-alpha-0",
                launch,
                30.0,
                credential_environment={"OPENAI_API_KEY": secret.decode()},
            )

        def forged_failure(argv, timeout_seconds, input_bytes):
            del argv, timeout_seconds, input_bytes
            return CommandResult(
                64,
                "",
                "cdb-helper: launch-stage-codex-auth-validate\n" + secret.decode(),
            )

        forged = control(fake, self.manifest, input_runner=forged_failure)
        forged._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        with self.assertRaises(HarnessFailure) as forged_raised:
            forged.run_agent(
                "cdb-trial-alpha-0",
                launch,
                30.0,
                credential_environment={"OPENAI_API_KEY": secret.decode()},
            )
        self.assertEqual(
            str(forged_raised.exception),
            "privileged credential-bearing agent launch failed with 64",
        )
        self.assertNotIn(secret.decode(), str(forged_raised.exception))

    def test_attempt_slug_is_single_source_for_vm_and_guest_root(self) -> None:
        trial_id = "x" * 80
        root = LumeControlPlane.attempt_root(trial_id)
        vm_name = LumeControlPlane.attempt_name(trial_id)
        self.assertEqual(len(root.name), 59)
        self.assertEqual(vm_name, "cdb-" + root.name)
        self.assertLessEqual(len(vm_name), 63)
        self.assertEqual(
            LumeControlPlane.protected_driver_socket(trial_id),
            PurePosixPath("/private/var/run/cdb-mediator") / root.name / "driver.sock",
        )

    def test_wait_transport_deadline_kills_bound_attempt_and_raises_deadline(self) -> None:
        class WaitTimeout(FakeLume):
            def __call__(self, argv, timeout_seconds):
                command = list(argv)
                if command[0] == "/usr/bin/ssh" and "wait-agent" in command[-1]:
                    self.commands.append(command)
                    return CommandResult(124, "", "host deadline exceeded")
                return super().__call__(argv, timeout_seconds)

        fake = WaitTimeout()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        launch = rendered_launch()
        with self.assertRaisesRegex(DeadlineExceeded, "exceeded"):
            controller.run_agent("cdb-trial-alpha-0", launch, 30.0)
        remotes = [command[-1] for command in fake.commands if command[0] == "/usr/bin/ssh"]
        self.assertTrue(any("kill-agent trial-alpha-0" in command for command in remotes))

    def test_agent_output_preserves_full_digest_and_truncation_metadata(self) -> None:
        full_output = b"x" * 70000

        class TruncatedOutput(FakeLume):
            def __call__(self, argv, timeout_seconds):
                command = list(argv)
                if command[0] == "/usr/bin/ssh" and "wait-agent" in command[-1]:
                    self.commands.append(command)
                    return CommandResult(
                        0,
                        json.dumps(
                            {
                                "verb": "wait-agent",
                                "attempt": "trial-alpha-0",
                                "completed": True,
                                "exit_code": 0,
                                "stdout_b64": base64.b64encode(full_output[-65536:]).decode(),
                                "stdout_bytes": len(full_output),
                                "stdout_sha256": hashlib.sha256(full_output).hexdigest(),
                                "stdout_truncated": True,
                                "stderr_b64": "",
                                "stderr_bytes": 0,
                                "stderr_sha256": hashlib.sha256(b"").hexdigest(),
                                "stderr_truncated": False,
                                "launch_envelope_sha256": self.launch_digest,
                            }
                        )
                        + "\n",
                        "",
                    )
                return super().__call__(argv, timeout_seconds)

        fake = TruncatedOutput()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        launch = rendered_launch()
        result = controller.run_agent("cdb-trial-alpha-0", launch, 30.0)
        self.assertEqual(len(result.stdout.encode()), 65536)
        self.assertEqual(result.stdout_bytes, 70000)
        self.assertEqual(result.stdout_sha256, hashlib.sha256(full_output).hexdigest())
        self.assertTrue(result.stdout_truncated)

    def test_agent_output_recovers_bounded_streams_after_timeout_kill(self) -> None:
        class KilledOutput(FakeLume):
            def __call__(self, argv, timeout_seconds):
                command = list(argv)
                if command[0] == "/usr/bin/ssh" and "wait-agent" in command[-1]:
                    self.commands.append(command)
                    return CommandResult(
                        0,
                        json.dumps(
                            {
                                "verb": "wait-agent",
                                "attempt": "trial-alpha-0",
                                "completed": True,
                                "exit_code": None,
                                "killed": True,
                                "stdout_b64": "b2sK",
                                "stdout_bytes": 3,
                                "stdout_sha256": hashlib.sha256(b"ok\n").hexdigest(),
                                "stdout_truncated": False,
                                "stderr_b64": "",
                                "stderr_bytes": 0,
                                "stderr_sha256": hashlib.sha256(b"").hexdigest(),
                                "stderr_truncated": False,
                                "launch_envelope_sha256": self.launch_digest,
                            }
                        )
                        + "\n",
                        "",
                    )
                return super().__call__(argv, timeout_seconds)

        fake = KilledOutput()
        controller = control(fake, self.manifest)
        controller._vm_addresses["cdb-trial-alpha-0"] = "192.0.2.10"
        result = controller.agent_output("cdb-trial-alpha-0", rendered_launch())

        self.assertIsNone(result.returncode)
        self.assertEqual(result.stdout, "ok\n")
        remotes = [command[-1] for command in fake.commands if command[0] == "/usr/bin/ssh"]
        self.assertTrue(any("wait-agent trial-alpha-0 0" in command for command in remotes))

    def test_disk_detach_retries_with_force_and_fails_closed(self) -> None:
        failed = CommandResult(1, "", "busy")
        succeeded = CommandResult(0, "", "")
        with mock.patch(
            "cua_bench_runtime.lume.subprocess.run", side_effect=[failed, succeeded]
        ) as run:
            LumeControlPlane._detach_device("/dev/disk99")
        self.assertEqual(run.call_count, 2)
        self.assertEqual(
            run.call_args_list[1].args[0],
            ["/usr/bin/hdiutil", "detach", "-force", "/dev/disk99"],
        )
        with mock.patch("cua_bench_runtime.lume.subprocess.run", return_value=failed):
            with self.assertRaisesRegex(HarnessFailure, "detachment"):
                LumeControlPlane._detach_device("/dev/disk99")


if __name__ == "__main__":
    unittest.main()
