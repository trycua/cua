"""Strict, secret-free production harness plans from frozen system manifests."""

from __future__ import annotations

import hashlib
import json
import posixpath
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from .production import (
    BundledSkillFile,
    CLAUDE_CODE_OAUTH_CREDENTIAL,
    CLAUDE_CODE_PINNED_VERSION,
    HarnessKind,
    ModelRoute,
    ProxyConfiguration,
    ProductionHarnessSpec,
)


PRODUCTION_ADAPTER = "cua-driver-bench-production-v1"

_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_BUILD_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "harness_id",
        "version",
        "guest_executable",
        "guest_executable_sha256",
    }
)
_BUILD_OPTIONAL_FIELDS = frozenset({"support_executables"})
_SUPPORT_EXECUTABLE_FIELDS = frozenset({"path", "sha256"})
_CODEX_CODE_MODE_HOST = "/usr/local/bin/codex-code-mode-host"
_CONFIG_FIELDS = frozenset(
    {
        "schema_version",
        "adapter",
        "credential_environment",
        "proxy_endpoint",
        "provider_allowlist",
        "provider_allowlist_sha256",
        "proxy_implementation_sha256",
    }
)
_CREDENTIAL_BY_HARNESS_AND_PROVIDER = {
    (HarnessKind.CODEX, "openai"): frozenset({"CDB_CODEX_AUTH_JSON"}),
    (HarnessKind.CLAUDE_CODE, "anthropic"): frozenset({CLAUDE_CODE_OAUTH_CREDENTIAL}),
    (HarnessKind.OPENCODE, "opencode"): frozenset(),
}
_GUEST_EXECUTABLE_ROOT = "/usr/local/bin/"
_SKILL_FIELDS = frozenset({"schema_version", "generator", "skills"})
_SKILL_GENERATOR_FIELDS = frozenset({"name", "version"})
_SKILL_ENTRY_FIELDS = frozenset({"id", "version", "format", "source", "files"})
_SKILL_SOURCE_FIELDS = frozenset({"repository", "revision", "path"})
_SKILL_FILE_FIELDS = frozenset({"path", "sha256", "content"})
_MAX_SKILL_FILES = 128
_MAX_SKILL_FILE_BYTES = 1024 * 1024
_MAX_SKILL_BUNDLE_BYTES = 4 * 1024 * 1024
_MAX_SKILL_INVENTORY_BYTES = 8 * 1024 * 1024
_MAX_SKILL_PATH_BYTES = 1024
_MAX_PROVENANCE_TEXT_BYTES = 512
_SAFE_SKILL_PATH_PART = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$")
_SAFE_PROVENANCE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,127}$")
_SUPPORTED_SKILL_FORMAT = "agents-skill-v1"
_TOOL_FIELDS = frozenset(
    {
        "schema_version",
        "generator",
        "driver",
        "native_harness_tools",
    }
)
_TOOL_DRIVER_FIELDS = frozenset(
    {
        "id",
        "version",
        "source",
        "mcp_tools_list",
        "daemon_tools_list",
        "mcp_tools_list_envelope",
        "daemon_tools_list_envelope",
        "mcp_tools_list_envelope_sha256",
        "daemon_tools_list_envelope_sha256",
        "mcp_tool_schemas_sha256",
        "daemon_tool_schemas_sha256",
        "digest_algorithm",
    }
)
_TOOL_SOURCE_FIELDS = frozenset({"repository", "revision", "path"})
_PINNED_DRIVER_ID = "trycua.cua-driver"
_PINNED_DRIVER_VERSION = "0.20.0"
_MAX_TOOL_INVENTORY_BYTES = 4 * 1024 * 1024
_MAX_TOOL_COUNT = 256
_MAX_ENFORCEMENT_ADAPTERS = 128
_MAX_NATIVE_HARNESS_TOOLS = 128
_MAX_TOOL_DOCUMENT_DEPTH = 32
_MAX_TOOL_CONTAINER_ENTRIES = 4096
_MAX_TOOL_STRING_BYTES = 256 * 1024
_MAX_TOOL_NAME_BYTES = 256
_NATIVE_HARNESS_TOOLS_BY_KIND = {
    HarnessKind.CODEX: frozenset({"shell", "file_edit", "subagent"}),
    HarnessKind.CLAUDE_CODE: frozenset({"shell", "file_edit", "subagent"}),
    HarnessKind.OPENCODE: frozenset({"shell", "file_edit", "subagent"}),
}


class ProductionSystemError(ValueError):
    """An activated production system artifact is malformed or mismatched."""


@dataclass(frozen=True)
class ProductionSystemPlan:
    """Frozen non-secret metadata needed to launch one production harness."""

    kind: HarnessKind
    harness: ProductionHarnessSpec
    model_route: ModelRoute
    guest_executable: str
    guest_executable_sha256: str
    credential_environment: tuple[str, ...]
    proxy: ProxyConfiguration
    policy_digest: str
    skill_files: tuple[BundledSkillFile, ...]
    daemon_tool_schemas_sha256: str
    mcp_tool_schemas_sha256: str
    daemon_tools_list_envelope_sha256: str
    mcp_tools_list_envelope_sha256: str
    tool_inventory_sha256: str
    daemon_tool_count: int
    mcp_tool_count: int
    support_executables: tuple[tuple[str, str], ...] = ()
    tool_names: tuple[str, ...] = ()


def production_system_plan(
    system_manifest: Mapping[str, Any],
    *,
    build_path: Path,
    configuration_path: Path,
    skill_inventory_path: Path,
    tool_inventory_path: Path,
) -> ProductionSystemPlan | None:
    """Derive a production guest plan from a validated v0.3 system manifest.

    A configuration without the exact production adapter marker is not a
    production configuration and returns ``None``.  Once that marker is
    present, every artifact field and manifest binding is fail-closed.
    """

    configuration_bytes, configuration = _read_json_object(
        configuration_path, "harness configuration"
    )
    if configuration.get("adapter") != PRODUCTION_ADAPTER:
        return None

    if system_manifest.get("schema_version") != "0.3.0":
        raise ProductionSystemError("system manifest schema_version must be 0.3.0")
    _require_fields(configuration, _CONFIG_FIELDS, "harness configuration")
    _require_schema_version(configuration, "harness configuration")
    _verify_manifest_artifact(
        system_manifest,
        artifact_name="configuration",
        artifact_bytes=configuration_bytes,
    )

    build_bytes, build = _read_json_object(build_path, "harness build")
    if frozenset(build) not in {
        _BUILD_FIELDS,
        _BUILD_FIELDS | _BUILD_OPTIONAL_FIELDS,
    }:
        raise ProductionSystemError("harness build fields do not match the closed schema")
    _require_schema_version(build, "harness build")
    _verify_manifest_artifact(system_manifest, artifact_name="build", artifact_bytes=build_bytes)

    try:
        kind = HarnessKind(build["kind"])
    except (TypeError, ValueError) as error:
        raise ProductionSystemError("harness build kind is unsupported") from error

    harness_manifest = _object(system_manifest.get("harness"), "system harness")
    harness_id = _text(harness_manifest.get("id"), "system harness id")
    harness_version = _text(harness_manifest.get("version"), "system harness version")
    if build["harness_id"] != harness_id:
        raise ProductionSystemError("harness build id does not match system manifest")
    if build["version"] != harness_version:
        raise ProductionSystemError("harness build version does not match system manifest")
    if kind is HarnessKind.CLAUDE_CODE:
        if harness_version != CLAUDE_CODE_PINNED_VERSION:
            raise ProductionSystemError(
                f"Claude Code version must be pinned to {CLAUDE_CODE_PINNED_VERSION}"
            )
        if build["guest_executable"] != "/usr/local/bin/claude":
            raise ProductionSystemError(
                "Claude Code guest executable must be /usr/local/bin/claude"
            )

    executable = _guest_executable(build["guest_executable"])
    executable_sha256 = _digest(build["guest_executable_sha256"], "guest executable sha256")
    support_executables = _support_executables(build.get("support_executables", []), kind)
    route, route_configuration_pin = _primary_model_route(system_manifest)
    credentials = _credential_environment(configuration, kind, route.provider)
    proxy = _proxy_configuration(configuration)
    configuration_pin = _manifest_digest(system_manifest, artifact_name="configuration")
    if route_configuration_pin != configuration_pin:
        raise ProductionSystemError(
            "primary model route configuration does not match harness configuration"
        )
    skill_files, skill_metadata, skill_inventory_sha256 = _skill_bundle(
        system_manifest, skill_inventory_path
    )
    tool_metadata = _tool_contract(system_manifest, tool_inventory_path, kind=kind)

    spec = ProductionHarnessSpec(kind=kind, executable=executable)
    policy_metadata = {
        "adapter": PRODUCTION_ADAPTER,
        "system_id": _text(system_manifest.get("id"), "system id"),
        "system_version": _text(system_manifest.get("version"), "system version"),
        "harness_id": harness_id,
        "harness_version": harness_version,
        "kind": kind.value,
        "guest_executable": executable,
        "guest_executable_sha256": executable_sha256,
        "support_executables": [
            {"path": path, "sha256": digest} for path, digest in support_executables
        ],
        "credential_environment": list(credentials),
        "proxy_endpoint": proxy.endpoint,
        "provider_allowlist": list(proxy.provider_allowlist),
        "provider_allowlist_sha256": proxy.provider_allowlist_sha256,
        "proxy_implementation_sha256": proxy.implementation_sha256,
        "build_sha256": _manifest_digest(system_manifest, artifact_name="build"),
        "configuration_sha256": configuration_pin,
        "skill_inventory_sha256": skill_inventory_sha256,
        "tool_inventory_sha256": tool_metadata["inventory_sha256"],
        "daemon_tool_schemas_sha256": tool_metadata["daemon_sha256"],
        "mcp_tool_schemas_sha256": tool_metadata["mcp_sha256"],
        "daemon_tools_list_envelope_sha256": tool_metadata["daemon_envelope_sha256"],
        "mcp_tools_list_envelope_sha256": tool_metadata["mcp_envelope_sha256"],
        "daemon_tool_count": tool_metadata["daemon_count"],
        "mcp_tool_count": tool_metadata["mcp_count"],
        "skill": {
            **skill_metadata,
            "files": [
                {
                    "path": file.path.as_posix(),
                    "sha256": hashlib.sha256(file.content).hexdigest(),
                    "size": len(file.content),
                }
                for file in skill_files
            ],
        },
        "model_route": {
            "id": route.route_id,
            "role": route.role,
            "provider": route.provider,
            "model": route.model,
            "snapshot": route.snapshot,
            "service_tier": route.service_tier,
        },
    }
    canonical = json.dumps(
        policy_metadata, ensure_ascii=True, sort_keys=True, separators=(",", ":")
    ).encode("ascii")
    policy_digest = f"sha256:{hashlib.sha256(canonical).hexdigest()}"
    return ProductionSystemPlan(
        kind=kind,
        harness=spec,
        model_route=route,
        guest_executable=executable,
        guest_executable_sha256=executable_sha256,
        credential_environment=credentials,
        proxy=proxy,
        skill_files=skill_files,
        policy_digest=policy_digest,
        daemon_tool_schemas_sha256=tool_metadata["daemon_sha256"],
        mcp_tool_schemas_sha256=tool_metadata["mcp_sha256"],
        daemon_tools_list_envelope_sha256=tool_metadata["daemon_envelope_sha256"],
        mcp_tools_list_envelope_sha256=tool_metadata["mcp_envelope_sha256"],
        tool_inventory_sha256=tool_metadata["inventory_sha256"],
        daemon_tool_count=tool_metadata["daemon_count"],
        mcp_tool_count=tool_metadata["mcp_count"],
        support_executables=support_executables,
        tool_names=tool_metadata["tool_names"],
    )


def _support_executables(value: Any, kind: HarnessKind) -> tuple[tuple[str, str], ...]:
    if not isinstance(value, list):
        raise ProductionSystemError("support_executables must be an array")
    parsed: list[tuple[str, str]] = []
    for item in value:
        entry = _object(item, "support executable")
        _require_fields(entry, _SUPPORT_EXECUTABLE_FIELDS, "support executable")
        parsed.append(
            (
                _guest_executable(entry["path"]),
                _digest(entry["sha256"], "support executable sha256"),
            )
        )
    if len(parsed) != len(set(path for path, _digest_value in parsed)):
        raise ProductionSystemError("support executable paths must be unique")
    canonical = tuple(sorted(parsed))
    if kind is HarnessKind.CODEX:
        if len(canonical) != 1 or canonical[0][0] != _CODEX_CODE_MODE_HOST:
            raise ProductionSystemError(
                "Codex support_executables must pin /usr/local/bin/codex-code-mode-host"
            )
    elif canonical:
        raise ProductionSystemError(
            "support_executables are unsupported for this production harness"
        )
    return canonical


def _proxy_configuration(configuration: Mapping[str, Any]) -> ProxyConfiguration:
    endpoint = configuration.get("proxy_endpoint")
    allowlist = configuration.get("provider_allowlist")
    digest = configuration.get("provider_allowlist_sha256")
    implementation_digest = configuration.get("proxy_implementation_sha256")
    if not isinstance(endpoint, str):
        raise ProductionSystemError("proxy_endpoint must be a string")
    if not isinstance(allowlist, list) or any(not isinstance(entry, str) for entry in allowlist):
        raise ProductionSystemError("provider_allowlist must be an array of strings")
    if not isinstance(digest, str):
        raise ProductionSystemError("provider_allowlist_sha256 must be a string")
    if not isinstance(implementation_digest, str):
        raise ProductionSystemError("proxy_implementation_sha256 must be a string")
    try:
        return ProxyConfiguration(
            endpoint=endpoint,
            provider_allowlist=tuple(allowlist),
            provider_allowlist_sha256=digest,
            implementation_sha256=implementation_digest,
        )
    except ValueError as error:
        raise ProductionSystemError(str(error)) from error


def _tool_contract(
    system_manifest: Mapping[str, Any],
    tool_inventory_path: Path,
    *,
    kind: HarnessKind,
) -> dict[str, Any]:
    """Validate the frozen native MCP and raw daemon schema inventory."""

    inventory_bytes, inventory = _read_json_object(
        tool_inventory_path,
        "tool inventory",
        max_bytes=_MAX_TOOL_INVENTORY_BYTES,
    )
    _require_fields(inventory, _TOOL_FIELDS, "tool inventory")
    if inventory["schema_version"] != 1:
        raise ProductionSystemError("tool inventory schema_version must be 1")
    generator = _object(inventory["generator"], "tool inventory generator")
    _require_fields(generator, _SKILL_GENERATOR_FIELDS, "tool inventory generator")
    _provenance_id(generator["name"], "tool inventory generator name")
    _provenance_id(generator["version"], "tool inventory generator version")

    capability_inventory = _object(
        system_manifest.get("capability_inventory"), "system capability inventory"
    )
    declaration = _object(capability_inventory.get("tools"), "system tool artifact")
    expected_inventory_sha256 = _digest(declaration.get("sha256"), "system tool inventory sha256")
    inventory_sha256 = hashlib.sha256(inventory_bytes).hexdigest()
    if inventory_sha256 != expected_inventory_sha256:
        raise ProductionSystemError("tool inventory artifact does not match system manifest sha256")

    driver = _object(inventory["driver"], "tool inventory driver")
    _require_fields(driver, _TOOL_DRIVER_FIELDS, "tool inventory driver")
    manifest_driver = _object(system_manifest.get("driver"), "system driver")
    candidate = _object(manifest_driver.get("candidate"), "system driver candidate")
    candidate_id = _text(candidate.get("id"), "system driver candidate id")
    candidate_version = _text(candidate.get("version"), "system driver candidate version")
    if candidate_id != _PINNED_DRIVER_ID or candidate_version != _PINNED_DRIVER_VERSION:
        raise ProductionSystemError("production driver candidate must be trycua.cua-driver 0.20.0")
    if driver["id"] != candidate_id or driver["version"] != candidate_version:
        raise ProductionSystemError("tool inventory driver does not match system driver candidate")
    source = _object(driver["source"], "tool inventory driver source")
    _require_fields(source, _TOOL_SOURCE_FIELDS, "tool inventory driver source")
    _provenance_text(source["repository"], "tool inventory source repository")
    revision = _provenance_id(source["revision"], "tool inventory source revision")
    if revision != "cua-driver-rs-v0.20.0":
        raise ProductionSystemError("tool inventory source revision must pin cua-driver-rs-v0.20.0")
    _skill_relative_path(source["path"], "tool inventory source path")
    if driver["digest_algorithm"] != "sha256-rfc8785":
        raise ProductionSystemError("tool inventory digest_algorithm must be sha256-rfc8785")

    daemon_envelope = _list_result(
        driver["daemon_tools_list_envelope"],
        "daemon tools/list envelope",
        daemon=True,
    )
    mcp_envelope = _list_result(
        driver["mcp_tools_list_envelope"], "MCP tools/list envelope", daemon=False
    )
    daemon_tools = _tool_list(driver["daemon_tools_list"], "daemon tool list")
    mcp_tools = _tool_list(driver["mcp_tools_list"], "MCP tool list")
    if daemon_envelope["tools"] != daemon_tools:
        raise ProductionSystemError("daemon envelope tools do not match tool array")
    if mcp_envelope["tools"] != mcp_tools:
        raise ProductionSystemError("MCP envelope tools do not match tool array")
    daemon_sha256 = hashlib.sha256(_rfc8785_bytes(daemon_tools)).hexdigest()
    mcp_sha256 = hashlib.sha256(_rfc8785_bytes(mcp_tools)).hexdigest()
    daemon_envelope_sha256 = hashlib.sha256(_rfc8785_bytes(daemon_envelope)).hexdigest()
    mcp_envelope_sha256 = hashlib.sha256(_rfc8785_bytes(mcp_envelope)).hexdigest()
    if daemon_sha256 != _digest(driver["daemon_tool_schemas_sha256"], "daemon tool schemas sha256"):
        raise ProductionSystemError("daemon tool schema digest mismatch")
    if mcp_sha256 != _digest(driver["mcp_tool_schemas_sha256"], "MCP tool schemas sha256"):
        raise ProductionSystemError("MCP tool schema digest mismatch")
    if daemon_envelope_sha256 != _digest(
        driver["daemon_tools_list_envelope_sha256"],
        "daemon tools/list envelope sha256",
    ):
        raise ProductionSystemError("daemon tools/list envelope digest mismatch")
    if mcp_envelope_sha256 != _digest(
        driver["mcp_tools_list_envelope_sha256"],
        "MCP tools/list envelope sha256",
    ):
        raise ProductionSystemError("MCP tools/list envelope digest mismatch")
    contract_sha256 = _digest(
        manifest_driver.get("tool_contract_sha256"),
        "system driver tool contract sha256",
    )
    if contract_sha256 != mcp_sha256:
        raise ProductionSystemError("system driver tool contract does not match frozen MCP schemas")

    native = inventory["native_harness_tools"]
    if (
        not isinstance(native, list)
        or not native
        or len(native) > _MAX_NATIVE_HARNESS_TOOLS
        or any(
            not isinstance(name, str)
            or not name.strip()
            or len(name.encode("utf-8")) > _MAX_TOOL_NAME_BYTES
            for name in native
        )
        or len(native) != len(set(native))
    ):
        raise ProductionSystemError("native harness tools must be unique non-empty bounded strings")
    expected_native_tools = _NATIVE_HARNESS_TOOLS_BY_KIND.get(kind)
    if expected_native_tools is None or frozenset(native) != expected_native_tools:
        raise ProductionSystemError(
            f"native harness tools do not match the closed {kind.value} capability set"
        )
    return {
        "inventory_sha256": inventory_sha256,
        "daemon_sha256": daemon_sha256,
        "mcp_sha256": mcp_sha256,
        "daemon_envelope_sha256": daemon_envelope_sha256,
        "mcp_envelope_sha256": mcp_envelope_sha256,
        "daemon_count": len(daemon_tools),
        "mcp_count": len(mcp_tools),
        "tool_names": tuple(
            sorted(
                {
                    *(tool["name"] for tool in mcp_tools),
                    *native,
                }
            )
        ),
    }


def _list_result(value: Any, label: str, *, daemon: bool) -> Mapping[str, Any]:
    result = _object(value, label)
    expected_fields = {"schema_version", "capability_version", "tools"}
    if daemon:
        expected_fields.update({"enforcement_adapters", "tool_observation_owner"})
    _require_fields(result, frozenset(expected_fields), label)
    _bounded_json(result, label=label)
    if result["schema_version"] != "1" or result["capability_version"] != "1":
        raise ProductionSystemError(f"{label} version metadata is unsupported")
    if daemon:
        if result["tool_observation_owner"] != "daemon":
            raise ProductionSystemError("daemon list result tool_observation_owner must be daemon")
        adapters = result["enforcement_adapters"]
        if (
            not isinstance(adapters, list)
            or not adapters
            or len(adapters) > _MAX_ENFORCEMENT_ADAPTERS
            or any(not isinstance(adapter, Mapping) for adapter in adapters)
            or any(
                not isinstance(adapter.get("id"), str)
                or not adapter["id"].strip()
                or len(adapter["id"].encode("utf-8")) > _MAX_TOOL_NAME_BYTES
                for adapter in adapters
            )
            or len({adapter["id"] for adapter in adapters}) != len(adapters)
        ):
            raise ProductionSystemError(
                "daemon list result enforcement_adapters must have unique bounded object ids"
            )
    return result


def _tool_list(value: Any, label: str) -> list[Any]:
    wrapper = _object(value, label)
    _require_fields(wrapper, frozenset({"tools"}), label)
    tools = wrapper["tools"]
    if not isinstance(tools, list) or not tools or len(tools) > _MAX_TOOL_COUNT:
        raise ProductionSystemError(f"{label} tools must be a non-empty bounded array")
    names: list[str] = []
    for index, tool in enumerate(tools):
        _bounded_json(tool, label=f"{label} tool {index}")
        if not isinstance(tool, Mapping):
            raise ProductionSystemError(f"{label} tool {index} must be an object")
        name = tool.get("name")
        if (
            not isinstance(name, str)
            or not name.strip()
            or len(name.encode("utf-8")) > _MAX_TOOL_NAME_BYTES
        ):
            raise ProductionSystemError(f"{label} tool names must be non-empty and bounded")
        names.append(name)
    if len(names) != len(set(names)):
        raise ProductionSystemError(f"{label} tool names must be unique")
    return tools


def _bounded_json(value: Any, *, label: str, depth: int = 0) -> None:
    if depth > _MAX_TOOL_DOCUMENT_DEPTH:
        raise ProductionSystemError(f"{label} exceeds maximum nesting depth")
    if (
        value is None
        or isinstance(value, bool)
        or (isinstance(value, int) and not isinstance(value, bool))
    ):
        if isinstance(value, int) and not (-(2**53) + 1 <= value <= 2**53 - 1):
            raise ProductionSystemError(f"{label} integer is outside the safe range")
        return
    if isinstance(value, float):
        raise ProductionSystemError(f"{label} must not contain floating-point values")
    if isinstance(value, str):
        if len(value.encode("utf-8")) > _MAX_TOOL_STRING_BYTES:
            raise ProductionSystemError(f"{label} contains an oversized string")
        if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
            raise ProductionSystemError(f"{label} contains an invalid Unicode surrogate")
        return
    if isinstance(value, list):
        if len(value) > _MAX_TOOL_CONTAINER_ENTRIES:
            raise ProductionSystemError(f"{label} contains an oversized array")
        for item in value:
            _bounded_json(item, label=label, depth=depth + 1)
        return
    if isinstance(value, Mapping):
        if len(value) > _MAX_TOOL_CONTAINER_ENTRIES:
            raise ProductionSystemError(f"{label} contains an oversized object")
        for key, item in value.items():
            if not isinstance(key, str) or not key:
                raise ProductionSystemError(f"{label} contains an invalid object key")
            _bounded_json(key, label=label, depth=depth + 1)
            _bounded_json(item, label=label, depth=depth + 1)
        return
    raise ProductionSystemError(f"{label} contains an unsupported JSON value")


def _rfc8785_bytes(value: Any) -> bytes:
    """Canonicalize the bounded integer-only JSON subset used by tool schemas."""

    _bounded_json(value, label="tool schema")

    def encode(item: Any) -> str:
        if item is None:
            return "null"
        if item is True:
            return "true"
        if item is False:
            return "false"
        if isinstance(item, int):
            return str(item)
        if isinstance(item, str):
            return json.dumps(item, ensure_ascii=False, separators=(",", ":"))
        if isinstance(item, list):
            return "[" + ",".join(encode(child) for child in item) + "]"
        assert isinstance(item, Mapping)
        keys = sorted(item, key=lambda key: key.encode("utf-16-be"))
        return "{" + ",".join(encode(key) + ":" + encode(item[key]) for key in keys) + "}"

    return encode(value).encode("utf-8")


def _skill_bundle(
    system_manifest: Mapping[str, Any], skill_inventory_path: Path
) -> tuple[tuple[BundledSkillFile, ...], dict[str, Any], str]:
    driver = _object(system_manifest.get("driver"), "system driver")
    if driver.get("skill_mode") != "bundled":
        raise ProductionSystemError("production driver skill_mode must be bundled")
    profile = _object(driver.get("profile"), "system driver profile")
    if profile.get("id") != "native-bundle":
        raise ProductionSystemError("production driver profile must be native-bundle")
    candidate = _object(driver.get("candidate"), "system driver candidate")
    expected_version = _text(candidate.get("version"), "system driver candidate version")
    capability_inventory = _object(
        system_manifest.get("capability_inventory"), "system capability inventory"
    )
    if capability_inventory.get("native_harness_capabilities_enabled") is not True:
        raise ProductionSystemError("native harness capabilities must be enabled")

    inventory_bytes, inventory = _read_json_object(
        skill_inventory_path,
        "skill inventory",
        max_bytes=_MAX_SKILL_INVENTORY_BYTES,
    )
    _require_fields(inventory, _SKILL_FIELDS, "skill inventory")
    _require_schema_version(inventory, "skill inventory")
    generator = _object(inventory["generator"], "skill inventory generator")
    _require_fields(generator, _SKILL_GENERATOR_FIELDS, "skill inventory generator")
    generator_metadata = {
        "name": _provenance_id(generator["name"], "skill generator name"),
        "version": _provenance_id(generator["version"], "skill generator version"),
    }
    expected_inventory_digest = _digest(
        _object(capability_inventory.get("skills"), "system skill artifact").get("sha256"),
        "system skill inventory sha256",
    )
    if hashlib.sha256(inventory_bytes).hexdigest() != expected_inventory_digest:
        raise ProductionSystemError(
            "skill inventory artifact does not match system manifest sha256"
        )
    skills = inventory["skills"]
    if not isinstance(skills, list) or len(skills) != 1:
        raise ProductionSystemError("bundled skill inventory must contain exactly one skill")
    skill = skills[0]
    if not isinstance(skill, Mapping):
        raise ProductionSystemError("skill inventory entry must be an object")
    _require_fields(skill, _SKILL_ENTRY_FIELDS, "skill inventory entry")
    if skill["id"] != "cua-driver":
        raise ProductionSystemError("bundled skill id must be cua-driver")
    if skill["version"] != expected_version:
        raise ProductionSystemError("bundled skill version does not match driver candidate")
    if skill["format"] != _SUPPORTED_SKILL_FORMAT:
        raise ProductionSystemError(f"bundled skill format must be {_SUPPORTED_SKILL_FORMAT}")
    source = _object(skill["source"], "bundled skill source")
    _require_fields(source, _SKILL_SOURCE_FIELDS, "bundled skill source")
    source_metadata = {
        "repository": _provenance_text(source["repository"], "bundled skill source repository"),
        "revision": _provenance_id(source["revision"], "bundled skill source revision"),
        "path": _skill_relative_path(source["path"], "bundled skill source path"),
    }
    raw_files = skill["files"]
    if not isinstance(raw_files, list) or not raw_files:
        raise ProductionSystemError("bundled skill files must be a non-empty array")
    if len(raw_files) > _MAX_SKILL_FILES:
        raise ProductionSystemError("bundled skill has too many files")
    files: list[BundledSkillFile] = []
    paths: set[str] = set()
    folded_paths: set[str] = set()
    total_size = 0
    for raw_file in raw_files:
        if not isinstance(raw_file, Mapping):
            raise ProductionSystemError("bundled skill file must be an object")
        _require_fields(raw_file, _SKILL_FILE_FIELDS, "bundled skill file")
        raw_path = _skill_relative_path(raw_file["path"], "bundled skill file path")
        folded_path = raw_path.casefold()
        if raw_path in paths or folded_path in folded_paths:
            raise ProductionSystemError("bundled skill file path is duplicated")
        paths.add(raw_path)
        folded_paths.add(folded_path)
        content_text = raw_file["content"]
        if not isinstance(content_text, str):
            raise ProductionSystemError("bundled skill file content must be UTF-8 text")
        try:
            content = content_text.encode("utf-8")
        except UnicodeEncodeError as error:
            raise ProductionSystemError("bundled skill file content must be UTF-8 text") from error
        if len(content) > _MAX_SKILL_FILE_BYTES:
            raise ProductionSystemError("bundled skill file exceeds the size limit")
        total_size += len(content)
        if total_size > _MAX_SKILL_BUNDLE_BYTES:
            raise ProductionSystemError("bundled skill exceeds the size limit")
        if hashlib.sha256(content).hexdigest() != _digest(
            raw_file["sha256"], "bundled skill file sha256"
        ):
            raise ProductionSystemError("bundled skill file sha256 mismatch")
        files.append(BundledSkillFile(PurePosixPath(raw_path), content))
    if "SKILL.md" not in paths:
        raise ProductionSystemError("bundled skill must contain SKILL.md")
    metadata = {
        "id": "cua-driver",
        "version": expected_version,
        "format": _SUPPORTED_SKILL_FORMAT,
        "generator": generator_metadata,
        "source": source_metadata,
    }
    return tuple(files), metadata, expected_inventory_digest


def _provenance_id(value: Any, label: str) -> str:
    text = _text(value, label)
    if _SAFE_PROVENANCE_ID.fullmatch(text) is None:
        raise ProductionSystemError(f"{label} contains unsupported characters")
    return text


def _provenance_text(value: Any, label: str) -> str:
    text = _text(value, label)
    try:
        encoded = text.encode("ascii")
    except UnicodeEncodeError as error:
        raise ProductionSystemError(f"{label} must be printable ASCII") from error
    if len(encoded) > _MAX_PROVENANCE_TEXT_BYTES or any(
        byte < 0x21 or byte > 0x7E for byte in encoded
    ):
        raise ProductionSystemError(f"{label} must be bounded printable ASCII")
    return text


def _skill_relative_path(value: Any, label: str) -> str:
    raw_path = _text(value, label)
    path = PurePosixPath(raw_path)
    try:
        encoded = raw_path.encode("ascii")
    except UnicodeEncodeError as error:
        raise ProductionSystemError(f"{label} is unsafe") from error
    if (
        raw_path.startswith("/")
        or raw_path == "."
        or "\\" in raw_path
        or posixpath.normpath(raw_path) != raw_path
        or ".." in path.parts
        or any(_SAFE_SKILL_PATH_PART.fullmatch(part) is None for part in path.parts)
        or len(encoded) > _MAX_SKILL_PATH_BYTES
    ):
        raise ProductionSystemError(f"{label} is unsafe")
    return raw_path


def _read_json_object(
    path: Path, label: str, *, max_bytes: int | None = None
) -> tuple[bytes, dict[str, Any]]:
    try:
        source = Path(path)
        if max_bytes is not None and source.stat().st_size > max_bytes:
            raise ProductionSystemError(f"{label} exceeds the size limit")
        data = source.read_bytes()
        if max_bytes is not None and len(data) > max_bytes:
            raise ProductionSystemError(f"{label} exceeds the size limit")
        value = json.loads(data, object_pairs_hook=_unique_object)
    except ProductionSystemError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProductionSystemError(f"{label} is not readable strict JSON") from error
    if not isinstance(value, dict):
        raise ProductionSystemError(f"{label} must be a JSON object")
    return data, value


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ProductionSystemError(f"duplicate JSON member: {key}")
        result[key] = value
    return result


def _require_fields(value: Mapping[str, Any], fields: frozenset[str], label: str) -> None:
    actual = frozenset(value)
    if actual != fields:
        raise ProductionSystemError(f"{label} fields do not match the closed schema")


def _require_schema_version(value: Mapping[str, Any], label: str) -> None:
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ProductionSystemError(f"{label} schema_version must be 1")


def _object(value: Any, label: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ProductionSystemError(f"{label} must be an object")
    return value


def _text(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ProductionSystemError(f"{label} must be a non-empty string")
    return value


def _digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or _SHA256.fullmatch(value) is None:
        raise ProductionSystemError(f"{label} must be a lowercase sha256 digest")
    return value


def _manifest_digest(system_manifest: Mapping[str, Any], *, artifact_name: str) -> str:
    harness = _object(system_manifest.get("harness"), "system harness")
    artifact = _object(harness.get(artifact_name), f"system harness {artifact_name} artifact")
    return _digest(artifact.get("sha256"), f"system harness {artifact_name} sha256")


def _verify_manifest_artifact(
    system_manifest: Mapping[str, Any],
    *,
    artifact_name: str,
    artifact_bytes: bytes,
) -> None:
    expected = _manifest_digest(system_manifest, artifact_name=artifact_name)
    actual = hashlib.sha256(artifact_bytes).hexdigest()
    if actual != expected:
        raise ProductionSystemError(
            f"harness {artifact_name} artifact does not match system manifest sha256"
        )


def _guest_executable(value: Any) -> str:
    executable = _text(value, "guest executable")
    if not executable.startswith("/") or "\\" in executable:
        raise ProductionSystemError("guest executable must be an absolute POSIX path")
    if posixpath.normpath(executable) != executable:
        raise ProductionSystemError("guest executable path must be normalized")
    if not executable.startswith(_GUEST_EXECUTABLE_ROOT):
        raise ProductionSystemError("guest executable must be under /usr/local/bin")
    return executable


def _credential_environment(
    configuration: Mapping[str, Any], kind: HarnessKind, provider: str
) -> tuple[str, ...]:
    raw = configuration["credential_environment"]
    if not isinstance(raw, list):
        raise ProductionSystemError("credential_environment must be a JSON array")
    if any(not isinstance(name, str) for name in raw):
        raise ProductionSystemError("credential_environment names must be strings")
    if len(raw) != len(set(raw)):
        raise ProductionSystemError("credential_environment names must be unique")
    allowed = _CREDENTIAL_BY_HARNESS_AND_PROVIDER.get((kind, provider))
    if allowed is None:
        raise ProductionSystemError("model provider is not supported for the harness kind")
    if not allowed:
        if raw:
            raise ProductionSystemError(
                "credential_environment must be empty for the anonymous provider route"
            )
        return ()
    if len(raw) != 1 or raw[0] not in allowed:
        raise ProductionSystemError(
            "credential_environment must contain exactly one credential allowed for the harness and provider"
        )
    return (raw[0],)


def _primary_model_route(
    system_manifest: Mapping[str, Any],
) -> tuple[ModelRoute, str]:
    routing = _object(system_manifest.get("model_routing"), "system model routing")
    routes = routing.get("routes")
    if not isinstance(routes, list):
        raise ProductionSystemError("system model routes must be an array")
    primary = [
        route for route in routes if isinstance(route, Mapping) and route.get("role") == "primary"
    ]
    if len(primary) != 1:
        raise ProductionSystemError("system manifest must contain exactly one primary model route")
    route = primary[0]
    try:
        model_route = ModelRoute(
            route_id=route["id"],
            role=route["role"],
            provider=route["provider"],
            model=route["model"],
            snapshot=route["snapshot"],
            service_tier=route["service_tier"],
        )
    except (KeyError, TypeError, ValueError) as error:
        raise ProductionSystemError("primary model route is malformed") from error
    configuration_sha256 = _digest(
        route.get("configuration_sha256"),
        "primary model route configuration_sha256",
    )
    return model_route, configuration_sha256
