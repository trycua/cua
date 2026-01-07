"""Configuration loader for cua-bench."""

from pathlib import Path
from typing import Any

import yaml

from .schema import AgentsConfig, CuaConfig, CustomAgentEntry


class ConfigLoader:
    """Load and merge configuration from .cua/ directory."""

    CONFIG_DIR_NAME = ".cua"
    CONFIG_FILE_NAME = "config.yaml"
    AGENTS_FILE_NAME = "agents.yaml"

    def __init__(self, search_path: Path | None = None):
        """Initialize the config loader.

        Args:
            search_path: Starting path to search for .cua/ directory.
                        Defaults to current working directory.
        """
        self.search_path = search_path or Path.cwd()
        self._config_dir: Path | None = None
        self._config: CuaConfig | None = None
        self._agents: AgentsConfig | None = None

    def find_config_dir(self) -> Path | None:
        """Walk up directory tree to find .cua/ directory.

        Returns:
            Path to .cua/ directory if found, None otherwise.
        """
        if self._config_dir is not None:
            return self._config_dir

        current = self.search_path.resolve()

        while current != current.parent:
            config_dir = current / self.CONFIG_DIR_NAME
            if config_dir.is_dir():
                self._config_dir = config_dir
                return config_dir
            current = current.parent

        # Check root
        config_dir = current / self.CONFIG_DIR_NAME
        if config_dir.is_dir():
            self._config_dir = config_dir
            return config_dir

        return None

    def load_config(self) -> CuaConfig | None:
        """Load .cua/config.yaml if it exists.

        Returns:
            CuaConfig object if config file exists, None otherwise.
        """
        if self._config is not None:
            return self._config

        config_dir = self.find_config_dir()
        if config_dir is None:
            return None

        config_file = config_dir / self.CONFIG_FILE_NAME
        if not config_file.is_file():
            return None

        with open(config_file, "r") as f:
            data = yaml.safe_load(f) or {}

        self._config = CuaConfig.from_dict(data)
        return self._config

    def load_agents(self) -> list[CustomAgentEntry]:
        """Load .cua/agents.yaml if it exists.

        Returns:
            List of CustomAgentEntry objects.
        """
        if self._agents is not None:
            return self._agents.custom_agents

        config_dir = self.find_config_dir()
        if config_dir is None:
            return []

        agents_file = config_dir / self.AGENTS_FILE_NAME
        if not agents_file.is_file():
            return []

        with open(agents_file, "r") as f:
            data = yaml.safe_load(f) or {}

        self._agents = AgentsConfig.from_dict(data)
        return self._agents.custom_agents

    def get_agent_by_name(self, name: str) -> CustomAgentEntry | None:
        """Get a custom agent entry by name.

        Args:
            name: Agent name to look up.

        Returns:
            CustomAgentEntry if found, None otherwise.
        """
        agents = self.load_agents()
        for agent in agents:
            if agent.name == name:
                return agent
        return None

    def get_effective_config(
        self,
        cli_args: dict[str, Any],
        env_type: str | None = None,
    ) -> dict[str, Any]:
        """Merge configuration sources into effective config.

        Priority (highest to lowest):
        1. CLI arguments
        2. Environment-specific overrides
        3. Agent defaults from agents.yaml
        4. Agent config from config.yaml
        5. Defaults from config.yaml

        Args:
            cli_args: Command line arguments as dictionary.
            env_type: Environment type for env-specific overrides (e.g., "webtop", "winarena").

        Returns:
            Merged configuration dictionary.
        """
        effective: dict[str, Any] = {}

        # 1. Start with defaults from config.yaml
        config = self.load_config()
        if config and config.defaults:
            if config.defaults.model:
                effective["model"] = config.defaults.model
            effective["max_steps"] = config.defaults.max_steps
            effective["output_dir"] = config.defaults.output_dir

        # 2. Apply agent config from config.yaml
        if config and config.agent:
            if config.agent.name:
                effective["agent"] = config.agent.name
            if config.agent.import_path:
                effective["agent_import_path"] = config.agent.import_path
            if config.agent.model:
                effective["model"] = config.agent.model
            if config.agent.max_steps:
                effective["max_steps"] = config.agent.max_steps

            # 3. Apply environment-specific overrides
            if env_type and config.agent.environments:
                env_overrides = config.agent.environments.get(env_type, {})
                for key, value in env_overrides.items():
                    if value is not None:
                        effective[key] = value

        # 4. Apply agent defaults from agents.yaml (if agent is specified)
        agent_name = effective.get("agent") or cli_args.get("agent")
        if agent_name:
            agent_entry = self.get_agent_by_name(agent_name)
            if agent_entry:
                # Use import_path from agents.yaml
                effective["agent_import_path"] = agent_entry.import_path
                # Apply defaults from agents.yaml
                for key, value in agent_entry.defaults.items():
                    if key not in effective or effective[key] is None:
                        effective[key] = value

        # 5. Override with CLI arguments (highest priority)
        for key, value in cli_args.items():
            if value is not None and key not in ("command", "env_path"):
                effective[key] = value

        return effective


def detect_env_type(env_path: str) -> str | None:
    """Detect environment type from path.

    Args:
        env_path: Path to the environment.

    Returns:
        Environment type string ("webtop" or "winarena"), or None if unknown.
    """
    env_path_lower = env_path.lower()
    if "winarena" in env_path_lower:
        return "winarena"
    # Default to webtop for HTML-based environments
    return "webtop"
