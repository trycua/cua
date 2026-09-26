from __future__ import annotations

import copy
import hashlib
import json
import tempfile
import unittest
from dataclasses import FrozenInstanceError
from pathlib import Path

from cua_bench_runtime.adapters.agent_harnesses.production import (
    HarnessKind,
    ModelRoute,
    canonical_provider_allowlist_sha256,
)
from cua_bench_runtime.adapters.agent_harnesses.system import (
    PRODUCTION_ADAPTER,
    ProductionSystemError,
    production_system_plan,
)


class ProductionSystemPlanTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.build_path = root / "harness-build.json"
        self.configuration_path = root / "harness-config.json"
        self.skill_inventory_path = root / "skill-inventory.json"
        self.tool_inventory_path = root / "tools.json"
        self.build = {
            "schema_version": 1,
            "kind": "codex",
            "harness_id": "codex-cli",
            "version": "1.2.3",
            "guest_executable": "/usr/local/bin/codex",
            "guest_executable_sha256": "a" * 64,
            "support_executables": [
                {
                    "path": "/usr/local/bin/codex-code-mode-host",
                    "sha256": "c" * 64,
                }
            ],
        }
        provider_allowlist = ["api.openai.com@443"]
        self.configuration = {
            "schema_version": 1,
            "adapter": PRODUCTION_ADAPTER,
            "credential_environment": ["CDB_CODEX_AUTH_JSON"],
            "proxy_endpoint": "192.0.2.10@8443",
            "provider_allowlist": provider_allowlist,
            "provider_allowlist_sha256": canonical_provider_allowlist_sha256(provider_allowlist),
            "proxy_implementation_sha256": "9" * 64,
        }
        skill_content = "# Cua Driver\n\nUse the native cua MCP server.\n"
        self.skill_inventory = {
            "schema_version": 1,
            "generator": {"name": "cua-skill-freezer", "version": "1.0.0"},
            "skills": [
                {
                    "id": "cua-driver",
                    "version": "0.20.0",
                    "format": "agents-skill-v1",
                    "source": {
                        "repository": "https://github.com/trycua/cua",
                        "revision": "cua-driver-rs-v0.20.0",
                        "path": "libs/cua-driver/skills/cua-driver",
                    },
                    "files": [
                        {
                            "path": "SKILL.md",
                            "sha256": hashlib.sha256(skill_content.encode()).hexdigest(),
                            "content": skill_content,
                        }
                    ],
                }
            ],
        }
        daemon_tools = [
            {
                "name": "click",
                "description": "Click an element.",
                "input_schema": {"type": "object"},
            }
        ]
        mcp_tools = [
            {
                "name": "click",
                "description": "Click an element.",
                "inputSchema": {"type": "object"},
            }
        ]
        self.daemon_tools_sha256 = hashlib.sha256(
            json.dumps(daemon_tools, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        self.mcp_tools_sha256 = hashlib.sha256(
            json.dumps(mcp_tools, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        self.daemon_tools_list_envelope = {
            "schema_version": "1",
            "capability_version": "1",
            "enforcement_adapters": [{"id": "accessibility", "available": True}],
            "tool_observation_owner": "daemon",
            "tools": daemon_tools,
        }
        self.mcp_tools_list_envelope = {
            "schema_version": "1",
            "capability_version": "1",
            "tools": mcp_tools,
        }
        self.daemon_tools_list_envelope_sha256 = hashlib.sha256(
            json.dumps(
                self.daemon_tools_list_envelope,
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
        self.mcp_tools_list_envelope_sha256 = hashlib.sha256(
            json.dumps(
                self.mcp_tools_list_envelope,
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
        self.tool_inventory = {
            "schema_version": 1,
            "generator": {"name": "cua-tool-freezer", "version": "1.0.0"},
            "driver": {
                "id": "trycua.cua-driver",
                "version": "0.20.0",
                "source": {
                    "repository": "https://github.com/trycua/cua",
                    "revision": "cua-driver-rs-v0.20.0",
                    "path": "libs/cua-driver",
                },
                "mcp_tools_list": {"tools": mcp_tools},
                "daemon_tools_list": {"tools": daemon_tools},
                "mcp_tools_list_envelope": self.mcp_tools_list_envelope,
                "daemon_tools_list_envelope": self.daemon_tools_list_envelope,
                "mcp_tools_list_envelope_sha256": self.mcp_tools_list_envelope_sha256,
                "daemon_tools_list_envelope_sha256": self.daemon_tools_list_envelope_sha256,
                "mcp_tool_schemas_sha256": self.mcp_tools_sha256,
                "daemon_tool_schemas_sha256": self.daemon_tools_sha256,
                "digest_algorithm": "sha256-rfc8785",
            },
            "native_harness_tools": ["shell", "file_edit", "subagent"],
        }

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def _bytes(value: object) -> bytes:
        return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()

    def _write(self) -> dict[str, object]:
        build_bytes = self._bytes(self.build)
        configuration_bytes = self._bytes(self.configuration)
        self.build_path.write_bytes(build_bytes)
        self.configuration_path.write_bytes(configuration_bytes)
        skill_inventory_bytes = self._bytes(self.skill_inventory)
        self.skill_inventory_path.write_bytes(skill_inventory_bytes)
        tool_inventory_bytes = self._bytes(self.tool_inventory)
        self.tool_inventory_path.write_bytes(tool_inventory_bytes)
        configuration_sha256 = hashlib.sha256(configuration_bytes).hexdigest()
        return {
            "schema_version": "0.3.0",
            "id": "system.codex.production",
            "version": "1.0.0",
            "driver": {
                "candidate": {
                    "id": "trycua.cua-driver",
                    "version": "0.20.0",
                    "manifest_sha256": "b" * 64,
                },
                "profile": {"id": "native-bundle", "version": "1.0.0"},
                "tool_contract_sha256": self.mcp_tools_sha256,
                "skill_mode": "bundled",
            },
            "capability_inventory": {
                "tools": {
                    "path": "tools.json",
                    "sha256": hashlib.sha256(tool_inventory_bytes).hexdigest(),
                },
                "skills": {
                    "path": "skill-inventory.json",
                    "sha256": hashlib.sha256(skill_inventory_bytes).hexdigest(),
                },
                "native_harness_capabilities_enabled": True,
            },
            "harness": {
                "id": "codex-cli",
                "version": "1.2.3",
                "build": {
                    "path": "freeze-inputs/harness-build.json",
                    "sha256": hashlib.sha256(build_bytes).hexdigest(),
                },
                "configuration": {
                    "path": "freeze-inputs/harness-config.json",
                    "sha256": configuration_sha256,
                },
            },
            "model_routing": {
                "routes": [
                    {
                        "id": "route.primary",
                        "role": "primary",
                        "provider": "openai",
                        "model": "gpt-5.6",
                        "snapshot": "2026-08-17",
                        "service_tier": "priority",
                        "configuration_sha256": configuration_sha256,
                    }
                ],
                "undeclared_fallback_action": "comparison_ineligible",
            },
        }

    def _plan(self, manifest: dict[str, object] | None = None):
        if manifest is None:
            manifest = self._write()
        return production_system_plan(
            manifest,
            build_path=self.build_path,
            configuration_path=self.configuration_path,
            skill_inventory_path=self.skill_inventory_path,
            tool_inventory_path=self.tool_inventory_path,
        )

    def test_builds_frozen_secret_free_plan_with_stable_digest(self) -> None:
        manifest = self._write()
        plan = self._plan(manifest)
        duplicate = self._plan(copy.deepcopy(manifest))

        self.assertIsNotNone(plan)
        assert plan is not None and duplicate is not None
        self.assertEqual(plan.kind, HarnessKind.CODEX)
        self.assertEqual(plan.harness.kind, HarnessKind.CODEX)
        self.assertEqual(plan.harness.executable, "/usr/local/bin/codex")
        self.assertEqual(plan.guest_executable, "/usr/local/bin/codex")
        self.assertEqual(plan.guest_executable_sha256, "a" * 64)
        self.assertEqual(
            plan.support_executables,
            (("/usr/local/bin/codex-code-mode-host", "c" * 64),),
        )
        self.assertEqual(plan.credential_environment, ("CDB_CODEX_AUTH_JSON",))
        self.assertEqual(plan.proxy.endpoint, "192.0.2.10@8443")
        self.assertEqual(plan.proxy.url, "http://192.0.2.10:8443")
        self.assertEqual(plan.proxy.provider_allowlist, ("api.openai.com@443",))
        self.assertEqual(
            plan.model_route,
            ModelRoute(
                "route.primary",
                "primary",
                "openai",
                "gpt-5.6",
                "2026-08-17",
                "priority",
            ),
        )
        self.assertRegex(plan.policy_digest, r"^sha256:[0-9a-f]{64}$")
        self.assertEqual(
            plan.daemon_tools_list_envelope_sha256,
            self.daemon_tools_list_envelope_sha256,
        )
        self.assertEqual(
            plan.mcp_tools_list_envelope_sha256,
            self.mcp_tools_list_envelope_sha256,
        )
        self.assertEqual(plan.tool_names, ("click", "file_edit", "shell", "subagent"))
        self.assertEqual(plan.policy_digest, duplicate.policy_digest)
        self.assertNotIn("secret", repr(plan).lower())
        with self.assertRaises(FrozenInstanceError):
            plan.guest_executable = "/usr/local/bin/other"  # type: ignore[misc]

    def test_non_production_config_returns_none_without_reading_build(self) -> None:
        for configuration in (
            {"schema_version": 1},
            {"adapter": "some-other-adapter", "unexpected": True},
        ):
            with self.subTest(configuration=configuration):
                self.configuration_path.write_bytes(self._bytes(configuration))
                self.build_path.unlink(missing_ok=True)
                self.assertIsNone(
                    production_system_plan(
                        {},
                        build_path=self.build_path,
                        configuration_path=self.configuration_path,
                        skill_inventory_path=self.skill_inventory_path,
                        tool_inventory_path=self.tool_inventory_path,
                    )
                )

    def test_codex_build_requires_exact_digest_pinned_companion(self) -> None:
        for support_executables in (
            [],
            [{"path": "/usr/local/bin/other", "sha256": "c" * 64}],
            [
                {
                    "path": "/usr/local/bin/codex-code-mode-host",
                    "sha256": "C" * 64,
                }
            ],
            [
                {
                    "path": "/usr/local/bin/codex-code-mode-host",
                    "sha256": "c" * 64,
                    "source": "/private/host/path",
                }
            ],
        ):
            with self.subTest(support_executables=support_executables):
                self.build["support_executables"] = support_executables
                with self.assertRaises(ProductionSystemError):
                    self._plan(self._write())

    def test_all_supported_harness_provider_credential_bindings(self) -> None:
        cases = (
            ("codex", "openai", "/usr/local/bin/codex", ["CDB_CODEX_AUTH_JSON"]),
            (
                "claude-code",
                "anthropic",
                "/usr/local/bin/claude",
                ["CLAUDE_CODE_OAUTH_TOKEN"],
            ),
            ("opencode", "opencode", "/usr/local/bin/opencode", []),
        )
        for kind, provider, executable, credentials in cases:
            with self.subTest(kind=kind, provider=provider):
                self.build["kind"] = kind
                self.build["guest_executable"] = executable
                self.build["version"] = "2.1.233" if kind == "claude-code" else "1.2.3"
                if kind == "claude-code":
                    self.build.pop("support_executables")
                self.configuration["credential_environment"] = credentials
                manifest = self._write()
                if kind == "claude-code":
                    manifest["harness"]["version"] = "2.1.233"  # type: ignore[index]
                manifest["model_routing"]["routes"][0]["provider"] = provider  # type: ignore[index]
                plan = self._plan(manifest)
                assert plan is not None
                self.assertEqual(plan.kind.value, kind)
                self.assertEqual(plan.credential_environment, tuple(sorted(credentials)))

    def test_rejects_noncertifying_credential_transports(self) -> None:
        cases = (
            ("codex", "openai", "OPENAI_API_KEY", "exactly one credential"),
            ("opencode", "openai", "OPENAI_API_KEY", "not supported"),
            ("opencode", "anthropic", "ANTHROPIC_API_KEY", "not supported"),
        )
        for kind, provider, credential, message in cases:
            with self.subTest(kind=kind, provider=provider):
                self.build.update(
                    kind=kind,
                    version="1.2.3",
                    guest_executable=f"/usr/local/bin/{kind}",
                    support_executables=(
                        self.build["support_executables"] if kind == "codex" else []
                    ),
                )
                self.configuration["credential_environment"] = [credential]
                manifest = self._write()
                manifest["model_routing"]["routes"][0]["provider"] = provider  # type: ignore[index]
                with self.assertRaisesRegex(ProductionSystemError, message):
                    self._plan(manifest)

    def test_claude_code_requires_pinned_binary_and_oauth_fd_credential(self) -> None:
        self.build.update(
            kind="claude-code",
            version="2.1.233",
            guest_executable="/usr/local/bin/claude",
        )
        self.build.pop("support_executables")
        self.configuration["credential_environment"] = ["CLAUDE_CODE_OAUTH_TOKEN"]
        manifest = self._write()
        manifest["harness"]["version"] = "2.1.233"  # type: ignore[index]
        manifest["model_routing"]["routes"][0]["provider"] = "anthropic"  # type: ignore[index]
        self.assertIsNotNone(self._plan(manifest))

        for label, mutation in (
            ("version", lambda: self.build.update(version="2.1.234")),
            ("executable", lambda: self.build.update(guest_executable="/usr/local/bin/not-claude")),
            (
                "api-key",
                lambda: self.configuration.update(credential_environment=["ANTHROPIC_API_KEY"]),
            ),
        ):
            with self.subTest(label=label):
                self.build.update(version="2.1.233", guest_executable="/usr/local/bin/claude")
                self.configuration["credential_environment"] = ["CLAUDE_CODE_OAUTH_TOKEN"]
                mutation()
                invalid_manifest = self._write()
                invalid_manifest["harness"]["version"] = self.build["version"]  # type: ignore[index]
                invalid_manifest["model_routing"]["routes"][0]["provider"] = "anthropic"  # type: ignore[index]
                with self.assertRaises(ProductionSystemError):
                    self._plan(invalid_manifest)

    def test_rejects_harness_provider_credential_mismatches(self) -> None:
        cases = (
            ("codex", "anthropic", ["ANTHROPIC_API_KEY"]),
            ("claude-code", "openai", ["OPENAI_API_KEY"]),
            ("opencode", "openai", ["ANTHROPIC_API_KEY"]),
            ("opencode", "anthropic", ["OPENAI_API_KEY"]),
            ("codex", "openai", ["OPENAI_API_KEY", "CDB_CODEX_AUTH_JSON"]),
        )
        for kind, provider, credentials in cases:
            with self.subTest(kind=kind, provider=provider, credentials=credentials):
                self.build["kind"] = kind
                self.build["guest_executable"] = f"/usr/local/bin/{kind}"
                self.configuration["credential_environment"] = credentials
                manifest = self._write()
                manifest["model_routing"]["routes"][0]["provider"] = provider  # type: ignore[index]
                with self.assertRaises(ProductionSystemError):
                    self._plan(manifest)

    def test_marker_bearing_config_has_a_closed_strict_schema(self) -> None:
        baseline = copy.deepcopy(self.configuration)
        invalid = (
            ("missing", lambda value: value.pop("schema_version")),
            ("extra", lambda value: value.update(extra=True)),
            ("bool-version", lambda value: value.update(schema_version=True)),
            (
                "empty-credentials",
                lambda value: value.update(credential_environment=[]),
            ),
            (
                "non-string-credential",
                lambda value: value.update(credential_environment=[7]),
            ),
            (
                "duplicate-credential",
                lambda value: value.update(
                    credential_environment=["OPENAI_API_KEY", "OPENAI_API_KEY"]
                ),
            ),
            (
                "secret-value",
                lambda value: value.update(credential_environment=["sk-secret-value"]),
            ),
            (
                "wrong-kind-credential",
                lambda value: value.update(credential_environment=["ANTHROPIC_API_KEY"]),
            ),
        )
        for name, mutate in invalid:
            with self.subTest(name=name):
                self.configuration = copy.deepcopy(baseline)
                mutate(self.configuration)
                manifest = self._write()
                with self.assertRaises(ProductionSystemError):
                    self._plan(manifest)

    def test_production_proxy_contract_is_canonical_and_fail_closed(self) -> None:
        baseline = copy.deepcopy(self.configuration)
        cases = (
            ("missing-endpoint", lambda value: value.pop("proxy_endpoint")),
            (
                "url-endpoint",
                lambda value: value.update(proxy_endpoint="http://192.0.2.10:8443"),
            ),
            (
                "loopback",
                lambda value: value.update(proxy_endpoint="127.0.0.1@8443"),
            ),
            (
                "unspecified",
                lambda value: value.update(proxy_endpoint="0.0.0.0@8443"),
            ),
            (
                "name-service-port",
                lambda value: value.update(proxy_endpoint="192.0.2.10@53"),
            ),
            (
                "hostname-endpoint",
                lambda value: value.update(proxy_endpoint="proxy.example@8443"),
            ),
            (
                "endpoint-userinfo",
                lambda value: value.update(proxy_endpoint="user@192.0.2.10@8443"),
            ),
            (
                "endpoint-path",
                lambda value: value.update(proxy_endpoint="192.0.2.10/path@8443"),
            ),
            (
                "endpoint-leading-zero-port",
                lambda value: value.update(proxy_endpoint="192.0.2.10@08443"),
            ),
            (
                "provider-path",
                lambda value: value.update(provider_allowlist=["api.openai.com/path@443"]),
            ),
            (
                "provider-ip",
                lambda value: value.update(provider_allowlist=["198.51.100.4@443"]),
            ),
            (
                "provider-userinfo",
                lambda value: value.update(provider_allowlist=["user@api.openai.com@443"]),
            ),
            (
                "provider-case",
                lambda value: value.update(provider_allowlist=["API.openai.com@443"]),
            ),
            (
                "provider-duplicate",
                lambda value: value.update(
                    provider_allowlist=[
                        "api.openai.com@443",
                        "api.openai.com@443",
                    ]
                ),
            ),
            (
                "provider-unsorted",
                lambda value: value.update(provider_allowlist=["b.example@443", "a.example@443"]),
            ),
            (
                "digest-mismatch",
                lambda value: value.update(provider_allowlist_sha256="f" * 64),
            ),
            (
                "implementation-digest-invalid",
                lambda value: value.update(proxy_implementation_sha256="F" * 64),
            ),
        )
        for name, mutate in cases:
            with self.subTest(name=name):
                self.configuration = copy.deepcopy(baseline)
                mutate(self.configuration)
                manifest = self._write()
                with self.assertRaises(ProductionSystemError):
                    self._plan(manifest)

    def test_proxy_metadata_changes_policy_digest(self) -> None:
        baseline = self._plan(self._write())
        assert baseline is not None
        self.configuration["proxy_endpoint"] = "192.0.2.11@8443"
        changed = self._plan(self._write())
        assert changed is not None
        self.assertNotEqual(baseline.policy_digest, changed.policy_digest)

        self.configuration["proxy_implementation_sha256"] = "8" * 64
        implementation_changed = self._plan(self._write())
        assert implementation_changed is not None
        self.assertNotEqual(changed.policy_digest, implementation_changed.policy_digest)

    def test_build_has_a_closed_strict_schema(self) -> None:
        baseline = copy.deepcopy(self.build)
        invalid = (
            ("missing", lambda value: value.pop("version")),
            ("extra", lambda value: value.update(extra=True)),
            ("bool-version", lambda value: value.update(schema_version=True)),
            ("unsupported-kind", lambda value: value.update(kind="hermes")),
            (
                "relative-executable",
                lambda value: value.update(guest_executable="bin/codex"),
            ),
            (
                "outside-root",
                lambda value: value.update(guest_executable="/usr/bin/codex"),
            ),
            (
                "homebrew-root",
                lambda value: value.update(guest_executable="/opt/homebrew/bin/codex"),
            ),
            (
                "non-normal-executable",
                lambda value: value.update(guest_executable="/usr/local/bin/../codex"),
            ),
            (
                "uppercase-digest",
                lambda value: value.update(guest_executable_sha256="A" * 64),
            ),
            (
                "short-digest",
                lambda value: value.update(guest_executable_sha256="a" * 63),
            ),
        )
        for name, mutate in invalid:
            with self.subTest(name=name):
                self.build = copy.deepcopy(baseline)
                mutate(self.build)
                manifest = self._write()
                with self.assertRaises(ProductionSystemError):
                    self._plan(manifest)

    def test_rejects_artifact_and_manifest_mismatches(self) -> None:
        mutations = (
            (
                "manifest-version",
                lambda manifest: manifest.update(schema_version="0.2.0"),
            ),
            (
                "build-pin",
                lambda manifest: manifest["harness"]["build"].update(sha256="b" * 64),
            ),
            (
                "configuration-pin",
                lambda manifest: manifest["harness"]["configuration"].update(sha256="b" * 64),
            ),
            (
                "harness-id",
                lambda manifest: manifest["harness"].update(  # type: ignore[union-attr]
                    id="other"
                ),
            ),
            (
                "harness-version",
                lambda manifest: manifest["harness"].update(  # type: ignore[union-attr]
                    version="9.9.9"
                ),
            ),
            (
                "route-configuration",
                lambda manifest: manifest["model_routing"]["routes"][0].update(
                    configuration_sha256="c" * 64
                ),
            ),
        )
        for name, mutate in mutations:
            with self.subTest(name=name):
                manifest = self._write()
                mutate(manifest)
                with self.assertRaises(ProductionSystemError):
                    self._plan(manifest)

    def test_requires_exactly_one_well_formed_primary_route(self) -> None:
        manifest = self._write()
        route = manifest["model_routing"]["routes"][0]  # type: ignore[index]
        cases = (
            [],
            [{**route, "role": "fallback"}],
            [route, copy.deepcopy(route)],
            [{**route, "model": "contains space"}],
        )
        for routes in cases:
            with self.subTest(routes=routes):
                candidate = copy.deepcopy(manifest)
                candidate["model_routing"]["routes"] = routes  # type: ignore[index]
                with self.assertRaises(ProductionSystemError):
                    self._plan(candidate)

    def test_rejects_invalid_and_duplicate_json(self) -> None:
        self.configuration_path.write_text("{not json", encoding="utf-8")
        with self.assertRaises(ProductionSystemError):
            self._plan({})

        duplicate = (
            '{"schema_version":1,"adapter":"%s","adapter":"%s",'
            '"credential_environment":["CDB_CODEX_AUTH_JSON"]}'
            % (PRODUCTION_ADAPTER, PRODUCTION_ADAPTER)
        )
        self.configuration_path.write_text(duplicate, encoding="utf-8")
        with self.assertRaisesRegex(ProductionSystemError, "duplicate"):
            self._plan({})

    def test_skill_bundle_has_closed_manifest_bound_schema(self) -> None:
        baseline = copy.deepcopy(self.skill_inventory)

        def file() -> dict[str, object]:
            return self.skill_inventory["skills"][0]["files"][0]  # type: ignore[index,return-value]

        cases = (
            ("traversal", lambda: file().update(path="../SKILL.md"), "unsafe"),
            ("dot", lambda: file().update(path="."), "unsafe"),
            ("empty-part", lambda: file().update(path="refs//usage.md"), "unsafe"),
            ("control", lambda: file().update(path="refs/bad\x00.md"), "unsafe"),
            (
                "duplicate",
                lambda: self.skill_inventory["skills"][0]["files"].append(  # type: ignore[index,union-attr]
                    copy.deepcopy(file())
                ),
                "duplicated",
            ),
            (
                "digest",
                lambda: file().update(sha256="0" * 64),
                "sha256 mismatch",
            ),
            (
                "oversized",
                lambda: file().update(content="x" * (1024 * 1024 + 1)),
                "size limit",
            ),
            (
                "wrong-id",
                lambda: self.skill_inventory["skills"][0].update(id="other"),  # type: ignore[index,union-attr]
                "id must be cua-driver",
            ),
            (
                "wrong-version",
                lambda: self.skill_inventory["skills"][0].update(version="9.9.9"),  # type: ignore[index,union-attr]
                "version does not match driver candidate",
            ),
            (
                "omitted",
                lambda: self.skill_inventory.update(skills=[]),
                "exactly one skill",
            ),
        )
        for name, mutate, message in cases:
            with self.subTest(name=name):
                self.skill_inventory = copy.deepcopy(baseline)
                mutate()
                manifest = self._write()
                with self.assertRaisesRegex(ProductionSystemError, message):
                    self._plan(manifest)

    def test_skill_inventory_artifact_digest_is_manifest_bound(self) -> None:
        manifest = self._write()
        manifest["capability_inventory"]["skills"]["sha256"] = "0" * 64  # type: ignore[index]
        with self.assertRaisesRegex(ProductionSystemError, "manifest sha256"):
            self._plan(manifest)

    def test_tool_inventory_is_strict_bounded_and_digest_bound(self) -> None:
        baseline = copy.deepcopy(self.tool_inventory)

        def daemon_tools() -> list[dict[str, object]]:
            return self.tool_inventory["driver"]["daemon_tools_list"]["tools"]  # type: ignore[index,return-value]

        def deeply_nested_schema() -> dict[str, object]:
            value: dict[str, object] = {"type": "object"}
            for _ in range(40):
                value = {"properties": value}
            return value

        cases = (
            (
                "duplicate",
                lambda: daemon_tools().append(copy.deepcopy(daemon_tools()[0])),
                "unique",
            ),
            (
                "tool-count",
                lambda: self.tool_inventory["driver"]["daemon_tools_list"].update(  # type: ignore[index,union-attr]
                    tools=[{"name": f"tool-{index}"} for index in range(257)]
                ),
                "bounded array",
            ),
            (
                "depth",
                lambda: daemon_tools()[0].update(input_schema=deeply_nested_schema()),
                "nesting depth",
            ),
            (
                "float",
                lambda: daemon_tools()[0].update(timeout=1.5),
                "floating-point",
            ),
            (
                "claimed-digest",
                lambda: self.tool_inventory["driver"].update(  # type: ignore[union-attr]
                    daemon_tool_schemas_sha256="0" * 64
                ),
                "digest mismatch",
            ),
            (
                "daemon-envelope-addition",
                lambda: self.tool_inventory["driver"]["daemon_tools_list_envelope"].update(
                    unexpected=True
                ),  # type: ignore[index,union-attr]
                "closed schema",
            ),
            (
                "daemon-envelope-removal",
                lambda: self.tool_inventory["driver"]["daemon_tools_list_envelope"].pop(
                    "capability_version"
                ),  # type: ignore[index,union-attr]
                "closed schema",
            ),
            (
                "daemon-envelope-float",
                lambda: self.tool_inventory["driver"]["daemon_tools_list_envelope"][
                    "enforcement_adapters"
                ][0].update(weight=1.5),  # type: ignore[index,union-attr]
                "floating-point",
            ),
            (
                "daemon-envelope-overbound",
                lambda: self.tool_inventory["driver"]["daemon_tools_list_envelope"].update(
                    enforcement_adapters=[{"id": "x"}] * 4097
                ),  # type: ignore[index,union-attr]
                "oversized array",
            ),
            (
                "daemon-envelope-empty-adapters",
                lambda: self.tool_inventory["driver"]["daemon_tools_list_envelope"].update(
                    enforcement_adapters=[]
                ),  # type: ignore[index,union-attr]
                "unique bounded object ids",
            ),
            (
                "daemon-envelope-tool-drift",
                lambda: self.tool_inventory["driver"]["daemon_tools_list_envelope"].update(
                    tools=[{"name": "other"}]
                ),  # type: ignore[index,union-attr]
                "do not match",
            ),
            (
                "unknown-field",
                lambda: self.tool_inventory.update(extra=True),
                "closed schema",
            ),
            (
                "unbound-native-tools",
                lambda: self.tool_inventory.update(native_harness_tools=["shell", "file_edit"]),
                "closed codex capability set",
            ),
        )
        for name, mutate, message in cases:
            with self.subTest(name=name):
                self.tool_inventory = copy.deepcopy(baseline)
                mutate()
                manifest = self._write()
                with self.assertRaisesRegex(ProductionSystemError, message):
                    self._plan(manifest)

    def test_tool_inventory_candidate_and_system_bindings_fail_closed(self) -> None:
        manifest = self._write()
        manifest["capability_inventory"]["tools"]["sha256"] = "0" * 64  # type: ignore[index]
        with self.assertRaisesRegex(ProductionSystemError, "manifest sha256"):
            self._plan(manifest)

        manifest = self._write()
        manifest["driver"]["candidate"]["id"] = "other-driver"  # type: ignore[index]
        with self.assertRaises(ProductionSystemError):
            self._plan(manifest)

        self.skill_inventory["skills"][0]["version"] = "0.20.1"  # type: ignore[index]
        self.tool_inventory["driver"]["version"] = "0.20.1"  # type: ignore[index]
        manifest = self._write()
        manifest["driver"]["candidate"]["version"] = "0.20.1"  # type: ignore[index]
        with self.assertRaisesRegex(ProductionSystemError, "must be trycua.cua-driver 0.20.0"):
            self._plan(manifest)
        self.skill_inventory["skills"][0]["version"] = "0.20.0"  # type: ignore[index]
        self.tool_inventory["driver"]["version"] = "0.20.0"  # type: ignore[index]

        manifest = self._write()
        self.tool_inventory["driver"]["version"] = "0.20.1"  # type: ignore[index]
        manifest = self._write()
        with self.assertRaisesRegex(ProductionSystemError, "does not match"):
            self._plan(manifest)

        self.tool_inventory["driver"]["version"] = "0.20.0"  # type: ignore[index]
        manifest = self._write()
        manifest["driver"]["tool_contract_sha256"] = "0" * 64  # type: ignore[index]
        with self.assertRaisesRegex(ProductionSystemError, "frozen MCP schemas"):
            self._plan(manifest)

    def test_skill_provenance_has_a_closed_bounded_schema(self) -> None:
        baseline = copy.deepcopy(self.skill_inventory)

        def skill() -> dict[str, object]:
            return self.skill_inventory["skills"][0]  # type: ignore[index,return-value]

        cases = (
            (
                "missing-generator",
                lambda: self.skill_inventory.pop("generator"),
                "closed schema",
            ),
            (
                "extra-generator-field",
                lambda: self.skill_inventory["generator"].update(extra="x"),  # type: ignore[union-attr]
                "closed schema",
            ),
            (
                "empty-generator-version",
                lambda: self.skill_inventory["generator"].update(version=""),  # type: ignore[union-attr]
                "non-empty",
            ),
            (
                "unsupported-format",
                lambda: skill().update(format="markdown"),
                "format must be agents-skill-v1",
            ),
            (
                "missing-source",
                lambda: skill().pop("source"),
                "closed schema",
            ),
            (
                "extra-source-field",
                lambda: skill()["source"].update(extra="x"),  # type: ignore[union-attr]
                "closed schema",
            ),
            (
                "unsafe-repository",
                lambda: skill()["source"].update(repository="https://repo\nsecret"),  # type: ignore[union-attr]
                "bounded printable ASCII",
            ),
            (
                "oversized-repository",
                lambda: skill()["source"].update(repository="x" * 513),  # type: ignore[union-attr]
                "bounded printable ASCII",
            ),
            (
                "unsafe-revision",
                lambda: skill()["source"].update(revision="bad revision"),  # type: ignore[union-attr]
                "unsupported characters",
            ),
            (
                "source-traversal",
                lambda: skill()["source"].update(path="../cua-driver"),  # type: ignore[union-attr]
                "source path is unsafe",
            ),
        )
        for name, mutate, message in cases:
            with self.subTest(name=name):
                self.skill_inventory = copy.deepcopy(baseline)
                mutate()
                manifest = self._write()
                with self.assertRaisesRegex(ProductionSystemError, message):
                    self._plan(manifest)

    def test_skill_provenance_is_bound_into_policy_digest(self) -> None:
        baseline = self._plan()
        assert baseline is not None
        self.skill_inventory["generator"]["version"] = "1.0.1"  # type: ignore[index]
        changed = self._plan()
        assert changed is not None
        self.assertNotEqual(baseline.policy_digest, changed.policy_digest)

    def test_skill_inventory_raw_input_is_bounded_before_json_parse(self) -> None:
        manifest = self._write()
        oversized = b" " * (8 * 1024 * 1024 + 1)
        self.skill_inventory_path.write_bytes(oversized)
        manifest["capability_inventory"]["skills"]["sha256"] = hashlib.sha256(  # type: ignore[index]
            oversized
        ).hexdigest()
        with self.assertRaisesRegex(ProductionSystemError, "size limit"):
            self._plan(manifest)

    def test_production_requires_bundled_native_skill_capability(self) -> None:
        for mutation, message in (
            (
                lambda manifest: manifest["driver"].update(skill_mode="removed"),  # type: ignore[union-attr]
                "skill_mode must be bundled",
            ),
            (
                lambda manifest: manifest["driver"]["profile"].update(id="other"),  # type: ignore[index,union-attr]
                "profile must be native-bundle",
            ),
            (
                lambda manifest: manifest["capability_inventory"].update(  # type: ignore[union-attr]
                    native_harness_capabilities_enabled=False
                ),
                "capabilities must be enabled",
            ),
        ):
            manifest = self._write()
            mutation(manifest)
            with self.assertRaisesRegex(ProductionSystemError, message):
                self._plan(manifest)


if __name__ == "__main__":
    unittest.main()
