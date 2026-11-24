"""AsyncSandbox: an e2b sandbox that is a Cua Space (a Linux VM)."""

from __future__ import annotations

import asyncio
import concurrent.futures
import functools
import json
import logging
import re
import secrets
import shlex
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from typing import Dict, List, Optional

from e2b import _wire
from e2b._backend import NAME_PREFIX, get_backend
from e2b._errors import _is_cua_error, map_error
from e2b._info import (
    DEFAULT_CPUS,
    DEFAULT_MEMORY_MB,
    ENVD_VERSION,
    network_info,
    parse_ts,
    sandbox_info,
)
from e2b.connection_config import ConnectionConfig
from e2b.exceptions import (
    InvalidArgumentException,
    SandboxException,
    SandboxNotFoundException,
    format_sandbox_unavailable_exception,
)
from e2b.sandbox.commands.command_handle import CommandExitException
from e2b.sandbox.network import ALL_TRAFFIC
from e2b.sandbox.sandbox_api import (
    SandboxInfo,
    SandboxMetrics,
    SandboxNetworkUpdate,
    SandboxQuery,
    SandboxState,
    SnapshotInfo,
    build_iam_config,
    build_lifecycle_config,
    build_network_config,
    build_network_update_body,
    resolve_connect_memory,
    resolve_snapshot_memory,
    validate_fork_count,
)
from e2b.sandbox.utils import class_method_variant
from e2b.sandbox_async.commands.command import Commands, _bounded, guest_user
from e2b.sandbox_async.commands.pty import Pty
from e2b.sandbox_async.filesystem.filesystem import Filesystem
from e2b.sandbox_async.git import Git
from e2b.sandbox_async.paginator import AsyncSandboxPaginator, AsyncSnapshotPaginator
from e2b.sandbox_async.sandbox_api import SandboxApi

logger = logging.getLogger("e2b")

DEFAULT_SANDBOX_TIMEOUT = 300  # SandboxBase.default_sandbox_timeout
MAX_LIFETIME = 86400  # set_timeout is capped at 24 hours (e2b's longest plan)


_parse_ts = parse_ts


def _aware(t: datetime) -> datetime:
    return t if t.tzinfo else t.replace(tzinfo=timezone.utc)


class _Gone:
    """Stands in for a killed sandbox: every call fails as e2b's do once a
    sandbox is gone (a `TimeoutException`)."""

    def __init__(self, sandbox_id: str):
        self.sandbox_id = sandbox_id

    def __getattr__(self, name):
        async def gone(*args, **kwargs):
            raise format_sandbox_unavailable_exception("The sandbox was not found")

        return gone


class _Guard:
    """What a sandbox's own calls go through: a paused sandbox with
    `lifecycle={"auto_resume": True}` resumes first, and once the lifetime
    has passed a failure reads as e2b's (`TimeoutException`, "The sandbox
    was not found")."""

    def __init__(self, machine, sandbox):
        self._m = machine
        self._sb = sandbox
        self.sandbox_id = machine.sandbox_id

    def __getattr__(self, name):
        attr = getattr(self._m, name)
        if not callable(attr):
            return attr

        async def call(*args, **kwargs):
            if self._sb._lifecycle.get("auto_resume"):
                await self._sb._resume_if_paused()
            elif self._sb._paused and not await self._sb._refresh_paused():
                raise format_sandbox_unavailable_exception("The sandbox was not found")
            try:
                return await attr(*args, **kwargs)
            except Exception as e:  # noqa: BLE001
                if self._sb._expired() or await self._sb._unreachable(e):
                    raise format_sandbox_unavailable_exception("The sandbox was not found") from e
                raise

        return call


def _snapshot_mode(field: str, mode, keep_memory=None) -> Optional[str]:
    """`"full"` / `"filesystem"` from e2b's `mode` (or its deprecated
    `keep_memory`), validated by e2b's own rules; `None` when neither is given."""
    from e2b.api.client import Unset

    memory = resolve_snapshot_memory(field, mode, keep_memory)
    if isinstance(memory, Unset):
        return None
    return "full" if memory else "filesystem"


def _resolve_lifecycle(lifecycle) -> dict:
    """e2b's `lifecycle` as `{"on_timeout", "mode", "auto_resume"}`, validated
    by e2b's rules (`build_lifecycle_config`)."""
    from e2b.api.client import Unset

    body = build_lifecycle_config(lifecycle or None)
    paused = body.auto_pause is True
    mode = None
    if paused and not isinstance(body.auto_pause_memory, Unset):
        mode = "full" if body.auto_pause_memory else "filesystem"
    resume = body.auto_resume
    if not isinstance(resume, Unset):
        _wire.check_bool("NewSandbox", "/autoResume/enabled", resume["enabled"])
    auto_resume = None if isinstance(resume, Unset) else bool(resume["enabled"])
    return {
        "on_timeout": "pause" if paused else "kill",
        "mode": mode,
        "auto_resume": auto_resume,
    }


_WHY_IAM = (
    "asks e2b's control plane for short-lived cloud credentials inside the sandbox. Cua has no "
    "credential broker of that kind: put the cloud credentials the job needs in Keyvault or pass "
    "them with `envs=` (`cua run --env KEY`), or run the Space in your own cloud account "
    "(`CUA_E2B_ON=aws`) so its machine role applies"
)
_WHY_RULES = (
    "applies header transforms and a SOCKS5 egress proxy to the sandbox's outbound HTTP(S), which "
    "needs a TLS-intercepting proxy in front of the sandbox; Cua Spaces have none. Use "
    "allow_out/deny_out with IP addresses and CIDR blocks (enforced inside the Space), and pass "
    "credentials the sandbox needs with `envs=`"
)


def _not_found(sandbox_id: str, paused: bool = False) -> SandboxNotFoundException:
    """e2b's "Sandbox <id> not found" (connect: "Paused sandbox <id> not found")."""
    return SandboxNotFoundException(
        f"{'Paused sandbox' if paused else 'Sandbox'} {sandbox_id} not found"
    )


async def _state_of(sandbox_id: str) -> Optional[str]:
    """`"running"`, `"paused"`, or `None` for a sandbox that is gone."""
    backend = get_backend()
    try:
        state = getattr(backend, "state", None)
        if state is not None:
            return await state(sandbox_id)
        for m in await backend.list():
            if m.sandbox_id == sandbox_id:
                return m.state
        return None
    except Exception as e:  # noqa: BLE001
        raise map_error(e)


def _template_error(e: BaseException, template: Optional[str]) -> BaseException:
    """An unknown template or image is e2b's `404: template '<x>' not found`;
    any other failure of the create maps as usual."""
    name = type(e).__name__
    text = str(e)
    if name == "NotFound" or (
        name == "Runtime"
        and re.search(
            r"not authorized|unauthorized|manifest unknown|pull access denied"
            r"|invalid image reference|name unknown|repository does not exist",
            text,
            re.I,
        )
    ):
        return SandboxException(f"404: template '{template}' not found")
    return map_error(e, template=True)


def _check_on_resume(on_resume) -> None:
    resolve_connect_memory(on_resume)


def _effective_egress(internet: bool, network) -> Optional[dict]:
    """e2b's network options as `{"allow": [...], "deny": [...]}` IP/CIDR rules,
    or `None` when the sandbox has no restriction. `rules` (header transforms)
    and `egress_proxy` cannot be honoured and are refused, never dropped."""
    network = network or {}
    if network.get("rules") or network.get("egress_proxy"):
        raise InvalidArgumentException(f"cua-e2b: network rules and egress_proxy {_WHY_RULES}")
    allow = list(network.get("allow_out") or [])
    deny = list(network.get("deny_out") or [])
    if not internet or network.get("allow_internet_access") is False:
        deny.append(ALL_TRAFFIC)
    if not allow and not deny:
        return None
    return {"allow": allow, "deny": deny}


def _check_network_body(schema: str, pointer: str, body, domains: bool = True) -> None:
    """The checks e2b's API makes on the `allow_out`/`deny_out` of a request:
    lists of strings, every denied entry an address or CIDR block, and domains
    in `allow_out` only next to a deny of all traffic (`domains`; an update does
    not check that)."""
    if not body:
        return
    allow, deny = body.get("allow_out"), body.get("deny_out")
    _wire.check_string_list(schema, f"{pointer}/allowOut", allow)
    _wire.check_string_list(schema, f"{pointer}/denyOut", deny)
    for entry in deny or ():
        _wire.denied_cidr(entry)
    if (
        domains
        and any(not _wire.is_ip_or_cidr(x) for x in allow or ())
        and ALL_TRAFFIC not in (deny or ())
    ):
        raise _wire.bad_request(
            "When specifying allowed domains in allow out, you must include 'ALL_TRAFFIC' "
            "in deny out to block all other traffic."
        )


def _no_network(egress: Optional[dict]) -> bool:
    """Everything denied and nothing allowed: the VM gets no network device
    at all (stronger than rules, and it works on every kind)."""
    return bool(
        egress
        and not egress["allow"]
        and egress["deny"]
        and set(egress["deny"]) <= {ALL_TRAFFIC, "all"}
    )


def _run_sync(coro):
    """Runs `coro` to completion from synchronous code (e2b's `get_host` and
    URL builders are synchronous; opening a forward or minting a signed URL is
    not) on a private loop in a worker thread."""
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        return pool.submit(asyncio.run, coro).result()


class AsyncSandbox(SandboxApi):
    """An e2b `AsyncSandbox` backed by a Cua Space of kind `vm`.

    Create it with `await AsyncSandbox.create(template=...)`. `template` is
    a Cua checkpoint name (the sandbox is forked from it) or an image (a new
    Space boots from it); ``CUA_E2B_TEMPLATES`` maps e2b ids to either.
    """

    def __init__(
        self,
        machine,
        *,
        envs=None,
        metadata=None,
        template=None,
        lifecycle=None,
        connection_config: Optional[ConnectionConfig] = None,
    ):
        self._m = machine
        super().__init__(
            sandbox_id=machine.sandbox_id,
            envd_version=ENVD_VERSION,
            envd_access_token=None,
            sandbox_domain=None,
            connection_config=connection_config or ConnectionConfig(),
            traffic_access_token=None,
        )
        self._m = machine
        self._envs = dict(envs or {})
        self._metadata = dict(metadata or {})
        self._template = template
        self._lifecycle = (
            lifecycle
            if isinstance(lifecycle, dict) and "on_timeout" in lifecycle and "mode" in lifecycle
            else (_resolve_lifecycle(lifecycle) if lifecycle is not None else {})
        )
        self._started_at = datetime.now(timezone.utc)
        self._end_at: Optional[datetime] = None
        self._paused = False
        self._allow_internet: Optional[bool] = None
        self._egress: Optional[dict] = None
        self._homes: Dict[Optional[str], str] = {}
        target = _Guard(machine, self)
        self._commands = Commands(target, self._envs, self._home_of)
        self._pty = Pty(target, self._envs, self._home_of)
        self._git = Git(self._commands)
        self._filesystem = Filesystem(target, self._home_of)
        self._hosts: Dict[int, str] = {}
        self._killed = False
        self._timer: Optional[asyncio.Task] = None

    # ── properties ────────────────────────────────────────────────────────

    @property
    def sandbox_id(self) -> str:
        """The Space's name."""
        return self._m.sandbox_id

    @property
    def envd_api_url(self) -> str:
        """The URL of cua-spacesd (the in-sandbox agent) as this machine
        reaches it."""
        return self._m.endpoint() or ""

    @property
    def envd_direct_url(self) -> str:
        return self.envd_api_url

    @property
    def sandbox_domain(self) -> str:
        """The host cua-spacesd answers on."""
        from urllib.parse import urlsplit

        return urlsplit(self.envd_api_url).hostname or self.connection_config.domain

    @property
    def files(self) -> Filesystem:
        return self._filesystem

    @property
    def commands(self) -> Commands:
        return self._commands

    @property
    def pty(self) -> Pty:
        return self._pty

    @property
    def git(self) -> Git:
        return self._git

    @property
    def metadata(self) -> Dict[str, str]:
        return dict(self._metadata)

    # ── file URLs ─────────────────────────────────────────────────────────

    def _signed_url(self, method: str, path: str, user, expiration) -> str:
        ttl = 86400 if expiration is None else expiration
        if not isinstance(ttl, (int, float)) or ttl <= 0 or ttl > 86400:
            raise InvalidArgumentException(
                f"use_signature_expiration must be between 1 and 86400 seconds (got {expiration!r}): "
                "cua-spacesd signs file URLs for at most 24 hours."
            )

        async def mint() -> str:
            if self._killed:
                raise _not_found(self.sandbox_id)
            try:
                abs_path = await self._filesystem._abs(path, user)
                return await self._m.signed_url(abs_path, method, int(ttl), guest_user(user))
            except Exception as e:  # noqa: BLE001
                if self._killed or await self._unreachable(e):
                    # cua-spacesd mints the signature: no sandbox, no URL
                    raise _not_found(self.sandbox_id) from e
                raise

        try:
            return _run_sync(mint())
        except Exception as e:  # noqa: BLE001
            raise map_error(e, file=True)

    def download_url(
        self, path: str, user: Optional[str] = None, use_signature_expiration: Optional[int] = None
    ) -> str:
        """A signed URL that serves the file with a plain GET for
        `use_signature_expiration` seconds (default and maximum 24 hours); it
        stops working when cua-spacesd's access token is rotated."""
        return self._signed_url("GET", path, user, use_signature_expiration)

    def upload_url(
        self, path: str, user: Optional[str] = None, use_signature_expiration: Optional[int] = None
    ) -> str:
        """A signed URL that replaces the file with the body of a PUT request
        (atomically), valid as `download_url` is."""
        return self._signed_url("PUT", path, user, use_signature_expiration)

    def get_mcp_url(self) -> str:
        """The URL of the MCP gateway of a sandbox created with `mcp=` (plain
        HTTP: see `get_host`)."""
        return f"http://{self.get_host(self.mcp_port)}/mcp"

    async def get_mcp_token(self) -> Optional[str]:
        """The MCP gateway token of this sandbox."""
        if not self._mcp_token:
            self._mcp_token = await self.files.read("/etc/mcp-gateway/.token", user="root")
        return self._mcp_token

    # ── lifecycle ─────────────────────────────────────────────────────────

    @classmethod
    async def create(
        cls,
        template: Optional[str] = None,
        timeout: Optional[int] = None,
        metadata: Optional[Dict[str, str]] = None,
        envs: Optional[Dict[str, str]] = None,
        secure: Optional[bool] = None,
        allow_internet_access: Optional[bool] = None,
        mcp=None,
        network=None,
        iam=None,
        lifecycle=None,
        volume_mounts=None,
        logger: Optional[logging.Logger] = None,
        **opts,
    ):
        """Creates a sandbox: forks the checkpoint `template` names, or boots
        a new Linux VM Space from the image it names.

        `timeout` (seconds, default 300) is the sandbox lifetime, kept by the
        cua daemon: it deletes the sandbox then (or suspends it, with
        `lifecycle={"on_timeout": "pause"}`) even if this process is gone.
        `allow_internet_access=False` gives the VM no network (published
        ports and the Cua channel still reach it). `request_timeout` does
        not bound the create (a VM boot can take longer than an e2b API
        call).
        """
        params = cls._resolve_api_params(**opts)
        config = ConnectionConfig(logger=logger, **params)
        if not template and mcp is not None:
            template = cls.default_mcp_template
        elif not template:
            template = cls.default_template
        # e2b's own validation first: the same arguments fail with the same texts.
        lifecycle = _resolve_lifecycle(lifecycle)
        iam_body = build_iam_config(iam)
        net_body = build_network_config(network, iam_body)
        # then what its API server checks on the request body
        if timeout is not None:
            _wire.check_integer("NewSandbox", "/timeout", timeout, 1)
        _wire.check_string_map("NewSandbox", "/metadata", metadata)
        _wire.check_string_map("NewSandbox", "/envVars", envs)
        if allow_internet_access is not None:
            _wire.check_bool("NewSandbox", "/allow_internet_access", allow_internet_access)
        if mcp is not None and not isinstance(mcp, dict):
            raise _wire.invalid("NewSandbox", "/mcp", "value must be an object")
        _check_network_body("NewSandbox", "/network", net_body)
        _wire.template_ref(template)
        if iam_body:
            raise InvalidArgumentException(f"cua-e2b: create(iam=...) {_WHY_IAM}")
        # None is e2b 2.56's "the platform default": internet on (2.42's True).
        internet = True if allow_internet_access is None else allow_internet_access
        egress = _effective_egress(internet, network)
        net = "none" if _no_network(egress) else None
        try:
            machine = await get_backend().create(
                template, network=net, egress=None if net else egress
            )
        except Exception as e:  # noqa: BLE001
            raise _template_error(e, template)
        sb = cls(
            machine,
            envs=envs,
            metadata=metadata,
            template=template,
            lifecycle=lifecycle,
            connection_config=config,
        )
        # `None`, as e2b reports it, unless the caller chose
        if allow_internet_access is not None or egress is not None:
            sb._allow_internet = egress is None or (
                not _no_network(egress) and ALL_TRAFFIC not in egress["deny"]
            )
        try:
            if volume_mounts:
                await sb._mount_volumes(volume_mounts)
            if lifecycle["mode"] == "filesystem":
                # a timeout auto-pause that keeps only the disk: resuming cold-boots
                get_backend().annotate(sb.sandbox_id, resume="cold")
            if metadata:
                get_backend().annotate(sb.sandbox_id, metadata=dict(metadata))
            sb._egress = egress
            if egress or sb._allow_internet is not None:
                get_backend().annotate(
                    sb.sandbox_id, allow_internet_access=sb._allow_internet, egress=egress
                )
            await sb._set_lifetime(timeout if timeout is not None else cls.default_sandbox_timeout)
            if mcp is not None:
                await sb._start_mcp_gateway(mcp)
        except BaseException:
            await sb.kill()
            raise
        return sb

    async def _start_mcp_gateway(self, mcp) -> None:
        """Starts the MCP gateway the way e2b does: `mcp-gateway --config <json>` as
        root with a fresh access token. The template must provide the
        `mcp-gateway` program (e2b's `mcp-gateway` template does; build one
        with the Template builder for Cua)."""
        token = str(uuid.uuid4())
        self._mcp_token = token
        try:
            await self.commands.run(
                f"mcp-gateway --config {shlex.quote(json.dumps(mcp))}",
                user="root",
                envs={"GATEWAY_ACCESS_TOKEN": token},
            )
        except CommandExitException as e:
            raise SandboxException(f"Failed to start MCP gateway: {e.stderr}") from e

    @classmethod
    async def _cls_connect(
        cls, sandbox_id: str, timeout: Optional[int] = None, *, on_resume: str = "restore", **opts
    ):
        _check_on_resume(on_resume)
        _wire.sandbox_id(sandbox_id)
        try:
            machine = await get_backend().connect(sandbox_id, reboot=on_resume == "reboot")
        except Exception as e:  # noqa: BLE001
            mapped = map_error(e)
            if isinstance(mapped, SandboxNotFoundException):
                raise _not_found(sandbox_id, paused=True) from e
            raise mapped
        sb = cls._from_record(machine, get_backend().record(machine.sandbox_id))
        await sb.connect(timeout=timeout)
        return sb

    @classmethod
    def _from_record(cls, machine, rec: dict):
        """A handle on an existing sandbox, with what this machine recorded when
        it was created: template, metadata, start and end time, lifecycle."""
        sb = cls(machine, metadata=rec.get("metadata"), template=rec.get("template"))
        sb._started_at = _parse_ts(rec.get("started_at")) or sb._started_at
        sb._end_at = _parse_ts(rec.get("end_at"))
        if rec.get("lifecycle"):
            sb._lifecycle = dict(rec["lifecycle"])
        sb._allow_internet = rec.get("allow_internet_access")
        sb._egress = rec.get("egress")
        return sb

    @class_method_variant("_cls_connect")
    async def connect(self, timeout: Optional[int] = None, *, on_resume: str = "restore", **opts):
        """Connects to the sandbox again, resuming it if it was paused.
        `on_resume="reboot"` cold-boots a paused sandbox from its disk
        instead of restoring its memory (as does a filesystem-only pause)."""
        _check_on_resume(on_resume)
        try:
            await self._m.resume(reboot=on_resume == "reboot")
        except Exception as e:  # noqa: BLE001
            if self._paused:
                raise map_error(e)
            # already running
        self._paused = False
        if timeout is not None:
            await self._set_lifetime(timeout)
        return self

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc_value, traceback):
        await self.kill()

    @classmethod
    async def _cls_kill(cls, sandbox_id: str, **opts) -> bool:
        _wire.sandbox_id(sandbox_id)
        if not sandbox_id:
            return False
        try:
            return await get_backend().kill(sandbox_id)
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        finally:
            get_backend().forget(sandbox_id)

    @class_method_variant("_cls_kill")
    async def kill(self, **opts) -> bool:
        """Deletes the Space. `False` if it was already gone."""
        self._cancel_timer()
        if self._killed:
            return False
        self._killed = True
        gone = _Gone(self.sandbox_id)
        self._commands._m = gone
        self._pty._m = gone
        self._filesystem._m = gone
        get_backend().forget(self.sandbox_id)
        try:
            return await _bounded(self._m.delete(), opts.get("request_timeout") or 0)
        except Exception as e:  # noqa: BLE001
            raise map_error(e)

    async def _unreachable(self, e: BaseException) -> bool:
        """Whether `e`, from a call on this sandbox, is because the sandbox is
        gone or paused (e2b: "The sandbox was not found") rather than about
        the call itself."""
        name = type(e).__name__
        if not _is_cua_error(e):
            return False
        text = str(e)
        if name in ("PermissionDenied", "Timeout", "InvalidPlacement"):
            return False
        if name in ("NotFound", "InvalidArgument") and not re.search(
            r"sandbox|space|paused|suspended|not registered", text, re.I
        ):
            return False
        return await _state_of(self.sandbox_id) != "running"

    async def _refresh_paused(self) -> bool:
        """Whether this sandbox is running: `_paused` is only what this handle
        last saw, another handle may have resumed it."""
        state = await _state_of(self.sandbox_id)
        self._paused = state == "paused"
        return state == "running"

    @classmethod
    async def _cls_peek(cls, sandbox_id: str):
        """The sandbox for a class-level call, without resuming it: `(sb, True)`
        when running, `(None, False)` when paused; e2b's "Sandbox <id> not found"
        when it is gone."""
        _wire.sandbox_id(sandbox_id)
        state = await _state_of(sandbox_id)
        if state is None:
            raise _not_found(sandbox_id)
        if state == "paused":
            return None, False
        try:
            machine = await get_backend().connect(sandbox_id)
        except Exception as e:  # noqa: BLE001
            mapped = map_error(e)
            if isinstance(mapped, SandboxNotFoundException):
                raise _not_found(sandbox_id) from e
            raise mapped
        return cls._from_record(machine, get_backend().record(machine.sandbox_id)), True

    @classmethod
    async def _cls_set_timeout(cls, sandbox_id: str, timeout: int, **opts) -> None:
        _wire.check_integer("SandboxTimeoutRequest", "/timeout", timeout, 0)
        sb, running = await cls._cls_peek(sandbox_id)
        if not running:
            raise _not_found(sandbox_id)
        await sb.set_timeout(timeout)

    @class_method_variant("_cls_set_timeout")
    async def set_timeout(self, timeout: int, **opts) -> None:
        """Sets the lifetime to `timeout` seconds from now (at most 24 hours;
        `0` ends the sandbox, as in e2b)."""
        _wire.check_integer("SandboxTimeoutRequest", "/timeout", timeout, 0)
        if self._killed:
            raise _not_found(self.sandbox_id)
        if self._paused and not await self._refresh_paused():
            raise _not_found(self.sandbox_id)
        if timeout == 0:
            await self.kill()
            return
        await self._set_lifetime(min(timeout, MAX_LIFETIME))

    async def is_running(self, request_timeout: Optional[float] = None) -> bool:
        if self._killed or (self._paused and not await self._refresh_paused()):
            return False
        try:
            r = await self.commands.run("true", timeout=10, request_timeout=request_timeout or 10)
            return r.exit_code == 0
        except Exception:  # noqa: BLE001
            return False

    @classmethod
    async def _cls_pause(
        cls, sandbox_id: str, keep_memory: Optional[bool] = None, *, mode=None, **opts
    ):
        _snapshot_mode("mode", mode, keep_memory)  # e2b's argument checks first
        sb, running = await cls._cls_peek(sandbox_id)
        if not running:
            return False  # already paused
        return await sb.pause(keep_memory, mode=mode)

    @class_method_variant("_cls_pause")
    async def pause(
        self, keep_memory: Optional[bool] = None, *, mode: Optional[str] = None, **opts
    ):
        """Suspends the VM. `connect()` resumes it. `mode="filesystem"` (or
        `keep_memory=False`) keeps only the disk: the resume cold-boots, so
        processes and memory do not survive; the default keeps both."""
        mode = _snapshot_mode("mode", mode, keep_memory)
        state = await _state_of(self.sandbox_id)
        if state is None or self._killed:
            raise _not_found(self.sandbox_id)
        if state == "paused":
            self._paused = True
            return False
        try:
            await self._m.suspend(cold=mode == "filesystem")
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        self._paused = True
        return True

    beta_pause = pause

    @classmethod
    async def _cls_create_snapshot(
        cls, sandbox_id: str, name: Optional[str] = None, *, mode=None, **opts
    ) -> SnapshotInfo:
        _snapshot_mode("mode", mode)
        sb = await cls._cls_connect(sandbox_id)
        return await sb.create_snapshot(name, mode=mode)

    @class_method_variant("_cls_create_snapshot")
    async def create_snapshot(
        self, name: Optional[str] = None, *, mode: Optional[str] = None, **opts
    ) -> SnapshotInfo:
        """Checkpoints the running VM (memory and disk) as `name`; create
        sandboxes from it with `create(template=name)`. `mode="filesystem"`
        makes sandboxes created from it cold-boot from the disk."""
        mode = _snapshot_mode("mode", mode)
        name = name or f"snap-{self.sandbox_id}-{secrets.token_hex(3)}"
        try:
            saved = await self._m.checkpoint(name, cold=mode == "filesystem")
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        return SnapshotInfo(snapshot_id=saved, names=[saved])

    @classmethod
    def _cls_list_snapshots(
        cls, sandbox_id: Optional[str] = None, limit=None, next_token=None, name=None, **opts
    ) -> AsyncSnapshotPaginator:
        return AsyncSnapshotPaginator(sandbox_id, limit, next_token, name)

    @class_method_variant("_cls_list_snapshots")
    def list_snapshots(
        self, limit: Optional[int] = None, next_token=None, name: Optional[str] = None, **opts
    ) -> AsyncSnapshotPaginator:
        """The snapshots of this sandbox (or, called on the class, of all)."""
        return AsyncSnapshotPaginator(self.sandbox_id, limit, next_token, name)

    @classmethod
    async def _cls_delete_snapshot(cls, snapshot_id: str, **opts) -> bool:
        _wire.snapshot_ref(snapshot_id)
        try:
            return await get_backend().delete_snapshot(snapshot_id)
        except Exception as e:  # noqa: BLE001
            raise map_error(e)

    @classmethod
    async def delete_snapshot(cls, snapshot_id: str, **opts) -> bool:
        """Deletes a snapshot (sandboxes made from it keep running). `False`
        if there is none."""
        return await cls._cls_delete_snapshot(snapshot_id, **opts)

    @classmethod
    async def _cls_fork_sandbox(
        cls,
        sandbox_id: str,
        timeout: Optional[int] = None,
        count: Optional[int] = None,
        logger: Optional[logging.Logger] = None,
        **opts,
    ):
        validate_fork_count(count)
        sb = await cls._cls_connect(sandbox_id)
        return await sb.fork(timeout=timeout, count=count)

    @class_method_variant("_cls_fork_sandbox")
    async def fork(self, timeout: Optional[int] = None, count: Optional[int] = None, **opts):
        """Forks this running sandbox `count` times (default 1): each fork is
        a new sandbox holding this one's disk and memory (copy-on-write where
        the runtime can), which keeps running. The list holds a sandbox, or
        the exception that stopped that fork, per requested fork."""
        validate_fork_count(count)
        n = 1 if count is None else count

        async def one():
            name = f"{NAME_PREFIX}{secrets.token_hex(5)}"
            machine = await self._m.fork(name)
            sb = type(self)(
                machine,
                envs=self._envs,
                metadata=self._metadata,
                template=self._template,
                lifecycle=self._lifecycle,
            )
            sb._allow_internet = self._allow_internet
            sb._egress = self._egress
            get_backend().annotate(
                sb.sandbox_id,
                template=self._template,
                started_at=datetime.now(timezone.utc).isoformat(),
                metadata=dict(self._metadata),
            )
            if self._lifecycle.get("mode") == "filesystem":
                get_backend().annotate(sb.sandbox_id, resume="cold")
            try:
                await sb._set_lifetime(timeout if timeout is not None else DEFAULT_SANDBOX_TIMEOUT)
            except BaseException:
                await sb.kill()
                raise
            return sb

        out = await asyncio.gather(*(one() for _ in range(n)), return_exceptions=True)
        return [map_error(r) if isinstance(r, Exception) else r for r in out]

    # ── info ──────────────────────────────────────────────────────────────

    @classmethod
    async def _cls_get_info(cls, sandbox_id: str, **opts) -> SandboxInfo:
        _wire.sandbox_id(sandbox_id)
        try:
            machines = await get_backend().list()
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        for m in machines:
            if m.sandbox_id == sandbox_id:
                return sandbox_info(m)
        raise _not_found(sandbox_id)

    @class_method_variant("_cls_get_info")
    async def get_info(self, **opts) -> SandboxInfo:
        """What e2b reports about the sandbox. A paused sandbox answers from
        what was recorded (its VM is not running to be asked)."""
        state = None if self._killed else await _state_of(self.sandbox_id)
        if state is None:
            raise _not_found(self.sandbox_id)
        self._paused = state == "paused"
        cpu, mem = await self._size()
        egress = getattr(self._m, "egress", None)
        return SandboxInfo(
            sandbox_id=self.sandbox_id,
            sandbox_domain=None,
            template_id=self._template or "",
            name=self._template or self.default_template,
            metadata=dict(self._metadata),
            started_at=self._started_at,
            end_at=self._end_at or self._started_at,
            state=SandboxState.PAUSED if self._paused else SandboxState.RUNNING,
            cpu_count=cpu,
            memory_mb=mem,
            envd_version=ENVD_VERSION,
            allow_internet_access=self._allow_internet,
            network=network_info(self._egress or egress),
            lifecycle={
                "on_timeout": self._lifecycle.get("on_timeout", "kill"),
                "auto_resume": bool(self._lifecycle.get("auto_resume", False)),
            },
        )

    async def _size(self):
        """vCPUs and memory (MiB): measured while running, else as recorded."""
        if not self._paused:
            try:
                m = await self._sample()
                self._cached_size = (m.cpu_count, m.mem_total // (1024 * 1024))
                get_backend().annotate(
                    self.sandbox_id,
                    cpu_count=self._cached_size[0],
                    memory_mb=self._cached_size[1],
                )
            except Exception:  # noqa: BLE001
                if not getattr(self, "_cached_size", None):
                    raise
        if getattr(self, "_cached_size", None):
            return self._cached_size
        rec = get_backend().record(self.sandbox_id)
        return int(rec.get("cpu_count") or DEFAULT_CPUS), int(
            rec.get("memory_mb") or DEFAULT_MEMORY_MB
        )

    @classmethod
    async def _cls_update_network(cls, sandbox_id: str, network, **opts) -> None:
        body = build_network_update_body(network)
        _check_network_body("SandboxNetworkUpdateConfig", "", body, domains=False)
        sb, running = await cls._cls_peek(sandbox_id)
        if not running:
            raise _not_found(sandbox_id)
        await sb.update_network(network)

    @class_method_variant("_cls_update_network")
    async def update_network(self, network: SandboxNetworkUpdate, **opts) -> None:
        """Replaces the sandbox's egress rules (omitted fields are cleared, as
        in e2b): `allow_out`/`deny_out` with IP addresses and CIDR blocks and
        `allow_internet_access` are enforced inside the Space by the core;
        `rules` and `egress_proxy` are refused (see `create`)."""
        body = build_network_update_body(network)
        _check_network_body("SandboxNetworkUpdateConfig", "", body, domains=False)
        internet = body.get("allow_internet_access", True)
        egress = _effective_egress(
            internet,
            {k: body[k] for k in ("allow_out", "deny_out", "rules", "egress_proxy") if k in body},
        )
        try:
            await self._m.set_egress(
                (egress or {}).get("allow", []), (egress or {}).get("deny", [])
            )
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        self._allow_internet = egress is None or (
            not _no_network(egress) and ALL_TRAFFIC not in egress["deny"]
        )
        self._egress = egress
        get_backend().annotate(
            self.sandbox_id, allow_internet_access=self._allow_internet, egress=egress
        )

    @classmethod
    async def _cls_get_metrics(cls, sandbox_id: str, start=None, end=None, **opts):
        _wire.sandbox_id(sandbox_id)
        _wire.metrics_window(start, end)
        try:
            sb, running = await cls._cls_peek(sandbox_id)
        except SandboxNotFoundException:
            return []  # e2b answers an unknown sandbox with no samples
        if not running:
            return []
        return await sb.get_metrics(start, end)

    @class_method_variant("_cls_get_metrics")
    async def get_metrics(self, start=None, end=None, **opts) -> List[SandboxMetrics]:
        """The current resource sample from the guest (CPU, memory, root
        disk). cua-spacesd keeps no history, so a window that ends before now
        or starts after it is empty."""
        _wire.metrics_window(start, end)
        if self._killed or (self._paused and not await self._refresh_paused()):
            return []
        if end is not None and _aware(end) < self._started_at:
            return []
        try:
            return [await self._sample()]
        except SandboxException:
            if await _state_of(self.sandbox_id) != "running":
                return []  # gone or paused: nothing to sample
            raise

    async def _sample(self) -> SandboxMetrics:
        try:
            m = await _Guard(self._m, self).metrics()
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        return SandboxMetrics(
            cpu_count=m["cpu_count"],
            cpu_used_pct=round(m["cpu_pct"], 2),
            disk_total=m["disk_total"],
            disk_used=m["disk_used"],
            mem_total=m["mem_total"],
            mem_used=m["mem_used"],
            mem_cache=m["mem_cache"],
            timestamp=datetime.now(timezone.utc),
        )

    # ── ports ─────────────────────────────────────────────────────────────

    def get_host(self, port: int) -> str:
        """A `host:port` on this machine that reaches guest `port` (plain
        TCP, so `http://` rather than e2b's `https://`). The first call for
        a port opens the forward; ``CUA_E2B_BIND=0.0.0.0:0`` serves the LAN."""
        if port not in self._hosts:
            # e2b's get_host is synchronous; opening a forward is not. Run it
            # to completion on a private loop in a worker thread.
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                try:
                    self._hosts[port] = pool.submit(asyncio.run, self._m.forward(port)).result()
                except Exception as e:  # noqa: BLE001
                    raise map_error(e)
        return self._hosts[port]

    # ── internals ─────────────────────────────────────────────────────────

    async def _mount_volumes(self, mounts) -> None:
        """`volume_mounts={"/data": volume}`: the path becomes the volume's
        folder of Cua Volume, which the Space mounts at /volume."""
        from e2b.volume import mount_source

        r = await self.commands.run("mountpoint -q /volume && echo yes || echo no", timeout=30)
        if r.stdout.strip() != "yes":
            raise InvalidArgumentException(
                "cua-e2b: volume_mounts needs Cua Volume mounted in the sandbox (/volume), and "
                "this Space has none (QEMU VMs do not mount it yet). Read and write the volume "
                "from your code with Volume.read_file/write_file, or with files.write on the sandbox"
            )
        for path, vol in mounts.items():
            name = getattr(vol, "name", vol)
            src, dest = mount_source(name), shlex.quote(str(path))
            await self.commands.run(
                f"mkdir -p {shlex.quote(src)} $(dirname {dest}) && ln -sfn {shlex.quote(src)} {dest}",
                user="root",
                timeout=60,
            )

    async def _set_lifetime(self, seconds: float) -> None:
        on_expire = "suspend" if self._lifecycle.get("on_timeout") == "pause" else "delete"
        try:
            enforced = await self._m.set_lifetime(seconds, on_expire)
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        self._end_at = datetime.now(timezone.utc) + timedelta(seconds=seconds)
        get_backend().annotate(
            self.sandbox_id,
            end_at=self._end_at.isoformat(),
            lifecycle=dict(self._lifecycle) or None,
        )
        self._cancel_timer()
        if not enforced:
            # No cua daemon (an embedded runtime): this process keeps time.
            self._arm_timer(seconds)

    def _expired(self) -> bool:
        return (
            self._lifecycle.get("on_timeout", "kill") == "kill"
            and self._end_at is not None
            and datetime.now(timezone.utc) >= self._end_at
        )

    async def _resume_if_paused(self) -> None:
        if self._paused:
            await self.connect()

    async def _home_of(self, user: Optional[str]) -> str:
        u = guest_user(user)
        if u not in self._homes:
            r = await self.commands.run('printf %s "$HOME"', user=user, timeout=30)
            self._homes[u] = r.stdout.strip() or ("/root" if u == "root" else "/")
        return self._homes[u]

    def _cancel_timer(self) -> None:
        if self._timer is not None and not self._timer.done():
            self._timer.cancel()
        self._timer = None

    def _arm_timer(self, seconds: float) -> None:
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        self._timer = loop.create_task(self._expire(seconds))

    async def _expire(self, seconds: float) -> None:
        await asyncio.sleep(seconds)
        self._timer = None
        try:
            if self._lifecycle.get("on_timeout") == "pause":
                await self.pause(mode=self._lifecycle.get("mode"))
            else:
                await self.kill()
        except Exception:  # noqa: BLE001
            logger.warning(
                "cua-e2b: could not end expired sandbox %s", self.sandbox_id, exc_info=True
            )

    def __repr__(self) -> str:
        return f"AsyncSandbox(sandbox_id={self.sandbox_id!r})"


__all__ = [
    "ALL_TRAFFIC",
    "AsyncSandbox",
    "AsyncSandboxPaginator",
    "AsyncSnapshotPaginator",
    "SandboxInfo",
    "SandboxMetrics",
    "SandboxQuery",
    "SnapshotInfo",
]
