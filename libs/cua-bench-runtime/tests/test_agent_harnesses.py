from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path, PurePosixPath

from cua_bench_runtime.adapters.agent_harnesses import (
    HarnessKind,
    HarnessRenderContext,
    LaunchContract,
    ModelRoute,
    NativeMcpDriver,
    TelemetryTrust,
    production_harness,
)
from cua_bench_runtime.adapters.agent_harnesses.production import (
    BundledSkillFile,
    ProxyConfiguration,
    canonical_provider_allowlist_sha256,
)
from cua_bench_runtime.process import command_for, run_process
from cua_bench_runtime.signals import InterruptFlag


class ProductionHarnessTests(unittest.TestCase):
    def setUp(self) -> None:
        self.test_root = Path.cwd().resolve() / ".agent-harness-tests"

    def context(self, test_root: Path | None = None) -> HarnessRenderContext:
        test_root = self.test_root if test_root is None else test_root
        return HarnessRenderContext(
            home=test_root / "home",
            workspace=test_root / "workspace",
            artifacts=test_root / "artifacts",
            brief=test_root / "task" / "brief.md",
            model_route=ModelRoute(
                "route.primary",
                "primary",
                "anthropic",
                "claude-sonnet-4-5",
                "2026-08-01",
                "standard",
            ),
            driver=NativeMcpDriver(test_root / "driver.sock"),
        )

    def test_factory_supports_initial_production_harnesses(self) -> None:
        self.assertEqual(production_harness("codex").kind, HarnessKind.CODEX)
        self.assertEqual(
            production_harness("claude-code").executable,
            "claude",
        )
        self.assertEqual(production_harness("opencode").executable, "opencode")
        with self.assertRaisesRegex(ValueError, "unsupported production harness"):
            production_harness("other")
        with self.assertRaisesRegex(ValueError, "non-empty"):
            production_harness("codex", executable="")

    def test_every_launch_owns_model_request_and_uses_isolated_home(self) -> None:
        context = self.context()
        for kind in HarnessKind:
            with self.subTest(harness=kind):
                launch = production_harness(kind).render(context)
                model_index = launch.argv.index("--model")
                expected_model = context.model_route.model
                if kind is HarnessKind.OPENCODE:
                    expected_model = f"{context.model_route.provider}/{context.model_route.model}"
                self.assertEqual(launch.argv[model_index + 1], expected_model)
                self.assertEqual(launch.cwd, context.workspace)
                self.assertEqual(launch.stdin_path, context.brief)
                self.assertEqual(launch.environment["HOME"], str(context.home))
                self.assertTrue(
                    all(item.path.is_relative_to(context.home) for item in launch.config_files)
                )
                self.assertEqual(
                    launch.environment["PATH"],
                    "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                )
                with self.assertRaises(TypeError):
                    launch.environment["INJECTED"] = "value"  # type: ignore[index]

    def test_launches_use_stdin_without_persisting_prompt_content(self) -> None:
        context = self.context()
        secret_brief = "private task text that must not enter launch evidence"
        for kind in HarnessKind:
            with self.subTest(harness=kind):
                launch = production_harness(kind).render(context)
                self.assertNotIn(secret_brief, launch.argv)
                self.assertNotIn(secret_brief, launch.environment.values())
                self.assertNotIn(
                    secret_brief.encode("utf-8"),
                    (config.content for config in launch.config_files),
                )
        codex = production_harness("codex").render(context)
        claude = production_harness("claude-code").render(context)
        opencode = production_harness("opencode").render(context)
        self.assertEqual(codex.argv[-1], "-")
        self.assertEqual(claude.argv[-1], "--print")
        self.assertEqual(opencode.argv[1:3], ("--pure", "run"))
        self.assertEqual(
            opencode.argv[-5:],
            ("--model", "anthropic/claude-sonnet-4-5", "--format", "json", "--auto"),
        )
        self.assertIn("--ephemeral", codex.argv)
        self.assertIn("--skip-git-repo-check", codex.argv)
        self.assertIn("--dangerously-bypass-approvals-and-sandbox", codex.argv)
        self.assertIn("--no-session-persistence", claude.argv)
        self.assertIn("--allow-dangerously-skip-permissions", claude.argv)
        self.assertIn("--permission-mode", claude.argv)

    def test_native_mcp_configuration_preserves_socket_and_schema(self) -> None:
        context = self.context()
        expected = list(context.driver.command)

        codex = production_harness("codex").render(context)
        codex_text = codex.config_files[0].content.decode("utf-8")
        codex_server = tomllib.loads(codex_text)["mcp_servers"]["cua"]
        self.assertEqual(
            [codex_server["command"], *codex_server["args"]],
            expected,
        )

        claude = production_harness("claude-code").render(context)
        claude_config = json.loads(claude.config_files[0].content)
        server = claude_config["mcpServers"]["cua"]
        self.assertEqual([server["command"], *server["args"]], expected)

        opencode = production_harness("opencode").render(context)
        self.assertEqual(
            opencode.config_files[0].path,
            context.home / ".config" / "opencode" / "opencode.json",
        )
        opencode_config = json.loads(opencode.config_files[0].content)
        self.assertEqual(opencode_config["mcp"]["cua"]["command"], expected)

    def test_production_harnesses_receive_exact_closed_proxy_environment(self) -> None:
        context = self.context()
        allowlist = ("api.anthropic.com@443", "statsig.anthropic.com@443")
        proxy = ProxyConfiguration(
            endpoint="192.0.2.10@8443",
            provider_allowlist=allowlist,
            provider_allowlist_sha256=canonical_provider_allowlist_sha256(allowlist),
            implementation_sha256="f" * 64,
        )
        common = {
            "HOME": str(context.home),
            "PATH": "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "HTTPS_PROXY": "http://192.0.2.10:8443",
            "https_proxy": "http://192.0.2.10:8443",
            "NO_PROXY": "localhost,127.0.0.1",
            "no_proxy": "localhost,127.0.0.1",
        }
        codex = production_harness("codex").render(context, proxy=proxy)
        claude = production_harness("claude-code").render(context, proxy=proxy)
        opencode = production_harness("opencode").render(context, proxy=proxy)
        self.assertEqual(
            dict(codex.environment),
            {**common, "CODEX_HOME": str(context.home / ".codex")},
        )
        self.assertEqual(
            dict(claude.environment),
            {**common, "CLAUDE_CONFIG_DIR": str(context.home / ".claude")},
        )
        self.assertEqual(
            json.loads(claude.config_files[0].content)["mcpServers"]["cua"]["args"],
            list(context.driver.command[1:]),
        )
        self.assertEqual(
            dict(opencode.environment),
            {
                **common,
                "XDG_CACHE_HOME": str(context.home / ".cache"),
                "XDG_CONFIG_HOME": str(context.home / ".config"),
                "XDG_DATA_HOME": str(context.home / ".local" / "share"),
            },
        )

    def test_proxy_configuration_rejects_noncanonical_boundaries(self) -> None:
        valid_allowlist = ("api.openai.com@443",)
        digest = canonical_provider_allowlist_sha256(valid_allowlist)
        invalid = (
            ("https://192.0.2.10:8443", valid_allowlist, digest),
            ("127.0.0.1@8443", valid_allowlist, digest),
            ("0.0.0.0@8443", valid_allowlist, digest),
            ("169.254.1.1@8443", valid_allowlist, digest),
            ("224.0.0.1@8443", valid_allowlist, digest),
            ("192.0.2.10@53", valid_allowlist, digest),
            ("192.0.2.10/path@8443", valid_allowlist, digest),
            ("192.0.2.10@08443", valid_allowlist, digest),
            ("192.0.2.10@8443", ("API.openai.com@443",), digest),
            ("192.0.2.10@8443", ("api.openai.com/path@443",), digest),
            ("192.0.2.10@8443", valid_allowlist, "f" * 64),
        )
        for endpoint, allowlist, allowlist_digest in invalid:
            with self.subTest(endpoint=endpoint, allowlist=allowlist):
                with self.assertRaises(ValueError):
                    ProxyConfiguration(endpoint, allowlist, allowlist_digest, "f" * 64)
        with self.assertRaisesRegex(ValueError, "implementation"):
            ProxyConfiguration("192.0.2.10@8443", valid_allowlist, digest, "F" * 64)

    def test_bundled_skill_uses_exact_isolated_harness_paths(self) -> None:
        context = self.context()
        files = (
            BundledSkillFile(PurePosixPath("SKILL.md"), b"# Cua Driver\n"),
            BundledSkillFile(PurePosixPath("references/usage.md"), b"Use MCP.\n"),
        )
        codex = production_harness("codex").render(context, skill_files=files)
        claude = production_harness("claude-code").render(context, skill_files=files)
        self.assertEqual(
            tuple(config.path for config in codex.config_files[1:]),
            (
                context.home / ".agents/skills/cua-driver/SKILL.md",
                context.home / ".agents/skills/cua-driver/references/usage.md",
            ),
        )
        self.assertEqual(codex.environment["CODEX_HOME"], str(context.home / ".codex"))
        self.assertEqual(
            codex.config_files[0].path,
            Path(codex.environment["CODEX_HOME"]) / "config.toml",
        )
        self.assertEqual(
            tomllib.loads(codex.config_files[0].content.decode("utf-8"))["mcp_servers"]["cua"],
            {
                "command": context.driver.command[0],
                "args": list(context.driver.command[1:]),
            },
        )
        self.assertEqual(codex.config_files[1].content, files[0].content)
        self.assertEqual(
            tuple(config.path for config in claude.config_files[1:]),
            (
                context.home / ".claude/skills/cua-driver/SKILL.md",
                context.home / ".claude/skills/cua-driver/references/usage.md",
            ),
        )
        opencode = production_harness("opencode").render(context, skill_files=files)
        self.assertEqual(
            tuple(config.path for config in opencode.config_files[1:]),
            (
                context.home / ".config/opencode/skills/cua-driver/SKILL.md",
                context.home / ".config/opencode/skills/cua-driver/references/usage.md",
            ),
        )

    def test_opencode_1_18_15_fixture_discovers_config_and_stdin(self) -> None:
        fixture = (Path(__file__).parent / "fixtures" / "fake_opencode_1_18_15.py").resolve()
        version = subprocess.run(
            command_for(fixture, ["--version"]),
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.assertEqual(version.stdout.strip(), "1.18.15")

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            context = self.context(root)
            launch = production_harness("opencode", executable=str(fixture)).render(context)
            context.workspace.mkdir(parents=True)
            context.brief.parent.mkdir(parents=True)
            brief = b"synthetic private brief\x00\xff\n"
            context.brief.write_bytes(brief)
            for config in launch.config_files:
                config.path.parent.mkdir(parents=True, exist_ok=True)
                config.path.write_bytes(config.content)

            result = run_process(
                command_for(fixture, launch.argv[1:]),
                cwd=launch.cwd,
                stdout_path=root / "stdout",
                stderr_path=root / "stderr",
                timeout_seconds=5,
                interrupt=InterruptFlag(),
                extra_env=launch.environment,
                stdin_path=launch.stdin_path,
            )
            facts = json.loads(result.stdout.read_bytes())

        self.assertEqual(result.returncode, 0)
        self.assertEqual(facts["version"], "1.18.15")
        self.assertTrue(facts["auto"])
        self.assertTrue(facts["pure"])
        self.assertEqual(facts["config_name"], "opencode.json")
        self.assertEqual(facts["model"], "anthropic/claude-sonnet-4-5")
        self.assertEqual(facts["positional_count"], 0)
        self.assertEqual(facts["stdin_bytes"], len(brief))
        self.assertEqual(facts["stdin_sha256"], hashlib.sha256(brief).hexdigest())
        self.assertEqual(facts["mcp_command"], list(context.driver.command))
        self.assertNotIn(brief.decode("latin-1"), repr(launch))

    def test_model_route_is_mandatory_and_validated(self) -> None:
        valid = ("route", "primary", "provider", "model", "snapshot", "tier")
        for index, value in enumerate(valid):
            invalid = list(valid)
            invalid[index] = ""
            with self.subTest(index=index), self.assertRaises(ValueError):
                ModelRoute(*invalid)
        with self.assertRaisesRegex(ValueError, "unsupported characters"):
            ModelRoute("route", "primary", "provider", "bad model", "snap", "tier")
        with self.assertRaisesRegex(ValueError, "role must be"):
            ModelRoute("route", "coordinator", "provider", "model", "snap", "tier")

    def test_context_rejects_nonisolated_target_and_relative_socket(self) -> None:
        home = self.test_root / "contained-home"
        with self.assertRaisesRegex(ValueError, "workspace must not be inside"):
            HarnessRenderContext(
                home=home,
                workspace=home / "work",
                artifacts=self.test_root / "artifacts",
                brief=self.test_root / "brief",
                model_route=ModelRoute(
                    "route",
                    "primary",
                    "provider",
                    "model",
                    "snapshot",
                    "standard",
                ),
                driver=NativeMcpDriver(self.test_root / "driver.sock"),
            )
        with self.assertRaisesRegex(ValueError, "driver socket must be absolute"):
            NativeMcpDriver(Path("driver.sock"))

    def test_launch_contract_rejects_secret_bearing_environment(self) -> None:
        context = self.context()
        launch = production_harness("codex").render(context)
        with self.assertRaisesRegex(ValueError, "secret-bearing"):
            LaunchContract(
                argv=launch.argv,
                cwd=launch.cwd,
                environment={"API_TOKEN": "do-not-persist"},
                stdin_path=launch.stdin_path,
                config_files=launch.config_files,
                requested_route=launch.requested_route,
                driver=launch.driver,
            )
        with self.assertRaisesRegex(ValueError, "secret-bearing launch argument"):
            LaunchContract(
                argv=("agent", "--api-key", "do-not-persist"),
                cwd=launch.cwd,
                environment={},
                stdin_path=launch.stdin_path,
                config_files=launch.config_files,
                requested_route=launch.requested_route,
                driver=launch.driver,
            )

    def test_telemetry_normalization_drops_content_and_marks_boundaries(self) -> None:
        route = ModelRoute(
            "route-1",
            "primary",
            "provider-1",
            "model-1",
            "snapshot-1",
            "standard",
        )
        event = {
            "type": "result",
            "message": {
                "model": "model-1",
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 5,
                    "cache_read_input_tokens": 4,
                    "cache_creation_input_tokens": 2,
                },
                "content": "private response",
            },
            "prompt": "private prompt",
            "tool_arguments": {"text": "private"},
        }
        normalized = production_harness("claude-code").normalize_telemetry(event, route)
        self.assertEqual(normalized.requested_model, "model-1")
        self.assertEqual(normalized.requested_provider, "provider-1")
        self.assertEqual(normalized.requested_snapshot, "snapshot-1")
        self.assertEqual(normalized.requested_model_trust, TelemetryTrust.BENCHMARK_OWNED)
        self.assertEqual(normalized.reported_model, "model-1")
        self.assertEqual(normalized.reported_model_trust, TelemetryTrust.HARNESS_REPORTED)
        self.assertEqual((normalized.input_tokens, normalized.output_tokens), (12, 5))
        self.assertEqual((normalized.cache_read_tokens, normalized.cache_write_tokens), (4, 2))
        self.assertIs(normalized.includes_subagents, False)
        self.assertTrue(normalized.usage_is_cumulative_total)
        self.assertEqual(normalized.usage_trust, TelemetryTrust.HARNESS_REPORTED)
        self.assertIsNone(normalized.served_model)
        self.assertEqual(normalized.served_model_trust, TelemetryTrust.UNAVAILABLE)
        self.assertIsNone(normalized.terminal_failure)
        self.assertNotIn("private", repr(normalized))

    def test_terminal_provider_failures_are_content_free_and_closed(self) -> None:
        route = ModelRoute("route", "primary", "provider", "model", "snap", "tier")
        cases = (
            (
                "claude-code",
                {
                    "type": "result",
                    "is_error": True,
                    "terminal_reason": "api_error",
                    "result": "Not logged in · Please run /login secret-token",
                },
                "authentication_unavailable",
            ),
            (
                "codex",
                {
                    "type": "error",
                    "message": "You've hit your usage limit. purchase more credits secret-token",
                },
                "usage_limit",
            ),
            (
                "claude-code",
                {"type": "result", "is_error": True, "api_error_status": 429},
                "rate_limited",
            ),
            (
                "codex",
                {
                    "type": "turn.failed",
                    "error": {"message": "opaque failure secret-token"},
                },
                "provider_error",
            ),
        )
        for harness, event, expected in cases:
            with self.subTest(harness=harness, expected=expected):
                normalized = production_harness(harness).normalize_telemetry(event, route)
                self.assertEqual(normalized.terminal_failure, expected)
                self.assertNotIn("secret-token", repr(normalized))

        success = production_harness("claude-code").normalize_telemetry(
            {
                "type": "result",
                "is_error": False,
                "result": "Not logged in is model-authored text secret-token",
            },
            route,
        )
        self.assertIsNone(success.terminal_failure)
        self.assertNotIn("secret-token", repr(success))

    def test_unknown_or_malformed_telemetry_remains_unavailable(self) -> None:
        normalized = production_harness("opencode").normalize_telemetry(
            {"usage": {"input_tokens": -1, "output_tokens": True}, "text": "drop me"},
            ModelRoute("route", "primary", "provider", "model", "snap", "tier"),
        )
        self.assertEqual(normalized.event_type, "unknown")
        self.assertIsNone(normalized.reported_model)
        self.assertIsNone(normalized.input_tokens)
        self.assertEqual(normalized.usage_trust, TelemetryTrust.UNAVAILABLE)

    def test_opencode_1_18_15_step_finish_reports_cumulative_usage(self) -> None:
        normalized = production_harness("opencode").normalize_telemetry(
            {
                "type": "step_finish",
                "part": {
                    "reason": "stop",
                    "tokens": {
                        "total": 10466,
                        "input": 8659,
                        "output": 15,
                        "reasoning": 0,
                        "cache": {"write": 0, "read": 1792},
                    },
                    "cost": 0,
                },
            },
            ModelRoute("route", "primary", "opencode", "big-pickle", "snap", "tier"),
        )
        self.assertEqual(normalized.event_type, "step_finish")
        self.assertTrue(normalized.usage_is_cumulative_total)
        self.assertEqual(normalized.input_tokens, 8659)
        self.assertEqual(normalized.output_tokens, 15)
        self.assertEqual(normalized.cache_read_tokens, 1792)
        self.assertEqual(normalized.cache_write_tokens, 0)
        self.assertEqual(normalized.usage_trust, TelemetryTrust.HARNESS_REPORTED)

    def test_nonterminal_usage_is_not_misreported_as_a_cumulative_total(self) -> None:
        normalized = production_harness("claude-code").normalize_telemetry(
            {
                "type": "message",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 3,
                    "cache_read_input_tokens": 2,
                    "cache_creation_input_tokens": 1,
                },
            },
            ModelRoute("route", "primary", "provider", "model", "snap", "tier"),
        )
        self.assertFalse(normalized.usage_is_cumulative_total)
        self.assertIsNone(normalized.input_tokens)
        self.assertIsNone(normalized.output_tokens)
        self.assertIsNone(normalized.cache_read_tokens)
        self.assertIsNone(normalized.cache_write_tokens)
        self.assertIsNone(normalized.includes_subagents)
        self.assertEqual(normalized.usage_trust, TelemetryTrust.UNAVAILABLE)

    def test_missing_cache_fields_remain_missing_on_a_terminal_total(self) -> None:
        normalized = production_harness("codex").normalize_telemetry(
            {
                "type": "turn.completed",
                "usage": {"input_tokens": 10, "output_tokens": 3},
            },
            ModelRoute("route", "primary", "provider", "model", "snap", "tier"),
        )
        self.assertTrue(normalized.usage_is_cumulative_total)
        self.assertEqual((normalized.input_tokens, normalized.output_tokens), (10, 3))
        self.assertIsNone(normalized.cache_read_tokens)
        self.assertIsNone(normalized.cache_write_tokens)
        self.assertIs(normalized.includes_subagents, False)

    def test_telemetry_normalization_never_persists_untrusted_strings(self) -> None:
        secret = "sk-test-never-persist-this"
        normalized = production_harness("codex").normalize_telemetry(
            {
                "type": secret,
                "event": "result",
                "model": secret,
                "message": {"model": secret},
                "usage": {
                    "input_tokens": 1_000_000_000_001,
                    "output_tokens": 3,
                },
            },
            ModelRoute("route", "primary", "openai", "model", "snap", "tier"),
        )
        self.assertEqual(normalized.event_type, "unknown")
        self.assertIsNone(normalized.reported_model)
        self.assertEqual(normalized.reported_model_trust, TelemetryTrust.UNAVAILABLE)
        self.assertIsNone(normalized.input_tokens)
        self.assertIsNone(normalized.output_tokens)
        self.assertFalse(normalized.usage_is_cumulative_total)
        self.assertNotIn(secret, repr(normalized))


if __name__ == "__main__":
    unittest.main()
