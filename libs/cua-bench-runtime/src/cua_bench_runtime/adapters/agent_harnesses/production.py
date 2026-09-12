"""Side-effect-free production harness launch and telemetry contracts."""

from __future__ import annotations

import hashlib
import ipaddress
import json
import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path, PurePosixPath
from types import MappingProxyType
from typing import Any


class HarnessKind(StrEnum):
    CODEX = "codex"
    CLAUDE_CODE = "claude-code"
    OPENCODE = "opencode"


class TelemetryTrust(StrEnum):
    """Authority of a normalized fact, never the success of the trial."""

    BENCHMARK_OWNED = "benchmark_owned"
    HARNESS_REPORTED = "harness_reported"
    PROVIDER_REPORTED = "provider_reported"
    UNAVAILABLE = "unavailable"


_SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/-]*$")
_SECRET_NAME = re.compile(
    r"(?:TOKEN|SECRET|PASSWORD|PASSWD|API_KEY|APIKEY|CREDENTIAL|AUTHORIZATION|COOKIE)",
    re.IGNORECASE,
)
_SECRET_ARGUMENT = re.compile(
    r"^--?(?:api[-_]?key|token|secret|password|passwd|credential|authorization)(?:=|$)",
    re.IGNORECASE,
)
_CLOSED_PATH = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
_TELEMETRY_EVENT_TYPES = frozenset(
    {
        "assistant",
        "error",
        "message",
        "result",
        "step_finish",
        "step_start",
        "system",
        "text",
        "turn.completed",
        "turn.failed",
    }
)
_CUMULATIVE_USAGE_EVENT_TYPE = {
    HarnessKind.CODEX: "turn.completed",
    HarnessKind.CLAUDE_CODE: "result",
    HarnessKind.OPENCODE: "step_finish",
}
_MAX_TOKEN_COUNT = 1_000_000_000_000
CLAUDE_CODE_PINNED_VERSION = "2.1.233"
CLAUDE_CODE_OAUTH_CREDENTIAL = "CLAUDE_CODE_OAUTH_TOKEN"
_PROVIDER_HOSTNAME = re.compile(
    r"^(?=.{1,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)*"
    r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$"
)
_LOCALHOST_NO_PROXY = "localhost,127.0.0.1"


def _nonempty(value: str, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a non-empty string")
    return value


def _absolute(path: Path, label: str) -> Path:
    # Guest macOS paths remain POSIX paths even when the host-side contract
    # tests execute on Windows.  Do not reinterpret a PurePosixPath using the
    # host filesystem flavour.
    candidate = path if isinstance(path, PurePosixPath) else Path(path)
    if not candidate.is_absolute():
        raise ValueError(f"{label} must be absolute")
    return path


@dataclass(frozen=True)
class ModelRoute:
    """An explicit, manifest-bound model request.

    There is intentionally no default model.  Both identifiers are retained:
    ``route_id`` is the benchmark/system-manifest identity and ``model`` is the
    exact value supplied to the harness CLI.
    """

    route_id: str
    role: str
    provider: str
    model: str
    snapshot: str
    service_tier: str

    def __post_init__(self) -> None:
        for value, label in (
            (self.route_id, "route_id"),
            (self.role, "role"),
            (self.provider, "provider"),
            (self.model, "model"),
            (self.snapshot, "snapshot"),
            (self.service_tier, "service_tier"),
        ):
            _nonempty(value, label)
            if not _SAFE_ID.fullmatch(value):
                raise ValueError(f"{label} contains unsupported characters")
        if self.role not in {"primary", "subagent", "fallback"}:
            raise ValueError("role must be primary, subagent, or fallback")


@dataclass(frozen=True)
class NativeMcpDriver:
    """A harness-native MCP server pointed at the protected trial socket."""

    socket_path: Path
    executable: str = "cua-driver"
    server_name: str = "cua"

    def __post_init__(self) -> None:
        _absolute(self.socket_path, "driver socket")
        _nonempty(self.executable, "driver executable")
        _nonempty(self.server_name, "driver server name")
        if not _SAFE_ID.fullmatch(self.server_name):
            raise ValueError("driver server name contains unsupported characters")

    @property
    def command(self) -> tuple[str, ...]:
        return (self.executable, "mcp", "--socket", str(self.socket_path))


@dataclass(frozen=True)
class HarnessRenderContext:
    home: Path
    workspace: Path
    artifacts: Path
    brief: Path
    model_route: ModelRoute
    driver: NativeMcpDriver

    def __post_init__(self) -> None:
        for path, label in (
            (self.home, "harness home"),
            (self.workspace, "workspace"),
            (self.artifacts, "artifacts"),
            (self.brief, "brief"),
        ):
            _absolute(path, label)
        if self.workspace.is_relative_to(self.home):
            raise ValueError("target workspace must not be inside the harness home")
        if self.artifacts.is_relative_to(self.home):
            raise ValueError("artifacts must not be inside the harness home")


@dataclass(frozen=True)
class RenderedConfig:
    path: Path
    content: bytes


@dataclass(frozen=True)
class BundledSkillFile:
    """One validated, immutable file from a manifest-bound skill bundle."""

    path: PurePosixPath
    content: bytes

    def __post_init__(self) -> None:
        if (
            not isinstance(self.path, PurePosixPath)
            or self.path.is_absolute()
            or self.path.as_posix() == "."
            or ".." in self.path.parts
        ):
            raise ValueError("bundled skill file path must be safe and relative")
        if not isinstance(self.content, bytes):
            raise ValueError("bundled skill file content must be bytes")


@dataclass(frozen=True)
class ProxyConfiguration:
    """Frozen, non-secret boundary for the benchmark CONNECT proxy."""

    endpoint: str
    provider_allowlist: tuple[str, ...]
    provider_allowlist_sha256: str
    implementation_sha256: str

    def __post_init__(self) -> None:
        host, port = _host_at_port(self.endpoint, "proxy endpoint")
        try:
            address = ipaddress.IPv4Address(host)
        except ipaddress.AddressValueError as error:
            raise ValueError("proxy endpoint host must be a literal IPv4 address") from error
        if (
            address.is_loopback
            or address.is_unspecified
            or address.is_multicast
            or address.is_link_local
            or address == ipaddress.IPv4Address("255.255.255.255")
        ):
            raise ValueError("proxy endpoint IPv4 address is not usable")
        if port in {53, 5353}:
            raise ValueError("proxy endpoint must not use a name-service port")
        if self.endpoint != f"{address}@{port}":
            raise ValueError("proxy endpoint must use canonical IPv4@port syntax")
        if not self.provider_allowlist:
            raise ValueError("provider allowlist must not be empty")
        canonical: list[str] = []
        for entry in self.provider_allowlist:
            provider_host, provider_port = _host_at_port(entry, "provider allowlist entry")
            if not _PROVIDER_HOSTNAME.fullmatch(provider_host):
                raise ValueError("provider allowlist hosts must be lowercase DNS hostnames")
            try:
                ipaddress.ip_address(provider_host)
            except ValueError:
                pass
            else:
                raise ValueError("provider allowlist hosts must not be IP addresses")
            canonical.append(f"{provider_host}@{provider_port}")
        if tuple(canonical) != self.provider_allowlist:
            raise ValueError("provider allowlist entries must use canonical hostname@port syntax")
        if tuple(sorted(set(canonical))) != self.provider_allowlist:
            raise ValueError("provider allowlist must be unique and sorted")
        expected_digest = canonical_provider_allowlist_sha256(self.provider_allowlist)
        if self.provider_allowlist_sha256 != expected_digest:
            raise ValueError("provider allowlist sha256 does not match canonical entries")
        if re.fullmatch(r"[a-f0-9]{64}", self.implementation_sha256) is None:
            raise ValueError("proxy implementation sha256 must be lowercase hex")

    @property
    def url(self) -> str:
        host, port = self.endpoint.rsplit("@", 1)
        return f"http://{host}:{port}"


def canonical_provider_allowlist_sha256(entries: Sequence[str]) -> str:
    # The configuration uses ``hostname@port`` to avoid ambiguity with host
    # syntax, while the proxy authorizes the corresponding HTTP CONNECT
    # authorities.  Bind the latter canonical semantic representation.
    authorities = [entry.rsplit("@", 1)[0] + ":" + entry.rsplit("@", 1)[1] for entry in entries]
    canonical = json.dumps(authorities, ensure_ascii=True, separators=(",", ":")).encode("ascii")
    return hashlib.sha256(canonical).hexdigest()


def _host_at_port(value: str, label: str) -> tuple[str, int]:
    if not isinstance(value, str) or value.count("@") != 1:
        raise ValueError(f"{label} must use host@port syntax")
    host, port_text = value.split("@")
    if not host or not port_text.isascii() or not port_text.isdecimal():
        raise ValueError(f"{label} must use host@port syntax")
    port = int(port_text)
    if not 1 <= port <= 65535 or port_text != str(port):
        raise ValueError(f"{label} port must be canonical and between 1 and 65535")
    return host, port


@dataclass(frozen=True)
class LaunchContract:
    """Closed launch input safe to persist as benchmark evidence."""

    argv: tuple[str, ...]
    cwd: Path
    environment: Mapping[str, str]
    stdin_path: Path
    config_files: tuple[RenderedConfig, ...]
    requested_route: ModelRoute
    driver: NativeMcpDriver

    def __post_init__(self) -> None:
        if not self.argv or any(not isinstance(arg, str) or not arg for arg in self.argv):
            raise ValueError("launch argv must contain non-empty strings")
        _absolute(self.cwd, "launch cwd")
        _absolute(self.stdin_path, "launch stdin")
        if any(_SECRET_ARGUMENT.search(arg) for arg in self.argv):
            raise ValueError("secret-bearing launch argument is forbidden")
        for name, value in self.environment.items():
            if _SECRET_NAME.search(name):
                raise ValueError(f"secret-bearing environment variable is forbidden: {name}")
            if not isinstance(value, str):
                raise ValueError(f"environment value for {name} must be a string")
        serialized = (*self.argv, *self.environment.values())
        if any("Bearer " in value for value in serialized):
            raise ValueError("launch contract must not contain bearer credentials")
        object.__setattr__(
            self,
            "environment",
            MappingProxyType(dict(sorted(self.environment.items()))),
        )


@dataclass(frozen=True)
class NormalizedTelemetry:
    """Prompt-free facts extracted at the harness telemetry boundary."""

    harness: HarnessKind
    event_type: str
    requested_route_id: str
    requested_role: str
    requested_provider: str
    requested_model: str
    requested_snapshot: str
    requested_service_tier: str
    requested_model_trust: TelemetryTrust = TelemetryTrust.BENCHMARK_OWNED
    reported_model: str | None = None
    reported_model_trust: TelemetryTrust = TelemetryTrust.UNAVAILABLE
    input_tokens: int | None = None
    output_tokens: int | None = None
    cache_read_tokens: int | None = None
    cache_write_tokens: int | None = None
    includes_subagents: bool | None = None
    usage_is_cumulative_total: bool = False
    usage_trust: TelemetryTrust = TelemetryTrust.UNAVAILABLE
    served_model: str | None = None
    served_model_trust: TelemetryTrust = TelemetryTrust.UNAVAILABLE
    terminal_failure: str | None = None


@dataclass(frozen=True)
class ProductionHarnessSpec:
    kind: HarnessKind
    executable: str

    def __post_init__(self) -> None:
        _nonempty(self.executable, "harness executable")

    def render(
        self,
        context: HarnessRenderContext,
        *,
        proxy: ProxyConfiguration | None = None,
        skill_files: tuple[BundledSkillFile, ...] = (),
    ) -> LaunchContract:
        """Render isolated CLI, environment, and native MCP configuration."""

        home = context.home
        driver_command = list(context.driver.command)
        model = context.model_route.model
        if self.kind is HarnessKind.CODEX:
            config_path = home / ".codex" / "config.toml"
            content = (
                f"model = {json.dumps(model, ensure_ascii=True)}\n"
                f"[mcp_servers.{context.driver.server_name}]\n"
                f"command = {json.dumps(driver_command[0], ensure_ascii=True)}\n"
                f"args = {json.dumps(driver_command[1:], separators=(',', ':'))}\n"
            ).encode("utf-8")
            argv = (
                self.executable,
                "exec",
                "--model",
                model,
                "--json",
                "--ephemeral",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "-",
            )
            environment = {
                "HOME": str(home),
                "CODEX_HOME": str(home / ".codex"),
                "PATH": _CLOSED_PATH,
            }
        elif self.kind is HarnessKind.CLAUDE_CODE:
            config_path = home / ".claude" / "mcp.json"
            content = _json_bytes(
                {
                    "mcpServers": {
                        context.driver.server_name: {
                            "command": driver_command[0],
                            "args": driver_command[1:],
                        }
                    }
                }
            )
            argv = (
                self.executable,
                "--model",
                model,
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "bypassPermissions",
                "--allow-dangerously-skip-permissions",
                "--strict-mcp-config",
                "--mcp-config",
                str(config_path),
                "--no-session-persistence",
                "--print",
            )
            environment = {
                "HOME": str(home),
                "CLAUDE_CONFIG_DIR": str(home / ".claude"),
                "PATH": _CLOSED_PATH,
            }
        else:
            config_path = home / ".config" / "opencode" / "opencode.json"
            content = _json_bytes(
                {
                    "$schema": "https://opencode.ai/config.json",
                    "mcp": {
                        context.driver.server_name: {
                            "type": "local",
                            "command": driver_command,
                            "enabled": True,
                        }
                    },
                }
            )
            argv = (
                self.executable,
                "--pure",
                "run",
                "--model",
                f"{context.model_route.provider}/{model}",
                "--format",
                "json",
                "--auto",
            )
            environment = {
                "HOME": str(home),
                "PATH": _CLOSED_PATH,
                "XDG_CONFIG_HOME": str(home / ".config"),
                "XDG_DATA_HOME": str(home / ".local" / "share"),
                "XDG_CACHE_HOME": str(home / ".cache"),
            }
        if proxy is not None:
            environment.update(
                {
                    "HTTPS_PROXY": proxy.url,
                    "NO_PROXY": _LOCALHOST_NO_PROXY,
                    "https_proxy": proxy.url,
                    "no_proxy": _LOCALHOST_NO_PROXY,
                }
            )
        configs = [RenderedConfig(path=config_path, content=content)]
        if skill_files:
            if self.kind is HarnessKind.CODEX:
                skill_root = home / ".agents" / "skills" / "cua-driver"
            elif self.kind is HarnessKind.CLAUDE_CODE:
                skill_root = home / ".claude" / "skills" / "cua-driver"
            else:
                skill_root = home / ".config" / "opencode" / "skills" / "cua-driver"
            configs.extend(
                RenderedConfig(path=skill_root / file.path, content=file.content)
                for file in skill_files
            )
        if any(not config.path.is_relative_to(home) for config in configs):
            raise ValueError("rendered harness config escapes isolated home")
        if len({config.path for config in configs}) != len(configs):
            raise ValueError("rendered harness config paths must be unique")
        return LaunchContract(
            argv=argv,
            cwd=context.workspace,
            environment=environment,
            stdin_path=context.brief,
            config_files=tuple(configs),
            requested_route=context.model_route,
            driver=context.driver,
        )

    def normalize_telemetry(
        self,
        event: Mapping[str, Any],
        requested_route: ModelRoute,
    ) -> NormalizedTelemetry:
        """Keep only non-content routing and accounting fields from CLI JSON.

        Harness formats evolve, so extraction accepts the known flat and nested
        metadata shapes.  Unknown fields (including prompts, responses, tool
        arguments, and screen content) are intentionally dropped.
        """

        event_type = _telemetry_event_type(event)
        message = _mapping(event.get("message"))
        reported_model = _reported_model(event, message, requested_route)
        usage_is_cumulative_total = event_type == _CUMULATIVE_USAGE_EVENT_TYPE[self.kind]
        usage: Mapping[str, Any] = {}
        if usage_is_cumulative_total:
            usage = _mapping(event.get("usage"))
            if not usage:
                usage = _mapping(message.get("usage"))
            if not usage and self.kind is HarnessKind.OPENCODE:
                usage = _mapping(_mapping(event.get("part")).get("tokens"))
        input_tokens = _token_count(usage, ("input_tokens", "inputTokens", "input"))
        output_tokens = _token_count(usage, ("output_tokens", "outputTokens", "output"))
        cache_read_tokens = _token_count(
            usage,
            (
                "cache_read_input_tokens",
                "cached_input_tokens",
                "cache_read_tokens",
                "cacheReadTokens",
            ),
        )
        if cache_read_tokens is None and self.kind is HarnessKind.OPENCODE:
            cache_read_tokens = _token_count(_mapping(usage.get("cache")), ("read",))
        cache_write_tokens = _token_count(
            usage,
            (
                "cache_creation_input_tokens",
                "cache_write_input_tokens",
                "cache_write_tokens",
                "cacheWriteTokens",
            ),
        )
        if cache_write_tokens is None and self.kind is HarnessKind.OPENCODE:
            cache_write_tokens = _token_count(_mapping(usage.get("cache")), ("write",))
        has_usage = any(
            value is not None
            for value in (
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            )
        )
        return NormalizedTelemetry(
            harness=self.kind,
            event_type=event_type,
            requested_route_id=requested_route.route_id,
            requested_role=requested_route.role,
            requested_provider=requested_route.provider,
            requested_model=requested_route.model,
            requested_snapshot=requested_route.snapshot,
            requested_service_tier=requested_route.service_tier,
            reported_model=reported_model,
            reported_model_trust=(
                TelemetryTrust.HARNESS_REPORTED
                if reported_model is not None
                else TelemetryTrust.UNAVAILABLE
            ),
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_read_tokens=cache_read_tokens,
            cache_write_tokens=cache_write_tokens,
            # A terminal CLI total covers this harness process.  It does not
            # prove that independently executed subagent calls are included.
            includes_subagents=False if has_usage else None,
            usage_is_cumulative_total=usage_is_cumulative_total,
            usage_trust=(
                TelemetryTrust.HARNESS_REPORTED if has_usage else TelemetryTrust.UNAVAILABLE
            ),
            terminal_failure=_terminal_failure(self.kind, event, event_type),
        )


def production_harness(
    harness: HarnessKind | str,
    *,
    executable: str | None = None,
) -> ProductionHarnessSpec:
    """Return a supported spec without consulting PATH or the environment."""

    try:
        kind = HarnessKind(harness)
    except ValueError as error:
        supported = ", ".join(item.value for item in HarnessKind)
        raise ValueError(
            f"unsupported production harness {harness!r}; expected {supported}"
        ) from error
    defaults = {
        HarnessKind.CODEX: "codex",
        HarnessKind.CLAUDE_CODE: "claude",
        HarnessKind.OPENCODE: "opencode",
    }
    return ProductionHarnessSpec(
        kind=kind,
        executable=defaults[kind] if executable is None else executable,
    )


def _json_bytes(value: Mapping[str, Any]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _first_text(*values: Any) -> str | None:
    return next((value for value in values if isinstance(value, str) and value), None)


def _telemetry_event_type(event: Mapping[str, Any]) -> str:
    candidate = _first_text(event.get("type"), event.get("event"))
    return candidate if candidate in _TELEMETRY_EVENT_TYPES else "unknown"


def _reported_model(
    event: Mapping[str, Any],
    message: Mapping[str, Any],
    requested_route: ModelRoute,
) -> str | None:
    candidate = _first_text(event.get("model"), message.get("model"))
    return candidate if candidate == requested_route.model else None


def _terminal_failure(
    kind: HarnessKind,
    event: Mapping[str, Any],
    event_type: str,
) -> str | None:
    """Classify terminal provider failures without retaining their text.

    CLI error messages can contain provider details and must never cross the
    telemetry boundary.  Matching is therefore deliberately closed and the
    returned value is one of a small benchmark-owned vocabulary.
    """

    is_terminal_error = (
        (
            kind is HarnessKind.CLAUDE_CODE
            and event_type == "result"
            and (event.get("is_error") is True or event.get("terminal_reason") == "api_error")
        )
        or (kind is HarnessKind.CODEX and event_type in {"error", "turn.failed"})
        or (
            kind is HarnessKind.OPENCODE
            and event_type == "result"
            and (event.get("is_error") is True or event.get("error") is not None)
        )
    )
    if not is_terminal_error:
        return None

    error = _mapping(event.get("error"))
    message = _first_text(
        event.get("message"),
        error.get("message"),
        event.get("result"),
    )
    lowered = message.casefold() if message is not None else ""
    status = event.get("api_error_status")
    if status in {401, 403} or lowered.startswith("not logged in"):
        return "authentication_unavailable"
    if "usage limit" in lowered or "quota" in lowered or "purchase more credits" in lowered:
        return "usage_limit"
    if status == 429 or "rate limit" in lowered or "rate-limit" in lowered:
        return "rate_limited"
    return "provider_error"


def _token_count(usage: Mapping[str, Any], keys: Sequence[str]) -> int | None:
    for key in keys:
        value = usage.get(key)
        if (
            isinstance(value, int)
            and not isinstance(value, bool)
            and 0 <= value <= _MAX_TOKEN_COUNT
        ):
            return value
    return None
