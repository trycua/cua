"""The types and pure helpers behind `Sandbox.create` and friends: lifecycle,
network, workload identity, snapshots, listing, and their validation.

The validators run for every create / connect / pause / update-network call, so an
invalid argument fails with e2b's `InvalidArgumentException` and text. Where e2b's
helpers return a generated REST model, these return a plain dict (or `UNSET`) with
the same field names, which the sandbox implementation then maps onto Cua.
"""

from __future__ import annotations

import inspect
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import (
    Any,
    Callable,
    Dict,
    Iterable,
    List,
    Literal,
    Mapping,
    Optional,
    TypedDict,
    Union,
    cast,
)

from e2b._typing import NotRequired, Unpack
from e2b.api import SandboxCreateResponse
from e2b.api.client import UNSET, Unset
from e2b.connection_config import ApiParams
from e2b.exceptions import InvalidArgumentException
from e2b.paginator import PaginatorBase
from e2b.sandbox.iam import IamTokenPlaceholders, validate_iam_token_name
from e2b.sandbox.mcp import McpServer as BaseMcpServer
from e2b.sandbox.network import ALL_TRAFFIC

MAX_FORK_COUNT = 20


class SandboxState(str, Enum):
    """State of a sandbox."""

    PAUSED = "paused"
    RUNNING = "running"

    def __str__(self) -> str:
        return str(self.value)


def _field(obj: Any, name: str) -> Any:
    """`obj[name]` of a mapping or `obj.name` of an object; `UNSET` if absent."""
    if isinstance(obj, Mapping):
        return obj.get(name, UNSET)
    return getattr(obj, name, UNSET)


def to_sandbox_create_response(sandbox: Any) -> SandboxCreateResponse:
    """Map a created, connected or forked sandbox (an object or mapping with
    `sandbox_id`, `domain`, `envd_version`, `envd_access_token`,
    `traffic_access_token`) to a `SandboxCreateResponse`."""
    domain = _field(sandbox, "domain")
    access = _field(sandbox, "envd_access_token")
    traffic = _field(sandbox, "traffic_access_token")
    return SandboxCreateResponse(
        sandbox_id=_field(sandbox, "sandbox_id"),
        sandbox_domain=domain if isinstance(domain, str) else None,
        envd_version=_field(sandbox, "envd_version"),
        envd_access_token=access if isinstance(access, str) else None,
        traffic_access_token=traffic if isinstance(traffic, str) else None,
    )


def validate_fork_count(count: Any) -> None:
    """Reject a fork count that is not an integer from 1 to 20 (`None` leaves the
    default)."""
    if count is None:
        return
    if isinstance(count, bool) or not isinstance(count, int) or count < 1 or count > MAX_FORK_COUNT:
        raise InvalidArgumentException(f"count must be an integer between 1 and {MAX_FORK_COUNT}")


class GitHubMcpServerConfig(TypedDict):
    """Configuration for a GitHub-based MCP server."""

    run_cmd: str
    """Command to run the MCP server. Must start a stdio-compatible server."""
    install_cmd: NotRequired[str]
    """Command to install dependencies, run in the root of the repository."""
    envs: NotRequired[Dict[str, str]]
    """Environment variables of the MCP process."""


# GitHub servers use keys of the form "github/owner/repo".
GitHubMcpServer = Dict[str, Union[GitHubMcpServerConfig, Any]]

# The catalog servers plus dynamic GitHub servers.
McpServer = Union[BaseMcpServer, GitHubMcpServer]


class SandboxNetworkTransform(TypedDict):
    """Transform applied to egress requests matching a `SandboxNetworkRule`."""

    headers: NotRequired[Dict[str, str]]
    """Headers to inject into the outbound request (override existing ones)."""


@dataclass(frozen=True)
class _SandboxNetworkTransformIam:
    """Workload identity placeholders."""

    tokens: Mapping[str, str]
    """Placeholder of each registered workload token by name:
    `tokens["aws"]` is `"${e2b.identity.tokens.aws}"`. Reading an unregistered
    name raises `InvalidArgumentException`."""


@dataclass(frozen=True)
class SandboxNetworkTransformContext:
    """Context passed to a `SandboxNetworkRule` `transform` callable: literal
    placeholder strings the egress proxy resolves per request."""

    iam: _SandboxNetworkTransformIam
    """Workload identity placeholders."""


SandboxNetworkTransformResolver = Callable[
    [SandboxNetworkTransformContext], SandboxNetworkTransform
]
"""Callable form of `SandboxNetworkRule.transform`, invoked once while the request
is built."""


class SandboxNetworkRule(TypedDict):
    """Per-domain rule applied to egress requests."""

    transform: NotRequired[Union[SandboxNetworkTransform, SandboxNetworkTransformResolver]]
    """A static transform, or a callable receiving a
    `SandboxNetworkTransformContext`."""


SandboxNetworkRules = Dict[str, List[SandboxNetworkRule]]
"""Host (or CIDR / IP) to the ordered rules applied to outbound requests for it.
A host listed here must still appear in `allow_out` to be reachable."""


class SandboxNetworkRuleInfo(TypedDict):
    """A rule as the sandbox info reports it: `transform` is always static."""

    transform: NotRequired[SandboxNetworkTransform]


@dataclass(frozen=True)
class SandboxNetworkSelectorContext:
    """Context passed to `allow_out` / `deny_out` callables."""

    all_traffic: str
    """All-traffic sentinel, `"0.0.0.0/0"`."""

    rules: Mapping[str, List[SandboxNetworkRule]]
    """Rules registered in `SandboxNetworkOpts.rules`."""


SandboxNetworkSelector = Union[
    List[str],
    Callable[[SandboxNetworkSelectorContext], List[str]],
]
"""A list of CIDR blocks / IPs / hostnames, or a callable returning one."""


class SandboxEgressProxyTLSOpts(TypedDict, total=False):
    """TLS settings for the connection to a SOCKS5 egress proxy."""

    server_name: str
    """Name to verify the proxy certificate against and send as SNI (at most 253
    characters)."""

    ca_cert: str
    """PEM CA certificates (at most 8192 bytes); replaces the system trust store."""


class SandboxEgressProxyOpts(TypedDict):
    """SOCKS5 proxy the sandbox's outbound TCP is tunneled through."""

    address: str
    """`host:port` of the proxy."""

    username: NotRequired[str]
    """SOCKS5 username (at most 255 bytes)."""

    password: NotRequired[str]
    """SOCKS5 password; only valid together with `username`."""

    tls: NotRequired[Union[bool, SandboxEgressProxyTLSOpts]]
    """Connect to the proxy over TLS: `True` for the system trust store, or a
    mapping with `server_name` / `ca_cert`."""


class SandboxEgressProxyTLSInfo(TypedDict):
    """TLS status of the egress proxy connection (the CA certificate is never
    returned)."""

    server_name: NotRequired[str]


class SandboxEgressProxyInfo(TypedDict):
    """Egress proxy as the sandbox info reports it, without `password`/`ca_cert`."""

    address: str
    username: NotRequired[str]
    tls: NotRequired[SandboxEgressProxyTLSInfo]


class SandboxNetworkOpts(TypedDict):
    """Sandbox network configuration options."""

    allow_out: NotRequired[SandboxNetworkSelector]
    """Allow outbound traffic to these addresses (all when not given)."""

    deny_out: NotRequired[SandboxNetworkSelector]
    """Deny outbound traffic to these addresses."""

    rules: NotRequired[SandboxNetworkRules]
    """Per-domain transform rules for matching egress HTTP/HTTPS requests."""

    egress_proxy: NotRequired[SandboxEgressProxyOpts]
    """Tunnel outbound TCP through a SOCKS5 proxy."""

    allow_public_traffic: NotRequired[bool]
    """Whether sandbox URLs are publicly accessible. Defaults to True."""

    mask_request_host: NotRequired[str]
    """Custom host mask for sandbox requests; supports `${PORT}`."""

    https_ports: NotRequired[List[int]]
    """Ports whose public URLs connect to the sandbox over HTTPS."""


class SandboxNetworkUpdate(TypedDict, total=False):
    """Subset of `SandboxNetworkOpts` accepted by `Sandbox.update_network`. The
    update replaces all egress rules atomically: omitted fields are cleared."""

    allow_out: SandboxNetworkSelector
    deny_out: SandboxNetworkSelector
    rules: SandboxNetworkRules
    egress_proxy: SandboxEgressProxyOpts
    allow_internet_access: bool
    """`False` is the same as `deny_out=["0.0.0.0/0"]`."""


SandboxIamTokenType = Union[Literal["JWT-SVID"], str]
"""Workload token type; `"JWT-SVID"` is the only one the API accepts today."""


class SandboxIamToken(TypedDict):
    """Workload token definition for sandbox workload identity."""

    audience: str
    """Audience of the workload token, stored exactly as provided."""

    token_type: SandboxIamTokenType
    """Workload token type."""


class SandboxIamOpts(TypedDict, total=False):
    """Sandbox workload identity configuration; a non-empty `tokens` map enables it."""

    tokens: Dict[str, SandboxIamToken]
    """Named workload-token definitions (see `Secret.iam_token`)."""


class SandboxNetworkInfo(TypedDict, total=False):
    """Network configuration as the sandbox info reports it."""

    allow_out: List[str]
    deny_out: List[str]
    rules: Dict[str, List[SandboxNetworkRuleInfo]]
    egress_proxy: SandboxEgressProxyInfo
    allow_public_traffic: bool
    mask_request_host: str
    https_ports: List[int]


SnapshotMode = Literal["full", "filesystem"]
"""What a pause or snapshot persists: `"full"` the filesystem and the memory,
`"filesystem"` only the filesystem (resuming cold-boots)."""


def resolve_snapshot_memory(
    field: str,
    mode: Optional[SnapshotMode],
    keep_memory: Optional[bool] = None,
) -> Union[Unset, bool]:
    """Resolve `mode` (or the deprecated `keep_memory`) into the `memory` flag;
    `UNSET` when neither is given. `field` names the option in error messages."""
    if mode is not None and keep_memory is not None:
        raise InvalidArgumentException(
            f"Pass either {field} or the deprecated keep_memory, not both."
        )
    if mode is None:
        return keep_memory if keep_memory is not None else UNSET
    allowed = ("full", "filesystem")
    if mode not in allowed:
        raise InvalidArgumentException(
            f"{field} must be one of: {', '.join(allowed)} (got {mode!r})."
        )
    return mode == "full"


class SandboxOnTimeoutPause(TypedDict):
    """Object form of `on_timeout` that pauses the sandbox at its timeout."""

    action: Literal["pause"]

    mode: NotRequired[SnapshotMode]
    """What the auto-pause persists. `"filesystem"` cannot be combined with
    `auto_resume`."""

    keep_memory: NotRequired[bool]
    """Deprecated: `False` is `mode="filesystem"`, `True` is `mode="full"`."""


class SandboxOnTimeoutKill(TypedDict):
    """Object form of `on_timeout` that kills the sandbox at its timeout."""

    action: Literal["kill"]


SandboxOnTimeout = Union[Literal["pause", "kill"], SandboxOnTimeoutPause, SandboxOnTimeoutKill]
"""What happens to the sandbox at its timeout: the bare action or the object form."""


class SandboxLifecycle(TypedDict):
    """Sandbox lifecycle configuration: post-timeout behaviour and auto-resume."""

    on_timeout: SandboxOnTimeout
    """`"kill"` terminates the sandbox, `"pause"` pauses it for a later resume."""

    auto_resume: NotRequired[bool]
    """Whether activity resumes a paused sandbox. `True` only with `on_timeout`
    `pause` and not with `mode: "filesystem"`."""


class SandboxInfoLifecycle(TypedDict):
    """Sandbox lifecycle configuration as the sandbox info reports it."""

    on_timeout: Literal["pause", "kill"]
    auto_resume: bool


SandboxOnResume = Literal["restore", "reboot"]
"""How a paused sandbox comes back: `"restore"` restores the memory snapshot,
`"reboot"` cold-boots from disk state alone."""


def resolve_connect_memory(
    on_resume: Optional["SandboxOnResume"],
) -> Union[Unset, bool]:
    """Resolve `on_resume` into the connect request's `memory` flag: `"restore"`
    (the API default) is `UNSET`, `"reboot"` is `False`."""
    if on_resume is None:
        return UNSET
    allowed = ("restore", "reboot")
    if on_resume not in allowed:
        raise InvalidArgumentException(
            f"on_resume must be one of: {', '.join(allowed)} (got {on_resume!r})."
        )
    return False if on_resume == "reboot" else UNSET


def _resolve_network_selector(
    selector: Optional[SandboxNetworkSelector],
    rules: Mapping[str, List[SandboxNetworkRule]],
) -> Optional[List[str]]:
    if selector is None:
        return None

    if callable(selector):
        ctx = SandboxNetworkSelectorContext(all_traffic=ALL_TRAFFIC, rules=rules)
        return list(selector(ctx))

    return list(selector)


def _build_transform_context(
    token_names: Iterable[str],
    *,
    validate: bool,
) -> SandboxNetworkTransformContext:
    return SandboxNetworkTransformContext(
        iam=_SandboxNetworkTransformIam(
            tokens=IamTokenPlaceholders(token_names, validate=validate),
        ),
    )


def _build_client_rules(
    rules: SandboxNetworkRules,
    ctx: SandboxNetworkTransformContext,
) -> Dict[str, List[Dict[str, Any]]]:
    client_rules: Dict[str, List[Dict[str, Any]]] = {}
    for host, host_rules in rules.items():
        converted: List[Dict[str, Any]] = []
        for rule in host_rules:
            transform = rule.get("transform")
            if transform is None:
                converted.append({})
                continue

            if callable(transform):
                transform = transform(ctx)
                # A callable that returns anything but a transform would create the
                # rule without the headers it is for.
                if inspect.isawaitable(transform):
                    if inspect.iscoroutine(transform):
                        transform.close()
                    raise InvalidArgumentException(
                        f"Network transform callable for {host!r} must be "
                        "synchronous, it returned an awaitable. Resolve the "
                        "value before creating the sandbox."
                    )
                if not isinstance(transform, Mapping):
                    raise InvalidArgumentException(
                        f"Network transform callable for {host!r} must return a "
                        f"transform dict, got {type(transform).__name__}."
                    )

            client_transform: Dict[str, Any] = {}
            headers = transform.get("headers")
            if headers:
                client_transform["headers"] = dict(headers)

            converted.append({"transform": client_transform})
        client_rules[host] = converted

    return client_rules


def _build_egress_proxy(
    egress_proxy: SandboxEgressProxyOpts,
) -> Dict[str, Any]:
    """Rebuild the proxy config from the known fields (stray keys never pass)."""
    if not isinstance(egress_proxy, Mapping) or not isinstance(egress_proxy.get("address"), str):
        raise InvalidArgumentException(
            "network egress_proxy must be a dict with a string 'address' "
            "(e.g. 'proxy.example.com:1080')."
        )

    body: Dict[str, Any] = {"address": egress_proxy["address"]}
    if egress_proxy.get("username") is not None:
        body["username"] = egress_proxy["username"]
    if egress_proxy.get("password") is not None:
        body["password"] = egress_proxy["password"]
    tls = egress_proxy.get("tls")
    if tls is not None:
        if isinstance(tls, bool):
            if tls:
                body["tls"] = {"enabled": True}
        elif isinstance(tls, Mapping):
            if "enabled" in tls:
                raise InvalidArgumentException(
                    "network egress_proxy tls takes no 'enabled'; pass "
                    "tls=True or tls=False instead."
                )

            client_tls: Dict[str, Any] = {"enabled": True}
            if tls.get("server_name") is not None:
                client_tls["server_name"] = tls["server_name"]
            if tls.get("ca_cert") is not None:
                client_tls["ca_cert"] = tls["ca_cert"]
            body["tls"] = client_tls
        else:
            raise InvalidArgumentException("network egress_proxy tls must be a bool or a dict.")

    return body


def _build_network_egress(
    network: Mapping[str, Any],
    ctx: SandboxNetworkTransformContext,
) -> Dict[str, Any]:
    """The egress fields shared by create and update: `allow_out`, `deny_out`,
    `egress_proxy` and per-host `rules` (host to the list of rule dicts)."""
    rules = network.get("rules") or {}
    allow_out = _resolve_network_selector(network.get("allow_out"), rules)
    deny_out = _resolve_network_selector(network.get("deny_out"), rules)
    # `is not None`, not truthiness: an empty dict must reach the builder and fail
    # loudly instead of silently disabling tunneling.
    egress_proxy = network.get("egress_proxy")

    body: Dict[str, Any] = {}
    if allow_out is not None:
        body["allow_out"] = allow_out
    if deny_out is not None:
        body["deny_out"] = deny_out
    if egress_proxy is not None:
        body["egress_proxy"] = _build_egress_proxy(egress_proxy)
    if "rules" in network and network["rules"] is not None:
        body["rules"] = _build_client_rules(network["rules"], ctx)

    return body


def build_network_config(
    network: Optional[SandboxNetworkOpts],
    iam: Optional[Dict[str, Any]] = None,
) -> Optional[Dict[str, Any]]:
    """Resolve a `SandboxNetworkOpts` into the dict the create call carries.

    `iam` is the built workload identity config of the same request
    (`build_iam_config`); its token names are what `transform` callables may
    reference.
    """
    if network is None:
        return None

    token_names: List[str] = []
    if iam is not None:
        tokens = _field(iam, "tokens")
        if not isinstance(tokens, Unset):
            token_names = list(tokens)

    body = _build_network_egress(
        network,
        _build_transform_context(token_names, validate=True),
    )
    if "allow_public_traffic" in network:
        body["allow_public_traffic"] = network["allow_public_traffic"]
    if "mask_request_host" in network:
        body["mask_request_host"] = network["mask_request_host"]
    if network.get("https_ports") is not None:
        body["https_ports"] = list(network["https_ports"])

    return body


def build_iam_config(
    iam: Optional[SandboxIamOpts],
) -> Optional[Dict[str, Any]]:
    """Resolve a `SandboxIamOpts` into `{"tokens": {name: {"audience", "token_type"}}}`.

    `None` for a config with no tokens: only a non-empty tokens map enables
    workload identity.
    """
    if iam is None:
        return None

    tokens = iam.get("tokens")
    if not tokens:
        return None

    client_tokens: Dict[str, Dict[str, str]] = {}
    for name, token in tokens.items():
        if (
            not isinstance(token, dict)
            or not isinstance(token.get("audience"), str)
            or not isinstance(token.get("token_type"), str)
        ):
            raise InvalidArgumentException(
                f"iam token {name!r} must be a dict with string 'audience' and "
                "'token_type' values (snake_case 'token_type', not 'tokenType')."
            )

        # A placeholder is the only way to consume a token, so a name that cannot
        # appear in one is rejected where it is registered.
        validate_iam_token_name(name)

        client_tokens[name] = {
            "audience": token["audience"],
            "token_type": token["token_type"],
        }

    return {"tokens": client_tokens}


@dataclass(frozen=True)
class SandboxLifecycleBody:
    """Lifecycle fields of a create request: `auto_pause`, `auto_pause_memory`
    and `auto_resume` (`{"enabled": bool}`), each `UNSET` when not chosen."""

    auto_pause: Union[Unset, bool]
    auto_pause_memory: Union[Unset, bool]
    auto_resume: Union[Unset, Dict[str, Any]]


def build_lifecycle_config(
    lifecycle: Optional[SandboxLifecycle],
) -> SandboxLifecycleBody:
    """Resolve a `SandboxLifecycle` into the create-request lifecycle fields.

    `auto_pause` stays unset without an `on_timeout` (sending `False` would read as
    an explicit kill and override the default the platform owns);
    `auto_pause_memory` stays unset unless `mode` was chosen.
    """
    on_timeout_raw = lifecycle.get("on_timeout") if lifecycle else None
    # A missing on_timeout (or an explicit None) is not a choice of kill; it only
    # resolves to kill semantics for the validation below.
    on_timeout_configured = on_timeout_raw is not None
    if isinstance(on_timeout_raw, dict):
        on_timeout = on_timeout_raw.get("action")
        pause_opts = cast(Dict[str, Any], on_timeout_raw)
    else:
        on_timeout = on_timeout_raw if on_timeout_configured else "kill"
        pause_opts = {}

    allowed_actions = ("pause", "kill")
    if on_timeout_configured and on_timeout not in allowed_actions:
        field = 'on_timeout["action"]' if isinstance(on_timeout_raw, dict) else "on_timeout"
        raise InvalidArgumentException(
            f"{field} must be one of: {', '.join(allowed_actions)} (got {on_timeout!r})."
        )

    if ("mode" in pause_opts or "keep_memory" in pause_opts) and on_timeout != "pause":
        raise InvalidArgumentException("mode is only allowed when on_timeout action is 'pause'.")
    memory = resolve_snapshot_memory(
        'on_timeout["mode"]', pause_opts.get("mode"), pause_opts.get("keep_memory")
    )
    auto_resume = lifecycle.get("auto_resume") if lifecycle else None

    if auto_resume and on_timeout != "pause":
        raise InvalidArgumentException(
            "auto_resume can only be True when on_timeout action is 'pause'."
        )

    if memory is False and auto_resume:
        raise InvalidArgumentException(
            "auto_resume: True is not a valid value with mode: 'filesystem' - "
            "a filesystem-only snapshot cannot be auto-resumed by traffic and "
            "must be resumed explicitly using Sandbox.connect()."
        )

    return SandboxLifecycleBody(
        auto_pause=(on_timeout == "pause") if on_timeout_configured else UNSET,
        auto_pause_memory=memory if on_timeout == "pause" else UNSET,
        auto_resume=({"enabled": auto_resume} if auto_resume is not None else UNSET),
    )


def build_network_update_body(
    network: SandboxNetworkUpdate,
) -> Dict[str, Any]:
    """Resolve a `SandboxNetworkUpdate` into the update request body (only the
    fields that are set)."""
    egress = _build_network_egress(network, _build_transform_context([], validate=False))

    body: Dict[str, Any] = {}
    for key in ("allow_out", "deny_out", "egress_proxy", "rules"):
        if key in egress:
            body[key] = egress[key]
    if "allow_internet_access" in network:
        body["allow_internet_access"] = network["allow_internet_access"]

    return body


def _from_client_egress_proxy(
    egress_proxy: Any,
) -> Optional[SandboxEgressProxyInfo]:
    """The proxy as reported: `password` and the CA certificate are dropped (the
    API never returns them) and "no proxy" becomes an absent key."""
    if not isinstance(egress_proxy, Mapping):
        return None

    result: SandboxEgressProxyInfo = {"address": egress_proxy["address"]}
    if "username" in egress_proxy:
        result["username"] = egress_proxy["username"]
    tls = egress_proxy.get("tls")
    if isinstance(tls, Mapping) and tls.get("enabled") is True:
        tls_info: SandboxEgressProxyTLSInfo = {}
        if "server_name" in tls:
            tls_info["server_name"] = tls["server_name"]
        result["tls"] = tls_info

    return result


def from_client_network_config(
    network: Union[Unset, Mapping[str, Any], None],
) -> Optional[SandboxNetworkInfo]:
    """Map a network config (a mapping with the REST field names, or `UNSET`) to
    `SandboxNetworkInfo`."""
    if not isinstance(network, Mapping):
        return None

    result: SandboxNetworkInfo = {}

    if "allow_out" in network:
        result["allow_out"] = list(network["allow_out"])
    if "deny_out" in network:
        result["deny_out"] = list(network["deny_out"])
    if "rules" in network:
        result["rules"] = cast(Dict[str, List[SandboxNetworkRuleInfo]], dict(network["rules"]))
    egress_proxy = _from_client_egress_proxy(network.get("egress_proxy"))
    if egress_proxy is not None:
        result["egress_proxy"] = egress_proxy
    if "allow_public_traffic" in network:
        result["allow_public_traffic"] = network["allow_public_traffic"]
    if "mask_request_host" in network:
        result["mask_request_host"] = network["mask_request_host"]
    if "https_ports" in network:
        result["https_ports"] = list(network["https_ports"])

    return result


def from_client_lifecycle(
    lifecycle: Union[Unset, Mapping[str, Any], None],
) -> Optional[SandboxInfoLifecycle]:
    """Map a lifecycle (a mapping with `on_timeout` and `auto_resume`, or
    `UNSET`) to `SandboxInfoLifecycle`."""
    if not isinstance(lifecycle, Mapping):
        return None

    result: SandboxInfoLifecycle = {
        "on_timeout": cast(Literal["pause", "kill"], lifecycle["on_timeout"]),
        "auto_resume": lifecycle["auto_resume"],
    }

    return result


@dataclass
class SandboxInfo:
    """Information about a sandbox."""

    sandbox_id: str
    """Sandbox ID."""
    sandbox_domain: Optional[str]
    """Domain where the sandbox is hosted."""
    template_id: str
    """Template ID."""
    name: Optional[str]
    """Template name."""
    metadata: Dict[str, str]
    """Saved sandbox metadata."""
    started_at: datetime
    """Sandbox start time."""
    end_at: datetime
    """Sandbox expiration date."""
    state: SandboxState
    """Sandbox state."""
    cpu_count: int
    """Sandbox CPU count."""
    memory_mb: int
    """Sandbox memory size in MiB."""
    envd_version: str
    """Envd version."""
    allow_internet_access: Optional[bool] = None
    """Whether internet access was explicitly enabled or disabled for the sandbox."""
    network: Optional[SandboxNetworkInfo] = None
    """Sandbox network configuration."""
    lifecycle: Optional[SandboxInfoLifecycle] = None
    """Sandbox lifecycle configuration."""
    volume_mounts: List[Dict[str, str]] = field(default_factory=list)
    """Volume mounts of the sandbox, as `{"name", "path"}`."""


SandboxListOrder = Literal["asc", "desc"]
"""Sort order for listing sandboxes by start time."""


@dataclass
class SandboxQuery:
    """Query parameters for listing sandboxes."""

    metadata: Optional[dict[str, str]] = None
    """Filter sandboxes by metadata."""

    state: Optional[list[SandboxState]] = None
    """Filter sandboxes by state."""

    started_after: Optional[datetime] = None
    """Filter to sandboxes started at or after this time (naive means local time)."""

    template: Optional[str] = None
    """Filter sandboxes by a template ID or name."""


@dataclass
class SandboxMetrics:
    """Sandbox metrics."""

    cpu_count: int
    """Number of CPUs."""
    cpu_used_pct: float
    """CPU usage percentage."""
    disk_total: int
    """Total disk space in bytes."""
    disk_used: int
    """Disk used in bytes."""
    mem_total: int
    """Total memory in bytes."""
    mem_used: int
    """Memory used in bytes."""
    mem_cache: int
    """Cached memory (page cache) in bytes."""
    timestamp: datetime
    """Timestamp of the metric entry."""


@dataclass
class SnapshotInfo:
    """Information about a snapshot."""

    snapshot_id: str
    """Snapshot identifier; usable as `template=` of `Sandbox.create()`."""
    names: List[str] = field(default_factory=list)
    """Full names of the snapshot."""


class SnapshotPaginatorBase(PaginatorBase[SnapshotInfo, ApiParams]):
    def __init__(
        self,
        sandbox_id: Optional[str] = None,
        limit: Optional[int] = None,
        next_token: Optional[str] = None,
        name: Optional[str] = None,
        **opts: Unpack[ApiParams],
    ):
        super().__init__(limit=limit, next_token=next_token, **opts)
        self.sandbox_id = sandbox_id
        self.name = name


class SandboxPaginatorBase(PaginatorBase[SandboxInfo, ApiParams]):
    def __init__(
        self,
        query: Optional[SandboxQuery] = None,
        limit: Optional[int] = None,
        next_token: Optional[str] = None,
        order: Optional[SandboxListOrder] = None,
        **opts: Unpack[ApiParams],
    ):
        super().__init__(limit=limit, next_token=next_token, **opts)
        self.query = query
        self.order = order
