"""
Agent Loop Testing Framework

Tests AI models with a minimal agent loop using a mock computer
that only provides screenshot functionality.

Automatically supports pluggable AI models including:
- Test models (no external dependencies)
- CUA models (Anthropic, OpenAI, etc.)
- Custom models (implement AIModelInterface)
"""

from .agent_test import test_agent_loop_with_model, create_ai_model
from .ai_interface import AIModelInterface, AgentLoop, AgentMessage, AgentResponse
from .mock_computer import MinimalMockComputer, MockComputerInterface

__all__ = [
    "test_agent_loop_with_model",
    "create_ai_model", 
    "AIModelInterface",
    "AgentLoop",
    "AgentMessage",
    "AgentResponse",
    "MinimalMockComputer",
    "MockComputerInterface",
]

__version__ = "2.0.0"