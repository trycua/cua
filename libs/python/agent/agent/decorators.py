"""
Decorators for agent - agent_loop decorator
"""

from typing import List, Optional

from .types import AgentConfigInfo

# Global registry
_agent_configs: List[AgentConfigInfo] = []


def register_agent(models: str, priority: int = 0):
    """
    Decorator to register an AsyncAgentConfig class.

    Args:
        models: Regex pattern to match supported models
        priority: Priority for agent selection (higher = more priority)
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
            agent_class=agent_class, models_regex=models, priority=priority
        )
        _agent_configs.append(config_info)

        # Sort by priority (highest first)
        _agent_configs.sort(key=lambda x: x.priority, reverse=True)

        return agent_class

    return decorator


def get_agent_configs() -> List[AgentConfigInfo]:
    """Get all registered agent configs"""
    return _agent_configs.copy()


def find_agent_config(model: str) -> Optional[AgentConfigInfo]:
    """Find the best matching agent config for a model"""
    for config_info in _agent_configs:
        if config_info.matches_model(model):
            return config_info
    return None
