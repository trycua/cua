# Agents package

from __future__ import annotations

import importlib
from typing import TYPE_CHECKING

from .base import BaseAgent, AgentResult, FailureMode

if TYPE_CHECKING:
    from cua_bench.config import ConfigLoader

# Agent registry for dynamic loading
_AGENT_REGISTRY = {}


def register_agent(name: str):
    """Decorator to register an agent class with a given name."""
    def decorator(cls):
        _AGENT_REGISTRY[name] = cls
        return cls
    return decorator


def load_agent_from_path(import_path: str) -> type[BaseAgent]:
    """Load an agent class from an import path.

    Args:
        import_path: Import path in format 'module.path:ClassName'

    Returns:
        Agent class

    Raises:
        ValueError: If import path format is invalid
        ImportError: If module cannot be imported
        AttributeError: If class is not found in module
    """
    if ":" not in import_path:
        raise ValueError(
            f"Invalid agent import path: {import_path}. "
            "Expected format: 'module.path:ClassName'"
        )

    module_path, class_name = import_path.split(":", 1)
    module = importlib.import_module(module_path)
    agent_class = getattr(module, class_name)
    return agent_class


def get_agent(name: str, config_loader: "ConfigLoader | None" = None) -> type[BaseAgent] | None:
    """Get an agent class by name.

    Lookup order:
    1. Local registry (.cua/agents.yaml) - if config_loader provided
    2. Built-in registry (_AGENT_REGISTRY)

    Args:
        name: Agent name to look up
        config_loader: Optional ConfigLoader for local registry lookup

    Returns:
        Agent class if found, None otherwise
    """
    # Check local registry first (from .cua/agents.yaml)
    if config_loader:
        agent_entry = config_loader.get_agent_by_name(name)
        if agent_entry:
            try:
                return load_agent_from_path(agent_entry.import_path)
            except (ValueError, ImportError, AttributeError) as e:
                # Log error but continue to check built-in registry
                print(f"Warning: Failed to load agent '{name}' from {agent_entry.import_path}: {e}")

    # Fall back to built-in registry
    return _AGENT_REGISTRY.get(name)


def list_agents(config_loader: "ConfigLoader | None" = None) -> list[str]:
    """List all registered agent names.

    Args:
        config_loader: Optional ConfigLoader to include local agents

    Returns:
        List of agent names (local + built-in, deduplicated)
    """
    agents = set(_AGENT_REGISTRY.keys())

    # Add local agents from .cua/agents.yaml
    if config_loader:
        for agent_entry in config_loader.load_agents():
            agents.add(agent_entry.name)

    return sorted(agents)

# Import agents (they will self-register via decorators)
from .cua_agent import CuaAgent
from .gemini import GeminiAgent

__all__ = [
    "BaseAgent",
    "AgentResult",
    "FailureMode",
    "CuaAgent",
    "GeminiAgent",
    "register_agent",
    "get_agent",
    "list_agents",
    "load_agent_from_path",
]
