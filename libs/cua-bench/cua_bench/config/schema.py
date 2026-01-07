"""Configuration schema for cua-bench."""

from dataclasses import dataclass, field
from typing import Any, Optional


# Default agent image used when no custom image is specified
DEFAULT_AGENT_IMAGE = "cua-bench:latest"


@dataclass
class CustomAgentEntry:
    """Entry for a custom agent in .cua/agents.yaml.

    Agents can be defined in two ways:
    1. Docker image (cloud-ready): Specify `image` field with a Docker image
    2. Import path (local dev): Specify `import_path` for Python import

    Examples:
        # Docker image agent
        - name: my-agent
          image: myregistry/my-agent:latest

        # Import path agent (uses default cua-agent image)
        - name: dev-agent
          import_path: my_agents.dev:DevAgent

        # Built-in agent
        - name: cua-agent
          builtin: true
    """

    name: str
    image: Optional[str] = None  # Docker image for agent container
    import_path: Optional[str] = None  # Python import path (for local dev)
    builtin: bool = False  # True for built-in agents (cua-agent, gemini)
    command: Optional[list[str]] = None  # Custom command to run in container
    defaults: dict[str, Any] = field(default_factory=dict)

    def get_image(self) -> str:
        """Get the Docker image to use for this agent.

        Returns:
            Docker image name. Uses custom image if specified,
            otherwise returns the default cua-agent image.
        """
        if self.image:
            return self.image
        return DEFAULT_AGENT_IMAGE

    def is_docker_agent(self) -> bool:
        """Check if this agent is defined as a Docker image.

        Returns:
            True if agent has a custom Docker image specified.
        """
        return self.image is not None


@dataclass
class AgentConfig:
    """Agent configuration from .cua/config.yaml."""

    name: str | None = None
    import_path: str | None = None
    model: str | None = None
    max_steps: int = 100
    environments: dict[str, dict[str, Any]] | None = None  # Per-env overrides

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AgentConfig":
        """Create AgentConfig from dictionary."""
        return cls(
            name=data.get("name"),
            import_path=data.get("import_path"),
            model=data.get("model"),
            max_steps=data.get("max_steps", 100),
            environments=data.get("environments"),
        )


@dataclass
class DefaultsConfig:
    """Default configuration values from .cua/config.yaml."""

    model: str | None = None
    max_steps: int = 100
    output_dir: str = "./results"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DefaultsConfig":
        """Create DefaultsConfig from dictionary."""
        return cls(
            model=data.get("model"),
            max_steps=data.get("max_steps", 100),
            output_dir=data.get("output_dir", "./results"),
        )


@dataclass
class CuaConfig:
    """Root configuration from .cua/config.yaml."""

    defaults: DefaultsConfig | None = None
    agent: AgentConfig | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "CuaConfig":
        """Create CuaConfig from dictionary."""
        defaults = None
        if "defaults" in data:
            defaults = DefaultsConfig.from_dict(data["defaults"])

        agent = None
        if "agent" in data:
            agent = AgentConfig.from_dict(data["agent"])

        return cls(defaults=defaults, agent=agent)


@dataclass
class AgentsConfig:
    """Configuration from .cua/agents.yaml.

    Supports two formats:
    - Legacy: `custom_agents` list
    - New: `agents` list (preferred)

    Example .cua/agents.yaml:
        agents:
          - name: my-agent
            image: myregistry/my-agent:latest
            defaults:
              model: gpt-4o

          - name: dev-agent
            import_path: my_agents.dev:DevAgent
    """

    custom_agents: list[CustomAgentEntry] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AgentsConfig":
        """Create AgentsConfig from dictionary."""
        agents = []

        # Support both 'agents' (new) and 'custom_agents' (legacy) keys
        agents_list = data.get("agents", data.get("custom_agents", []))

        for agent_data in agents_list:
            # Parse command if provided
            command = agent_data.get("command")
            if command and isinstance(command, str):
                command = command.split()  # Convert string to list

            agents.append(CustomAgentEntry(
                name=agent_data["name"],
                image=agent_data.get("image"),
                import_path=agent_data.get("import_path"),
                builtin=agent_data.get("builtin", False),
                command=command,
                defaults=agent_data.get("defaults", {}),
            ))
        return cls(custom_agents=agents)
