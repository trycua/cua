"""The cua SDK.

One `Cua` object, embedded in this process or connected to a running
`cua daemon`, with the same API either way::

    import asyncio, cua

    async def main():
        c = cua.embedded()                      # or cua.connect()
        sb = await c.sandboxes().connect_url("http://10.0.0.5:3211", "token", "dev")
        env = await sb.spacesd(None)
        out = await env.sh("uname -a", None)
        print(out.stdout.decode())

    asyncio.run(main())

Modules: sandboxes (local, direct, your own cloud), env (cua-spacesd, with
`call_json` for every `cua.env.v1` RPC), media sessions with `FrameSink` /
`AudioSink` callbacks (encoded) or `DecodedFrameSink` / `PcmSink` callbacks
(decoded BGRA frames and PCM audio), local runtimes and images, and spaces
(`c.spaces()`: add a machine by URL, provision a local one; then bash,
files, send_file, driver tools, streams, presence, teleport with an
approval callback, hotspot, agents).

Everything is generated from the Rust `cua-sdk` crate by UniFFI
(`cua._native`); this module only adds two constructors.

High-level API (optional extras)
--------------------------------
`cua` also carries the convenience surface of the former `cua` meta-package
(0.1.x), resolved lazily from the optional extras::

    pip install "cua[sandbox]"   # cua-sandbox: Sandbox, Image, runtimes ...
    pip install "cua[agent]"     # cua-agent: ComputerAgent, callbacks, tools
    pip install "cua[all]"

    from cua import Sandbox, Image          # cua_sandbox.Sandbox / Image
    from cua import ComputerAgent           # cua_agent.ComputerAgent

With cua-sandbox installed, `cua.Sandbox` is the high-level
`cua_sandbox.Sandbox`; the SDK's own sandbox handle type (what
`sandboxes().create()` returns) is always available as `cua.SandboxHandle`.
"""

from __future__ import annotations

import importlib
import os
from typing import Any, Optional

# Local public URLs are served by the cua daemon, which the SDK starts on
# demand with the CLI bundled in this wheel (`CUA_BIN` overrides).
try:
    from ._cli import binary as _bundled_cli

    if not os.environ.get("CUA_BIN") and _bundled_cli().is_file():
        os.environ["CUA_BIN"] = str(_bundled_cli())
except Exception:  # noqa: BLE001 - optional convenience
    pass

from ._native import *  # noqa: F401,F403
from ._native import Cua, CuaConfig
from ._native import Sandbox as SandboxHandle
from ._native import cua_sdk_version

__version__ = "0.5.0"  # x-release-please-version

# Usage telemetry (anonymous, content-free; https://cua.ai/docs/cua-sdk/concepts/telemetry)
# attributes events to this binding; `cua.telemetry` has the switches. It
# sends nothing by itself; off with DO_NOT_TRACK=1 or CUA_TELEMETRY=0.
try:
    from ._native import telemetry_set_surface as _telemetry_set_surface

    _telemetry_set_surface("sdk_python", cua_sdk_version())
except Exception:  # noqa: BLE001 - telemetry never fails an import
    pass


# ── Sandbox refs ───────────────────────────────────────────────────────────
# `CuaError.AmbiguousSandbox` (a bare name in more than one location) carries
# the qualified refs it matches, like `local:box` and `cloud:box`.
from ._native import CuaError as _CuaError  # noqa: E402
from ._native import ambiguous_sandbox_candidates as _ambiguous_candidates  # noqa: E402

_CuaError.AmbiguousSandbox.candidates = property(  # type: ignore[attr-defined]
    lambda self: list(_ambiguous_candidates(str(self)))
)

# ── Error docs ─────────────────────────────────────────────────────────────
# Every `CuaError` links to its entry (cause and fix) on the errors reference.
from ._native import error_doc_url as _error_doc_url  # noqa: E402

_CuaError.doc_url = property(  # type: ignore[attr-defined]
    lambda self: _error_doc_url(type(self).__name__)
)

# ── Plan limits ────────────────────────────────────────────────────────────
# The relay refuses a new live session over the plan's cap (HTTP 429). The
# SDK raises `CuaError.Http` with the plain sentence; `error.plan_limit`
# reads it as a `PlanLimitError` (None for any other error).
from ._plan_limit import parse_plan_limit as _parse_plan_limit  # noqa: E402


class PlanLimitError(_CuaError.Http):  # type: ignore[name-defined,misc]
    """The relay refused a new live session because the plan is at its cap.

    ``str(error)`` is the plain sentence. Live sessions are never cut, and
    retrying gets the same answer: close one, or send the person to
    ``upgrade_url``.
    """

    code = "plan_limit"
    status = 429

    def __init__(self, message, *, kind, plan, used, limit, upgrade_url=None):
        super().__init__(message)
        self.kind = kind  # "sessions" or "hours"
        self.plan = plan  # "free", "pro", "teams", or the relay's own name
        self.used = used
        self.limit = limit
        self.upgrade_url = upgrade_url

    def __repr__(self):
        return "PlanLimitError({})".format(repr(str(self)))

    @classmethod
    def from_error(cls, error):
        """The refusal an error carries, else None."""
        found = _parse_plan_limit(str(error))
        if found is None:
            return None
        return cls(
            found.message,
            kind=found.kind,
            plan=found.plan,
            used=found.used,
            limit=found.limit,
            upgrade_url=found.upgrade_url,
        )


_CuaError.Http.plan_limit = property(PlanLimitError.from_error)  # type: ignore[attr-defined]

# `Sandbox` is resolved lazily (see __getattr__): the cua-sandbox facade when
# it is installed, else the native handle type.
del Sandbox  # type: ignore[name-defined]  # noqa: F821


def embedded(
    *,
    state_dir: Optional[str] = None,
    account_from_session: bool = False,
    env_probe_timeout_ms: Optional[int] = None,
    spaces_home: Optional[str] = None,
    teleport_home: Optional[str] = None,
) -> Cua:
    """An SDK runtime in this process (no I/O until the first call).

    ``account_from_session=True`` uses the ``cua auth login`` session for
    the relay account the runtime lists "My machines" with.
    ``spaces_home`` is the Spaces registry directory (default ``~/.cua``);
    ``teleport_home`` makes teleport read app sessions under that directory
    with no host side effects (tests).
    """
    return Cua.embedded(
        CuaConfig(
            state_dir=state_dir,
            account_from_session=account_from_session,
            env_probe_timeout_ms=env_probe_timeout_ms,
            spaces_home=spaces_home,
            teleport_home=teleport_home,
        )
    )


# ── Agent runs: `async for event in run.stream()` ──────────────────────────
from ._native import AgentRun as _AgentRun  # noqa: E402


async def _agent_run_stream(
    self: Any, cursor: int = 0, poll: float = 0.5, until_idle: bool = True, max_polls: int = 172_800
):
    """Normalized events from ``cursor``, polling while nothing is new.

    Stops after the turn ends (``until_idle``) or after ``max_polls`` empty
    polls. The run lives in the sandbox: breaking out of the loop, or losing
    the connection, does not stop it; resume with ``stream(cursor=...)``.
    """
    import asyncio

    for _ in range(max_polls):
        page = await self.events(cursor, None)
        cursor = page.cursor
        ended = False
        for e in page.events:
            yield e
            ended = ended or e.kind in ("turn_ended", "exited")
        if page.caught_up:
            if until_idle and (ended or (await self.status()).status != "running"):
                return
            await asyncio.sleep(poll)


_AgentRun.stream = _agent_run_stream  # type: ignore[attr-defined]


def connect(address: Optional[str] = None, token: Optional[str] = None) -> Cua:
    """A client of a running `cua daemon` (socket path or loopback URL)."""
    return Cua.connect(address, token)


# ── High-level API from the optional extras (the former meta-package) ──────

_SANDBOX_NAMES = (
    "Sandbox",
    "SandboxInfo",
    "sandbox",
    "configure",
    "login",
    "whoami",
    "CloudTransport",
    "EnvTransport",
    "SpacesdNotAvailable",
    "RuntimeSupport",
    "check_local_support",
    "skip_if_unsupported",
)
_LAZY: dict[str, tuple[str, str, str]] = {
    # name -> (module, attribute, extra that provides it)
    **{name: ("cua_sandbox", name, "sandbox") for name in _SANDBOX_NAMES},
    "DockerRuntime": ("cua_sandbox.runtime", "DockerRuntime", "sandbox"),
    "QEMURuntime": ("cua_sandbox.runtime", "QEMURuntime", "sandbox"),
    "LumeRuntime": ("cua_sandbox.runtime", "LumeRuntime", "sandbox"),
    "TartRuntime": ("cua_sandbox.runtime", "TartRuntime", "sandbox"),
    "AndroidEmulatorRuntime": ("cua_sandbox.runtime", "AndroidEmulatorRuntime", "sandbox"),
    "HyperVRuntime": ("cua_sandbox.runtime", "HyperVRuntime", "sandbox"),
    "RuntimeInfo": ("cua_sandbox.runtime", "RuntimeInfo", "sandbox"),
    "Shell": ("cua_sandbox.interfaces.shell", "Shell", "sandbox"),
    "CommandResult": ("cua_sandbox.interfaces.shell", "CommandResult", "sandbox"),
    "Mouse": ("cua_sandbox.interfaces.mouse", "Mouse", "sandbox"),
    "Keyboard": ("cua_sandbox.interfaces.keyboard", "Keyboard", "sandbox"),
    "Screen": ("cua_sandbox.interfaces.screen", "Screen", "sandbox"),
    "Clipboard": ("cua_sandbox.interfaces.clipboard", "Clipboard", "sandbox"),
    "Tunnel": ("cua_sandbox.interfaces.tunnel", "Tunnel", "sandbox"),
    "TunnelInfo": ("cua_sandbox.interfaces.tunnel", "TunnelInfo", "sandbox"),
    "Mobile": ("cua_sandbox.interfaces.mobile", "Mobile", "sandbox"),
    "Terminal": ("cua_sandbox.interfaces.terminal", "Terminal", "sandbox"),
    "Window": ("cua_sandbox.interfaces.window", "Window", "sandbox"),
    "ComputerAgent": ("cua_agent", "ComputerAgent", "agent"),
    "AgentResponse": ("cua_agent", "AgentResponse", "agent"),
    "Messages": ("cua_agent", "Messages", "agent"),
    "register_agent": ("cua_agent", "register_agent", "agent"),
}


def __getattr__(name: str) -> Any:
    if name == "Sandbox":
        try:
            return importlib.import_module("cua_sandbox").Sandbox
        except ImportError:
            return SandboxHandle
    if name == "Image":
        # cua_sandbox.Image when installed, else the canonical-ref helpers.
        try:
            return importlib.import_module("cua_sandbox").Image
        except ImportError:
            from .images import Image

            return Image
    target = _LAZY.get(name)
    if target is None:
        raise AttributeError(f"module 'cua' has no attribute {name!r}")
    module, attribute, extra = target
    try:
        return getattr(importlib.import_module(module), attribute)
    except ImportError as error:
        raise ImportError(
            f"cua.{name} comes from the optional `{extra}` extra: pip install 'cua[{extra}]'"
        ) from error


def __dir__() -> list[str]:
    return sorted({*globals(), "Sandbox", "Image", *_LAZY})
