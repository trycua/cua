from __future__ import annotations

import hashlib
import json
import tempfile
import time
import unittest
from dataclasses import replace
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
from cua_bench_runtime.adapters.lume_macos import (
    LumeGuestHarness,
    LumeMacosEnvironment,
    _render_guest_launch,
    _validate_support_executable_installation,
)
from cua_bench_runtime.errors import DeadlineExceeded, HarnessFailure
from cua_bench_runtime.credential_lease import CredentialLease
from cua_bench_runtime.guest_launch import RenderedGuestLaunch
from cua_bench_runtime.lume import CommandResult
from cua_bench_runtime.model import EnvironmentHandle, TrialContext
from cua_bench_runtime.signals import InterruptFlag


def plan() -> ProductionSystemPlan:
    provider_allowlist = ("api.openai.com@443",)
    route = ModelRoute(
        route_id="primary",
        role="primary",
        provider="openai",
        model="gpt-5.6-sol",
        snapshot="2026-08-01",
        service_tier="priority",
    )
    return ProductionSystemPlan(
        kind=HarnessKind.CODEX,
        harness=ProductionHarnessSpec(HarnessKind.CODEX, "/usr/local/bin/codex"),
        model_route=route,
        guest_executable="/usr/local/bin/codex",
        guest_executable_sha256="a" * 64,
        credential_environment=("OPENAI_API_KEY",),
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
        tool_names=("click", "file_edit", "shell", "subagent"),
    )


class ProductionLumeTests(unittest.TestCase):
    def test_debug_timeout_persists_only_bounded_content_free_progress(self) -> None:
        secret = "sensitive-prompt-response-and-arguments"
        production_plan = replace(plan(), credential_environment=())
        stdout = "\n".join(
            (
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "type": "mcp_tool_call",
                            "tool": "click",
                            "arguments": secret,
                        },
                    }
                ),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "type": "mcp_tool_call",
                            "tool": secret,
                        },
                    }
                ),
                json.dumps(
                    {
                        "type": "turn.completed",
                        "usage": {
                            "input_tokens": 10,
                            "output_tokens": 4,
                            "cached_input_tokens": 3,
                            "cache_write_input_tokens": 2,
                        },
                        "response": secret,
                    }
                ),
            )
        )
        stderr = secret + "\n"
        recovered = CommandResult(
            None,
            stdout,
            stderr,
            stdout_bytes=len(stdout.encode()),
            stdout_sha256=hashlib.sha256(stdout.encode()).hexdigest(),
            stdout_truncated=False,
            stderr_bytes=len(stderr.encode()),
            stderr_sha256=hashlib.sha256(stderr.encode()).hexdigest(),
            stderr_truncated=False,
        )

        class Control:
            def run_agent(self, *args, **kwargs):
                del args, kwargs
                raise DeadlineExceeded("agent exceeded 30 seconds")

            def agent_output(self, *args, **kwargs):
                del args, kwargs
                return recovered

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            agent = root / "agent"
            agent.write_text("unused", encoding="utf-8")
            launch = RenderedGuestLaunch(
                argv=("/usr/local/bin/codex", "exec", "-"),
                cwd="/Users/Shared/cdb-attempts/trial/workspace",
                environment={"HOME": "/Users/Shared/cdb-attempts/trial/home"},
                stdin_path="/Users/Shared/cdb-attempts/trial/task/brief.md",
                harness_kind="codex",
                executable_sha256="a" * 64,
                credential_names=(),
            )
            environment = object.__new__(LumeMacosEnvironment)
            environment.debug_mode = True
            environment.debug_document = None
            environment.production_plan = production_plan
            environment.attempt = SimpleNamespace(vm_name="cdb-trial")
            environment.control = Control()
            environment.rendered_launch = lambda: launch
            context = TrialContext(
                trial_id="trial",
                task={},
                task_path=root / "task.json",
                config={"debug_mode": True},
                trial_dir=root,
                artifacts=artifacts,
                harness_workspace=root / "harness-workspace",
                emit=mock.Mock(),
                policy=mock.Mock(),
            )
            harness = LumeGuestHarness(
                environment,
                agent,
                "sha256:" + hashlib.sha256(agent.read_bytes()).hexdigest(),
            )

            with self.assertRaises(DeadlineExceeded):
                harness.run(
                    context,
                    EnvironmentHandle("lume", root, {}),
                    InterruptFlag(),
                    30,
                )

            debug_path = artifacts / "agent.debug.json"
            document = json.loads(debug_path.read_text(encoding="utf-8"))
            self.assertEqual(document["termination"]["classification"], "timeout")
            self.assertEqual(
                document["token_totals"],
                {"input": 10, "output": 4, "cache_read": 3, "cache_write": 2},
            )
            self.assertEqual(
                [event.get("tool_name") for event in document["events"]],
                ["click", None, None],
            )
            self.assertFalse(document["raw_output_persisted"])
            self.assertIsNone(document["provider_activity"])
            self.assertNotIn(secret.encode(), debug_path.read_bytes())
            self.assertFalse((artifacts / "agent.stdout").exists())
            self.assertFalse((artifacts / "agent.stderr").exists())

            environment._persist_debug(
                context,
                {
                    "accepted_connections": 2,
                    "rejected_connections": 1,
                    "bytes_guest_to_provider": 100,
                    "bytes_provider_to_guest": 200,
                },
            )
            document = json.loads(debug_path.read_text(encoding="utf-8"))
            self.assertEqual(document["provider_activity"]["accepted_connections"], 2)
            self.assertNotIn(secret.encode(), debug_path.read_bytes())

    def test_companion_guest_identity_is_fail_closed(self) -> None:
        expected = {
            "production_tools": {
                "codex-code-mode-host": {
                    "path": "/usr/local/bin/codex-code-mode-host",
                    "sha256": "sha256:" + "c" * 64,
                    "owner": "root:wheel",
                    "mode": "0755",
                }
            }
        }
        _validate_support_executable_installation(plan(), expected)
        for field, value in (
            ("path", "/private/tmp/host-copy"),
            ("sha256", "sha256:" + "d" * 64),
            ("owner", "cdb-agent:staff"),
            ("mode", "0555"),
        ):
            with self.subTest(field=field):
                changed = json.loads(json.dumps(expected))
                changed["production_tools"]["codex-code-mode-host"][field] = value
                with self.assertRaisesRegex(HarnessFailure, "frozen build"):
                    _validate_support_executable_installation(plan(), changed)

    def test_rendered_launch_is_secret_free_and_uses_native_driver_mcp(self) -> None:
        values = {
            "agent": "/Users/Shared/cdb-attempts/trial/harness/agent/unused",
            "workspace": "/Users/Shared/cdb-attempts/trial/workspace",
            "artifacts": "/Users/Shared/cdb-attempts/trial/artifacts",
            "home": "/Users/Shared/cdb-attempts/trial/home",
            "brief": "/Users/Shared/cdb-attempts/trial/task/brief.md",
            "driver_socket": "/private/var/run/cdb-mediator/trial/driver.sock",
        }
        rendered, configs = _render_guest_launch(values, mock.Mock(), plan())
        self.assertEqual(rendered.argv[0], "/usr/local/bin/codex")
        self.assertEqual(rendered.stdin_path, values["brief"])
        self.assertEqual(rendered.credential_names, ("OPENAI_API_KEY",))
        self.assertEqual(
            rendered.support_executables,
            (("/usr/local/bin/codex-code-mode-host", "c" * 64),),
        )
        self.assertNotIn("OPENAI_API_KEY", rendered.environment)
        self.assertEqual(len(configs), 2)
        config_path, config = configs[0]
        self.assertTrue(config_path.startswith(values["home"] + "/"))
        self.assertIn(values["driver_socket"].encode(), config)
        self.assertNotIn(b"api_key", config.lower())
        self.assertEqual(
            configs[1],
            (
                values["home"] + "/.agents/skills/cua-driver/SKILL.md",
                b"# Cua Driver\n",
            ),
        )

    def test_production_run_consumes_lease_and_persists_no_raw_output(self) -> None:
        secret = "sk-test-never-persist-this"
        production_plan = plan()

        class Control:
            def __init__(self) -> None:
                self.received: dict[str, str] | None = None

            def run_agent(
                self,
                vm_name,
                launch,
                timeout_seconds,
                credential_environment=None,
            ):
                del vm_name, launch, timeout_seconds
                self.received = dict(credential_environment)
                stdout = (
                    json.dumps(
                        {
                            "type": "turn.completed",
                            "model": secret,
                            "usage": {
                                "input_tokens": 4,
                                "output_tokens": 2,
                                "cached_input_tokens": 3,
                                "cache_write_input_tokens": 1,
                            },
                            "prompt": secret,
                            "response": secret,
                        }
                    )
                    + "\n"
                    + "".join(json.dumps({"type": "turn.completed"}) + "\n" for _ in range(1100))
                )
                stderr = secret + "\n"
                return CommandResult(
                    0,
                    stdout,
                    stderr,
                    stdout_bytes=len(stdout.encode()),
                    stdout_sha256=hashlib.sha256(stdout.encode()).hexdigest(),
                    stdout_truncated=False,
                    stderr_bytes=len(stderr.encode()),
                    stderr_sha256=hashlib.sha256(stderr.encode()).hexdigest(),
                    stderr_truncated=False,
                )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            agent_command = root / "agent"
            agent_command.write_text("unused", encoding="utf-8")
            agent_digest = "sha256:" + hashlib.sha256(agent_command.read_bytes()).hexdigest()
            launch = RenderedGuestLaunch(
                argv=("/usr/local/bin/codex", "exec", "-"),
                cwd="/Users/Shared/cdb-attempts/trial/workspace",
                environment={"HOME": "/Users/Shared/cdb-attempts/trial/home"},
                stdin_path="/Users/Shared/cdb-attempts/trial/task/brief.md",
                harness_kind="codex",
                executable_sha256="a" * 64,
                credential_names=("OPENAI_API_KEY",),
            )
            control = Control()
            environment = SimpleNamespace(
                production_plan=production_plan,
                attempt=SimpleNamespace(vm_name="cdb-trial"),
                control=control,
                rendered_launch=lambda: launch,
            )
            lease = CredentialLease(
                provider="openai",
                harness="codex",
                credentials={"OPENAI_API_KEY": secret},
                expires_at=time.time() + 60,
                policy_digest=production_plan.policy_digest,
            )
            policy = mock.Mock()
            context = TrialContext(
                trial_id="trial",
                task={},
                task_path=root / "task.json",
                config={},
                trial_dir=root,
                artifacts=artifacts,
                harness_workspace=root / "harness-workspace",
                emit=mock.Mock(),
                policy=policy,
            )
            harness = LumeGuestHarness(environment, agent_command, agent_digest, lease)
            outcome = harness.run(
                context,
                EnvironmentHandle("lume", root, {}),
                InterruptFlag(),
                30,
            )

            self.assertEqual(control.received, {"OPENAI_API_KEY": secret})
            self.assertEqual(lease.state, "consumed")
            self.assertEqual(
                outcome.artifacts,
                ("agent.output.json", "agent.telemetry.json"),
            )
            self.assertFalse((artifacts / "agent.stdout").exists())
            self.assertFalse((artifacts / "agent.stderr").exists())
            persisted = b"".join(path.read_bytes() for path in artifacts.iterdir())
            self.assertNotIn(secret.encode(), persisted)
            telemetry = json.loads((artifacts / "agent.telemetry.json").read_text(encoding="utf-8"))
            self.assertFalse(telemetry["raw_output_persisted"])
            self.assertEqual(telemetry["events"][0]["input_tokens"], 4)
            self.assertEqual(telemetry["events"][0]["event_type"], "turn.completed")
            self.assertIsNone(telemetry["events"][0]["terminal_failure"])
            self.assertIsNone(telemetry["events"][0]["reported_model"])
            self.assertEqual(telemetry["parsed_event_count"], 1024)
            self.assertEqual(telemetry["skipped_line_count"], 77)
            policy.record_model_call.assert_called_once_with(
                route_id="primary",
                role="primary",
                provider="openai",
                model="gpt-5.6-sol",
                snapshot="2026-08-01",
                service_tier="priority",
                tokens={
                    "input": 4,
                    "output": 2,
                    "cache_read": 3,
                    "cache_write": 1,
                },
                cost_usd=None,
                trust="non_certifying",
                includes_subagents=False,
            )

    def test_anonymous_opencode_run_requires_no_credential_lease(self) -> None:
        production_plan = replace(
            plan(),
            kind=HarnessKind.OPENCODE,
            harness=ProductionHarnessSpec(HarnessKind.OPENCODE, "/usr/local/bin/opencode"),
            model_route=ModelRoute(
                route_id="primary",
                role="primary",
                provider="opencode",
                model="big-pickle",
                snapshot="opencode-big-pickle-2026-08-18",
                service_tier="go",
            ),
            guest_executable="/usr/local/bin/opencode",
            credential_environment=(),
            support_executables=(),
        )

        class Control:
            received: object = mock.sentinel.not_called

            def run_agent(
                self,
                vm_name,
                launch,
                timeout_seconds,
                credential_environment=None,
            ):
                del vm_name, launch, timeout_seconds
                self.received = credential_environment
                stdout = "\n".join(
                    json.dumps(event)
                    for event in (
                        {
                            "type": "step_finish",
                            "part": {
                                "reason": "stop",
                                "tokens": {
                                    "input": 9,
                                    "output": 2,
                                    "cache": {"read": 3, "write": 0},
                                },
                                "cost": 0,
                            },
                        },
                        {
                            "type": "step_finish",
                            "part": {
                                "reason": "stop",
                                "tokens": {
                                    "input": 0,
                                    "output": 0,
                                    "cache": {"read": 0, "write": 0},
                                },
                                "cost": 0,
                            },
                        },
                    )
                )
                return CommandResult(
                    0,
                    stdout,
                    "",
                    stdout_bytes=len(stdout.encode()),
                    stdout_sha256=hashlib.sha256(stdout.encode()).hexdigest(),
                    stdout_truncated=False,
                    stderr_bytes=0,
                    stderr_sha256=hashlib.sha256(b"").hexdigest(),
                    stderr_truncated=False,
                )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            agent_command = root / "agent"
            agent_command.write_text("unused", encoding="utf-8")
            launch = RenderedGuestLaunch(
                argv=("/usr/local/bin/opencode", "--pure", "run"),
                cwd="/Users/Shared/cdb-attempts/trial/workspace",
                environment={"HOME": "/Users/Shared/cdb-attempts/trial/home"},
                stdin_path="/Users/Shared/cdb-attempts/trial/task/brief.md",
                harness_kind="opencode",
                executable_sha256="a" * 64,
                credential_names=(),
            )
            self.assertEqual(launch.document()["credential_names"], [])
            control = Control()
            environment = SimpleNamespace(
                production_plan=production_plan,
                attempt=SimpleNamespace(vm_name="cdb-trial"),
                control=control,
                rendered_launch=lambda: launch,
            )
            policy = mock.Mock()
            context = TrialContext(
                trial_id="trial",
                task={},
                task_path=root / "task.json",
                config={},
                trial_dir=root,
                artifacts=artifacts,
                harness_workspace=root / "harness-workspace",
                emit=mock.Mock(),
                policy=policy,
            )
            outcome = LumeGuestHarness(
                environment,
                agent_command,
                "sha256:" + hashlib.sha256(agent_command.read_bytes()).hexdigest(),
                None,
            ).run(
                context,
                EnvironmentHandle("lume", root, {}),
                InterruptFlag(),
                30,
            )

        self.assertIsNone(control.received)
        self.assertTrue(outcome.completed)
        policy.record_model_call.assert_called_once()
        self.assertEqual(
            policy.record_model_call.call_args.kwargs["tokens"],
            {"input": 9, "output": 2, "cache_read": 3, "cache_write": 0},
        )

    def test_production_run_reports_provider_failure_without_persisting_message(self) -> None:
        production_plan = plan()
        secret = "provider-secret-never-persist"
        result = CommandResult(
            1,
            json.dumps(
                {
                    "type": "error",
                    "message": f"You've hit your usage limit. {secret}",
                }
            )
            + "\n",
            "",
            stdout_bytes=100,
            stdout_sha256="a" * 64,
            stdout_truncated=False,
            stderr_bytes=0,
            stderr_sha256=hashlib.sha256(b"").hexdigest(),
            stderr_truncated=False,
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            artifacts = root / "artifacts"
            artifacts.mkdir()
            agent_command = root / "agent"
            agent_command.write_text("unused", encoding="utf-8")
            control = mock.Mock()
            control.run_agent.return_value = result
            launch = RenderedGuestLaunch(
                argv=("/usr/local/bin/codex", "exec", "-"),
                cwd="/tmp/workspace",
                environment={"HOME": "/tmp/home"},
                stdin_path="/tmp/brief.md",
                harness_kind="codex",
                executable_sha256="a" * 64,
                credential_names=("OPENAI_API_KEY",),
            )
            environment = SimpleNamespace(
                production_plan=production_plan,
                attempt=SimpleNamespace(vm_name="cdb-trial"),
                control=control,
                rendered_launch=lambda: launch,
            )
            lease = CredentialLease(
                provider="openai",
                harness="codex",
                credentials={"OPENAI_API_KEY": "test-secret"},
                expires_at=time.time() + 60,
                policy_digest=production_plan.policy_digest,
            )
            context = TrialContext(
                trial_id="trial",
                task={},
                task_path=root / "task.json",
                config={},
                trial_dir=root,
                artifacts=artifacts,
                harness_workspace=root / "harness-workspace",
                emit=mock.Mock(),
                policy=mock.Mock(),
            )
            outcome = LumeGuestHarness(
                environment,
                agent_command,
                "sha256:" + hashlib.sha256(agent_command.read_bytes()).hexdigest(),
                lease,
            ).run(context, EnvironmentHandle("lume", root, {}), InterruptFlag(), 30)
            self.assertFalse(outcome.completed)
            self.assertEqual(outcome.terminal_failure, "usage_limit")
            process_exit = next(
                call.args[1]
                for call in context.emit.call_args_list
                if call.args[0] == "process_exited"
            )
            self.assertEqual(process_exit["terminal_failure"], "usage_limit")
            persisted = b"".join(path.read_bytes() for path in context.artifacts.iterdir())
            self.assertNotIn(secret.encode(), persisted)

    def test_production_run_does_not_synthesize_zero_usage(self) -> None:
        production_plan = plan()
        result = CommandResult(
            0,
            json.dumps({"type": "turn.completed"}) + "\n",
            "",
            stdout_bytes=27,
            stdout_sha256=hashlib.sha256(
                (json.dumps({"type": "turn.completed"}) + "\n").encode()
            ).hexdigest(),
            stdout_truncated=False,
            stderr_bytes=0,
            stderr_sha256=hashlib.sha256(b"").hexdigest(),
            stderr_truncated=False,
        )
        control = mock.Mock()
        control.run_agent.return_value = result
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            agent = root / "agent"
            agent.write_text("unused", encoding="utf-8")
            launch = RenderedGuestLaunch(
                argv=("/usr/local/bin/codex", "exec", "-"),
                cwd="/tmp/workspace",
                environment={"HOME": "/tmp/home"},
                stdin_path="/tmp/brief.md",
                harness_kind="codex",
                executable_sha256="a" * 64,
                credential_names=("OPENAI_API_KEY",),
            )
            environment = SimpleNamespace(
                production_plan=production_plan,
                attempt=SimpleNamespace(vm_name="cdb-trial"),
                control=control,
                rendered_launch=lambda: launch,
            )
            policy = mock.Mock()
            context = TrialContext(
                trial_id="trial",
                task={},
                task_path=root / "task.json",
                config={},
                trial_dir=root,
                artifacts=artifacts,
                harness_workspace=root / "harness-workspace",
                emit=mock.Mock(),
                policy=policy,
            )
            lease = CredentialLease(
                provider="openai",
                harness="codex",
                credentials={"OPENAI_API_KEY": "test-secret"},
                expires_at=time.time() + 60,
                policy_digest=production_plan.policy_digest,
            )
            harness = LumeGuestHarness(
                environment,
                agent,
                "sha256:" + hashlib.sha256(agent.read_bytes()).hexdigest(),
                lease,
            )
            harness.run(context, EnvironmentHandle("lume", root, {}), InterruptFlag(), 30)
        self.assertIsNone(policy.record_model_call.call_args.kwargs["tokens"])
        self.assertIsNone(policy.record_model_call.call_args.kwargs["cost_usd"])


if __name__ == "__main__":
    unittest.main()
