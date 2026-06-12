"""
Decorators for agent - agent_loop decorator
"""

from typing import List, Optional

from .types import AgentConfigInfo

# Global registry
_agent_configs: List[AgentConfigInfo] = []


def register_agent(models: str, priority: int = 0, tool_type: Optional[str] = None):
    """
    Decorator to register an AsyncAgentConfig class.

    Args:
        models: Regex pattern to match supported models
        priority: Priority for agent selection (higher = more priority)
        tool_type: Required tool type for this model ("browser" | "mobile" | None).
                   Specialized models (like FARA) declare their required tool type,
                   and ComputerAgent will auto-wrap tools accordingly.
                   General models (like Claude) leave this as None for full flexibility.
    """

    def decorator(agent_class: type):
        # Validate that the class implements AsyncAgentConfig protocol
        if not hasattr(agent_class, "predict_step"):
            raise ValueError(
                f"Agent class {agent_class.__name__} must implement predict_step method"
            )
        if not hasattr(agent_class, "predict_click"):
            raise ValueError(
                f"Agent class {agent_class.__name__} must implement predict_click method"
            )
        if not hasattr(agent_class, "get_capabilities"):
            raise ValueError(
                f"Agent class {agent_class.__name__} must implement get_capabilities method"
            )

        # Register the agent config
        config_info = AgentConfigInfo(
            agent_class=agent_class,
            models_regex=models,
            priority=priority,
            tool_type=tool_type,
        )
        _agent_configs.append(config_info)

        # Sort by priority (highest first)
        _agent_configs.sort(key=lambda x: x.priority, reverse=True)

        return agent_class

    return decorator


def get_agent_configs() -> List[AgentConfigInfo]:
    """Get all registered agent configs"""
    return _agent_configs.copy()


def _strip_cua_prefix(model: str) -> str:
    """Strip the ``cua/<provider>/`` routing prefix so the bare model name
    can be matched against registered agent patterns.

    Examples:
        cua/google/gemini-3-flash-preview  ->  gemini-3-flash-preview
        cua/anthropic/claude-sonnet-4-6    ->  claude-sonnet-4-6
        gemini-3-flash-preview             ->  gemini-3-flash-preview  (unchanged)
    """
    parts = model.split("/")
    if parts[0] == "cua" and len(parts) >= 3:
        return "/".join(parts[2:])
    return model


def find_agent_config(model: str) -> Optional[AgentConfigInfo]:
    """Find the best matching agent config for a model.

    For each registered config (checked in priority order), tries the
    original model string first and then the bare model name with the
    ``cua/<provider>/`` routing prefix stripped.  This ensures that
    routed models (e.g. ``cua/google/gemini-3-flash-preview``) resolve
    to the same agent loop as their bare counterparts.
    """
    stripped = _strip_cua_prefix(model)
    for config_info in _agent_configs:
        if config_info.matches_model(model):
            return config_info
        if stripped != model and config_info.matches_model(stripped):
            return config_info
    return None
