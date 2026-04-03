"""cua — Computer-Use Agents unified SDK.

Quick start::

    from cua import Sandbox, Image, ComputerAgent

    # Configure API access (optional for local sandboxes)
    import cua
    cua.configure(api_key="sk-...")

    # Start an ephemeral sandbox and run an agent inside it
    async with Sandbox.ephemeral(Image.linux()) as sb:
        screenshot = await sb.screenshot()

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5", tools=[sb])
        async for response in agent.run("Open the browser"):
            print(response)

Opt out of telemetry::

    export CUA_TELEMETRY_ENABLED=false
"""

__version__ = "0.1.4"  # managed by bump2version — do not edit manually

# ---------------------------------------------------------------------------
# cua-sandbox surface
# ---------------------------------------------------------------------------
from cua_sandbox import (
    CloudTransport,
    Image,
    Localhost,
    Sandbox,
    SandboxInfo,
    configure,
    localhost,
    login,
    sandbox,
    whoami,
)

# ---------------------------------------------------------------------------
# Runtime compatibility helpers — lazily imported so older cua-sandbox
# releases that pre-date compat.py still work.
# ---------------------------------------------------------------------------
try:
    from cua_sandbox.runtime.compat import (
        RuntimeSupport,
        check_local_support,
        skip_if_unsupported,
    )
except ImportError:  # cua-sandbox < compat.py introduction

    def _missing_compat(*_args, **_kwargs):  # type: ignore[misc]
        raise ImportError(
            "cua_sandbox.runtime.compat is not available in this version of cua-sandbox. "
            "Upgrade cua-sandbox to use RuntimeSupport, check_local_support, or skip_if_unsupported."
        )

    RuntimeSupport = _missing_compat  # type: ignore[assignment,misc]
    check_local_support = _missing_compat  # type: ignore[assignment]
    skip_if_unsupported = _missing_compat  # type: ignore[assignment]

# ---------------------------------------------------------------------------
# Lazy imports — runtime classes, interface types, agent surface.
# These are pulled in on first attribute access so that `import cua` stays
# fast even when heavy optional deps (grpcio, vncdotool, …) are not installed.
# ---------------------------------------------------------------------------

_RUNTIME_NAMES: dict[str, tuple[str, str]] = {
    # name -> (module, attr)
    "DockerRuntime": ("cua_sandbox.runtime.docker", "DockerRuntime"),
    "QEMURuntime": ("cua_sandbox.runtime.qemu", "QEMURuntime"),
    "LumeRuntime": ("cua_sandbox.runtime.lume", "LumeRuntime"),
    "AndroidEmulatorRuntime": ("cua_sandbox.runtime.android_emulator", "AndroidEmulatorRuntime"),
    "HyperVRuntime": ("cua_sandbox.runtime.hyperv", "HyperVRuntime"),
    "RuntimeInfo": ("cua_sandbox.runtime.base", "RuntimeInfo"),
}

_INTERFACE_NAMES: dict[str, tuple[str, str]] = {
    "Shell": ("cua_sandbox.interfaces.shell", "Shell"),
    "CommandResult": ("cua_sandbox.interfaces.shell", "CommandResult"),
    "Mouse": ("cua_sandbox.interfaces.mouse", "Mouse"),
    "Keyboard": ("cua_sandbox.interfaces.keyboard", "Keyboard"),
    "Screen": ("cua_sandbox.interfaces.screen", "Screen"),
    "Clipboard": ("cua_sandbox.interfaces.clipboard", "Clipboard"),
    "Tunnel": ("cua_sandbox.interfaces.tunnel", "Tunnel"),
    "TunnelInfo": ("cua_sandbox.interfaces.tunnel", "TunnelInfo"),
    "Mobile": ("cua_sandbox.interfaces.mobile", "Mobile"),
    "Terminal": ("cua_sandbox.interfaces.terminal", "Terminal"),
    "Window": ("cua_sandbox.interfaces.window", "Window"),
}

_AGENT_NAMES: dict[str, tuple[str, str]] = {
    "ComputerAgent": ("agent", "ComputerAgent"),
    "AgentResponse": ("agent", "AgentResponse"),
    "Messages": ("agent", "Messages"),
    "register_agent": ("agent", "register_agent"),
}

_LAZY: dict[str, tuple[str, str]] = {**_RUNTIME_NAMES, **_INTERFACE_NAMES, **_AGENT_NAMES}


def __getattr__(name: str):
    if name in _LAZY:
        mod_path, attr = _LAZY[name]
        import importlib

        mod = importlib.import_module(mod_path)
        return getattr(mod, attr)
    raise AttributeError(f"module 'cua' has no attribute {name!r}")


__all__ = [
    # cua-sandbox core
    "configure",
    "login",
    "whoami",
    "Image",
    "Sandbox",
    "SandboxInfo",
    "sandbox",
    "Localhost",
    "localhost",
    "CloudTransport",
    # runtime compat (always available)
    "RuntimeSupport",
    "check_local_support",
    "skip_if_unsupported",
    # runtime classes (lazy)
    "DockerRuntime",
    "QEMURuntime",
    "LumeRuntime",
    "AndroidEmulatorRuntime",
    "HyperVRuntime",
    "RuntimeInfo",
    # interface types (lazy)
    "Shell",
    "CommandResult",
    "Mouse",
    "Keyboard",
    "Screen",
    "Clipboard",
    "Tunnel",
    "TunnelInfo",
    "Mobile",
    "Terminal",
    "Window",
    # cua-agent (lazy)
    "ComputerAgent",
    "AgentResponse",
    "Messages",
    "register_agent",
]
