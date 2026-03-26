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

__version__ = "0.1.1"  # managed by bump2version — do not edit manually

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
from cua_sandbox.runtime import (
    AndroidEmulatorRuntime,
    DockerRuntime,
    HyperVRuntime,
    LumeRuntime,
    QEMUBaremetalRuntime,
    QEMUDockerRuntime,
    QEMURuntime,
    QEMUWSL2Runtime,
    Runtime,
    RuntimeInfo,
    TartRuntime,
)

# ---------------------------------------------------------------------------
# cua-agent surface (lazy to avoid import-time side effects when only
# sandbox symbols are needed)
# ---------------------------------------------------------------------------


def __getattr__(name: str):
    _AGENT_NAMES = {"ComputerAgent", "AgentResponse", "Messages", "register_agent"}
    if name in _AGENT_NAMES:
        import agent as _agent

        return getattr(_agent, name)
    raise AttributeError(f"module 'cua' has no attribute {name!r}")


__all__ = [
    # cua-sandbox
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
    # cua-sandbox runtime
    "Runtime",
    "RuntimeInfo",
    "DockerRuntime",
    "QEMURuntime",
    "QEMUDockerRuntime",
    "QEMUBaremetalRuntime",
    "QEMUWSL2Runtime",
    "LumeRuntime",
    "HyperVRuntime",
    "AndroidEmulatorRuntime",
    "TartRuntime",
    # cua-agent (lazy)
    "ComputerAgent",
    "AgentResponse",
    "Messages",
    "register_agent",
]
