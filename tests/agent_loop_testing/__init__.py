"""
Agent Loop Testing Framework

Tests a ComputerAgent with a static screenshot as the "VM".
The agent will repeatedly try to click on Safari, but since it's just a static image,
it will keep trying. We verify the agent loop doesn't break.

Automatically uses ComputerAgent if dependencies are available,
falls back to MockAgent for demonstration if not.
"""

from .agent_test import test_agent_loop

__all__ = [
    "test_agent_loop",
]

__version__ = "1.0.0"