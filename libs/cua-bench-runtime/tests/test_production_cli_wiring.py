from __future__ import annotations

import hashlib
import json
import os
import tempfile
import time
import unittest
from pathlib import Path, PurePosixPath
from types import SimpleNamespace
from unittest import mock

from cua_bench_runtime.adapters.agent_harnesses.production import (
    BundledSkillFile,
    HarnessKind,
    ModelRoute,
    ProxyConfiguration,
    ProductionHarnessSpec,
    canonical_provider_allowlist_sha256,
)
from cua_bench_runtime.adapters.agent_harnesses.system import ProductionSystemPlan
from cua_bench_runtime.adapters.local import adapters
from cua_bench_runtime.cli import main
from cua_bench_runtime.credential_lease import CredentialLease
from cua_bench_runtime.engine import (
    MAX_AGENT_BRIEF_BYTES,
    _credential_lease_from_file,
    _read_agent_brief,
    _validate_agent_brief_option,
    _validate_production_credential_option,
    _validate_evaluator_node_contract,
    run_trial,
)
from cua_bench_runtime.errors import UsageFailure


class ProductionCliWiringTests(unittest.TestCase):
    def plan(self, credentials: tuple[str, ...] = ("OPENAI_API_KEY",)) -> ProductionSystemPlan:
        provider_allowlist = ("api.openai.com@443",)
        route = ModelRoute(
            route_id="primary",
            role="primary",
            provider="openai",
            model="gpt-5",
            snapshot="gpt-5-2025-01-01",
            service_tier="default",
        )
        return ProductionSystemPlan(
            kind=HarnessKind.CODEX,
            harness=ProductionHarnessSpec(HarnessKind.CODEX, "/usr/local/bin/codex"),
            model_route=route,
            guest_executable="/usr/local/bin/codex",
            guest_executable_sha256="a" * 64,
            credential_environment=credentials,
            proxy=ProxyConfiguration(
                endpoint="192.0.2.10@8443",
                provider_allowlist=provider_allowlist,
                provider_allowlist_sha256=canonical_provider_allowlist_sha256(provider_allowlist),
                implementation_sha256="f" * 64,
            ),
            skill_files=(BundledSkillFile(PurePosixPath("SKILL.md"), b"# Cua Driver\n"),),
            policy_digest="sha256:" + "b" * 64,
            daemon_tool_schemas_sha256="c" * 64,
            mcp_tool_schemas_sha256="d" * 64,
            daemon_tools_list_envelope_sha256="f" * 64,
            mcp_tools_list_envelope_sha256="1" * 64,
            tool_inventory_sha256="e" * 64,
            daemon_tool_count=1,
            mcp_tool_count=1,
            support_executables=(("/usr/local/bin/codex-code-mode-host", "c" * 64),),
        )

    def test_cli_forwards_credential_file_without_reading_it(self) -> None:
        credential = Path("/private/credential")
        with mock.patch(
            "cua_bench_runtime.cli.run_trial",
            return_value=(0, Path("/trial"), {"trial_id": "trial", "status": "completed"}),
        ) as run_trial:
            code = main(
                [
                    "run",
                    "--task",
                    "task.json",
                    "--agent",
                    "agent",
                    "--out",
                    "out",
                    "--credential-file",
                    str(credential),
                    "--json",
                ]
            )
        self.assertEqual(code, 0)
        self.assertEqual(run_trial.call_args.kwargs["credential_file_path"], credential)

    def test_cli_forwards_debug_mode(self) -> None:
        with mock.patch(
            "cua_bench_runtime.cli.run_trial",
            return_value=(
                4,
                Path("/trial"),
                {"trial_id": "trial", "status": "timeout"},
            ),
        ) as run_trial:
            code = main(
                [
                    "run",
                    "--task",
                    "task.json",
                    "--agent",
                    "agent",
                    "--out",
                    "out",
                    "--debug",
                ]
            )

        self.assertEqual(code, 4)
        self.assertTrue(run_trial.call_args.kwargs["debug_mode"])

    def test_cli_forwards_agent_brief_without_reading_it(self) -> None:
        brief = Path("hostile.md")
        with mock.patch(
            "cua_bench_runtime.cli.run_trial",
            return_value=(
                0,
                Path("/trial"),
                {"trial_id": "trial", "status": "completed"},
            ),
        ) as run_trial:
            code = main(
                [
                    "run",
                    "--task",
                    "task.json",
                    "--agent",
                    "agent",
                    "--out",
                    "out",
                    "--agent-brief",
                    str(brief),
                    "--apparatus-check",
                ]
            )
        self.assertEqual(code, 0)
        self.assertEqual(run_trial.call_args.kwargs["agent_brief_path"], brief)

    def test_cli_forwards_explicit_evaluator_node_contract(self) -> None:
        node = Path("/opt/evaluator/node")
        with mock.patch(
            "cua_bench_runtime.cli.run_trial",
            return_value=(
                0,
                Path("/trial"),
                {"trial_id": "trial", "status": "completed"},
            ),
        ) as run_trial:
            code = main(
                [
                    "run",
                    "--task",
                    "task.json",
                    "--agent",
                    "agent",
                    "--out",
                    "out",
                    "--evaluator-node",
                    str(node),
                    "--evaluator-node-sha256",
                    "a" * 64,
                    "--evaluator-node-version",
                    "v26.7.0",
                ]
            )
        self.assertEqual(code, 0)
        self.assertEqual(run_trial.call_args.kwargs["evaluator_node_path"], node)
        self.assertEqual(run_trial.call_args.kwargs["evaluator_node_sha256"], "a" * 64)
        self.assertEqual(run_trial.call_args.kwargs["evaluator_node_version"], "v26.7.0")

    def test_evaluator_node_contract_validates_regular_file_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            node = Path(directory) / "node"
            node.write_bytes(b"pinned evaluator node")
            node.chmod(0o700)
            expected_sha256 = hashlib.sha256(node.read_bytes()).hexdigest()
            completed = SimpleNamespace(returncode=0, stdout="v26.7.0\n")
            with mock.patch("cua_bench_runtime.engine.subprocess.run", return_value=completed):
                _validate_evaluator_node_contract(node, expected_sha256, "v26.7.0")

            link = Path(directory) / "node-link"
            link.symlink_to(node)
            with self.assertRaisesRegex(UsageFailure, "non-symlink"):
                _validate_evaluator_node_contract(link, expected_sha256, "v26.7.0")

    def test_agent_brief_override_is_fail_closed_to_production_apparatus_lume(
        self,
    ) -> None:
        brief = Path("brief.md")
        plan = self.plan()
        _validate_agent_brief_option(
            None,
            apparatus_check_requested=False,
            environment_name="local",
            production_plan=None,
        )
        with self.assertRaisesRegex(UsageFailure, "requires --apparatus-check"):
            _validate_agent_brief_option(
                brief,
                apparatus_check_requested=False,
                environment_name="lume-macos",
                production_plan=plan,
            )
        with self.assertRaisesRegex(UsageFailure, "production Lume"):
            _validate_agent_brief_option(
                brief,
                apparatus_check_requested=True,
                environment_name="local",
                production_plan=plan,
            )
        with self.assertRaisesRegex(UsageFailure, "production system"):
            _validate_agent_brief_option(
                brief,
                apparatus_check_requested=True,
                environment_name="lume-macos",
                production_plan=None,
            )
        _validate_agent_brief_option(
            brief,
            apparatus_check_requested=True,
            environment_name="lume-macos",
            production_plan=plan,
        )

    @unittest.skipUnless(
        hasattr(os, "geteuid") and hasattr(os, "O_NOFOLLOW"),
        "secure agent brief reads require POSIX no-follow support",
    )
    def test_agent_brief_reader_rejects_symlink_nonregular_and_oversize(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            brief = root / "brief.md"
            brief.write_text("brief", encoding="utf-8")
            link = root / "brief-link.md"
            link.symlink_to(brief)
            with self.assertRaisesRegex(UsageFailure, "non-symlink"):
                _read_agent_brief(link)
            with self.assertRaisesRegex(UsageFailure, "non-symlink"):
                _read_agent_brief(root)
            with brief.open("wb") as handle:
                handle.truncate(MAX_AGENT_BRIEF_BYTES + 1)
            with self.assertRaisesRegex(UsageFailure, "size limit"):
                _read_agent_brief(brief)

    @unittest.skipUnless(
        hasattr(os, "geteuid") and hasattr(os, "O_NOFOLLOW"),
        "secure agent brief reads require POSIX no-follow support",
    )
    def test_agent_brief_reader_detects_path_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            brief = Path(directory) / "brief.md"
            brief.write_text("brief", encoding="utf-8")
            actual = os.stat(brief)
            changed = list(actual)
            changed[1] += 1
            with mock.patch(
                "cua_bench_runtime.engine.os.fstat", return_value=os.stat_result(changed)
            ):
                with self.assertRaisesRegex(UsageFailure, "changed during secure open"):
                    _read_agent_brief(brief)

    @unittest.skipUnless(
        hasattr(os, "geteuid") and hasattr(os, "O_NOFOLLOW"),
        "secure credential file reads require POSIX no-follow support",
    )
    def test_secure_reader_builds_bound_lease_and_scrubs_source_buffer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "credential"
            path.write_bytes(b"secret-value")
            path.chmod(0o600)
            captured: dict[str, object] = {}
            from cua_bench_runtime.credential_lease import CredentialLease

            real_lease = CredentialLease

            def capture(**kwargs: object) -> CredentialLease:
                captured.update(kwargs)
                return real_lease(  # type: ignore[arg-type]
                    **kwargs, clock=lambda: 1_000.0
                )

            with (
                mock.patch("cua_bench_runtime.engine.time.time", return_value=1_000.0),
                mock.patch("cua_bench_runtime.engine.CredentialLease", side_effect=capture),
            ):
                lease = _credential_lease_from_file(path, plan=self.plan(), agent_limit=45.0)

            self.assertEqual(captured["expires_at"], 1_345.0)
            self.assertEqual(captured["provider"], "openai")
            self.assertEqual(captured["harness"], "codex")
            source = captured["credentials"]
            self.assertEqual(
                bytes(source["OPENAI_API_KEY"]),  # type: ignore[index]
                b"\0" * 12,
            )
            self.assertEqual(
                lease.consume(provider="openai", harness="codex"),
                {"OPENAI_API_KEY": "secret-value"},
            )

    @unittest.skipUnless(
        hasattr(os, "geteuid") and hasattr(os, "O_NOFOLLOW"),
        "secure credential file reads require POSIX no-follow support",
    )
    def test_secure_reader_rejects_unsafe_files_and_ambiguous_mapping(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            credential = root / "credential"
            credential.write_text("secret", encoding="utf-8")
            credential.chmod(0o644)
            with self.assertRaisesRegex(UsageFailure, "mode 0600"):
                _credential_lease_from_file(credential, plan=self.plan(), agent_limit=1.0)

            credential.chmod(0o600)
            link = root / "link"
            link.symlink_to(credential)
            with self.assertRaisesRegex(UsageFailure, "non-symlink"):
                _credential_lease_from_file(link, plan=self.plan(), agent_limit=1.0)
            link.unlink()
            hardlink = root / "hardlink"
            hardlink.hardlink_to(credential)
            with self.assertRaisesRegex(UsageFailure, "exactly one link"):
                _credential_lease_from_file(credential, plan=self.plan(), agent_limit=1.0)
            hardlink.unlink()
            with self.assertRaisesRegex(UsageFailure, "exactly one"):
                _credential_lease_from_file(
                    credential,
                    plan=self.plan(("OPENAI_API_KEY", "SECOND_KEY")),
                    agent_limit=1.0,
                )

    def test_production_plan_requires_credential_file_fail_fast(self) -> None:
        with self.assertRaisesRegex(UsageFailure, "require --credential-file"):
            _validate_production_credential_option(self.plan(), None)
        _validate_production_credential_option(self.plan(), Path("/private/credential"))

        anonymous = self.plan(())
        _validate_production_credential_option(anonymous, None)
        with self.assertRaisesRegex(UsageFailure, "forbidden for an anonymous"):
            _validate_production_credential_option(anonymous, Path("/private/credential"))

    def test_adapter_factory_passes_production_objects_to_lume_adapters(self) -> None:
        plan = self.plan()
        lease = mock.sentinel.lease
        environment = mock.sentinel.environment
        harness = mock.sentinel.harness
        with (
            mock.patch(
                "cua_bench_runtime.adapters.lume_macos.load_lume_settings", return_value="settings"
            ),
            mock.patch(
                "cua_bench_runtime.adapters.lume_macos.build_control", return_value="control"
            ),
            mock.patch("cua_bench_runtime.guest_launch.load_guest_launch", return_value="launch"),
            mock.patch(
                "cua_bench_runtime.adapters.lume_macos.LumeMacosEnvironment",
                return_value=environment,
            ) as environment_type,
            mock.patch(
                "cua_bench_runtime.adapters.lume_macos.LumeGuestHarness", return_value=harness
            ) as harness_type,
        ):
            built_environment, built_harness, _observer, _evaluator = adapters(
                "lume-macos",
                Path("/agent"),
                "sha256:agent",
                Path("/evaluator"),
                "sha256:evaluator",
                lume_config_path=Path("/lume.json"),
                guest_launch_path=Path("/launch.json"),
                production_plan=plan,
                credential_lease=lease,  # type: ignore[arg-type]
            )
        self.assertIs(built_environment, environment)
        self.assertIs(built_harness, harness)
        self.assertEqual(environment_type.call_args.kwargs["production_plan"], plan)
        self.assertIs(harness_type.call_args.kwargs["credential_lease"], lease)

    def test_event_log_failures_destroy_unconsumed_production_lease(self) -> None:
        task = {
            "schema_version": "0.3.0",
            "id": "task.production",
            "version": "1",
            "variants": [{"id": "default"}],
            "limits": {"agent_seconds": 30, "evaluator_seconds": 30},
            "evaluator": {"entrypoint": "evaluator", "staging": "agent-inaccessible"},
        }
        system = {
            "schema_version": "0.3.0",
            "id": "apparatus.production",
            "version": "1",
            "harness": {
                "build": {"path": "build"},
                "configuration": {"path": "configuration"},
            },
            "capability_inventory": {
                "skills": {"path": "skills"},
                "tools": {"path": "tools"},
            },
        }
        policy = {
            "schema_version": "0.3.0",
            "id": "policy.production",
            "version": "1",
            "accounting": {"model_price_table": {"path": "prices.json"}},
            "limits": {"wall_time_ms": 30_000},
        }

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            task_path = root / "task.json"
            system_path = root / "system.json"
            policy_path = root / "policy.json"
            agent_path = root / "agent"
            evaluator_path = root / "evaluator"
            lume_config_path = root / "lume.json"
            guest_launch_path = root / "guest-launch.json"
            for path in (
                task_path,
                system_path,
                policy_path,
                agent_path,
                evaluator_path,
                lume_config_path,
                guest_launch_path,
            ):
                path.write_text("{}", encoding="utf-8")
            (root / "prices.json").write_text("{}", encoding="utf-8")
            trial_dir = root / "trial"
            trial_dir.mkdir()

            for fail_during_append in (False, True):
                lease = CredentialLease(
                    provider="openai",
                    harness="codex",
                    credentials={"OPENAI_API_KEY": "secret-value"},
                    expires_at=time.time() + 60,
                    policy_digest=self.plan().policy_digest,
                )
                event_log = mock.Mock()
                if fail_during_append:
                    event_log.append.side_effect = OSError("append failed")
                    event_log_patch = mock.patch(
                        "cua_bench_runtime.engine.EventLog", return_value=event_log
                    )
                else:
                    event_log_patch = mock.patch(
                        "cua_bench_runtime.engine.EventLog", side_effect=OSError("open failed")
                    )

                with (
                    self.subTest(fail_during_append=fail_during_append),
                    mock.patch(
                        "cua_bench_runtime.engine.validate_manifest",
                        side_effect=(task, system, policy),
                    ),
                    mock.patch(
                        "cua_bench_runtime.engine.load_guest_launch",
                        return_value=SimpleNamespace(
                            id="launch", digest="sha256:launch", build_artifacts=()
                        ),
                    ),
                    mock.patch("cua_bench_runtime.engine._system_policy_inputs", return_value=()),
                    mock.patch(
                        "cua_bench_runtime.engine.production_system_plan", return_value=self.plan()
                    ),
                    mock.patch("cua_bench_runtime.engine.ExecutionPolicyController"),
                    mock.patch(
                        "cua_bench_runtime.engine._credential_lease_from_file", return_value=lease
                    ),
                    mock.patch(
                        "cua_bench_runtime.engine.adapter_names",
                        return_value=("agent", "evaluator"),
                    ),
                    mock.patch(
                        "cua_bench_runtime.engine.materialize_trial",
                        return_value=(
                            trial_dir,
                            "sha256:config",
                            "sha256:inputs",
                            agent_path,
                            evaluator_path,
                        ),
                    ),
                    mock.patch(
                        "cua_bench_runtime.engine.adapters",
                        return_value=(
                            mock.Mock(),
                            mock.Mock(),
                            mock.Mock(),
                            mock.Mock(),
                        ),
                    ),
                    event_log_patch,
                ):
                    with self.assertRaisesRegex(OSError, "failed"):
                        run_trial(
                            task_path=task_path,
                            agent_command=agent_path,
                            out=root / "out",
                            environment_name="lume-macos",
                            system_path=system_path,
                            execution_policy_path=policy_path,
                            lume_config_path=lume_config_path,
                            guest_launch_path=guest_launch_path,
                            credential_file_path=root / "credential",
                        )

                self.assertEqual(lease.state, "destroyed")
                self.assertEqual(lease._buffers, {})


if __name__ == "__main__":
    unittest.main()
