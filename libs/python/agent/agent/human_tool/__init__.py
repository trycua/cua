"""
Human-in-the-Loop Completion Tool

This package provides a human-in-the-loop completion system that allows
AI agents to request human assistance for complex decisions or responses.

Components:
- server.py: FastAPI server with completion queue management
- ui.py: Gradio UI for human interaction
- __main__.py: Combined server and UI application

Usage:
    # Run the server and UI
    python -m agent.human_tool

    # Or run components separately
    python -m agent.human_tool.server  # API server only
    python -m agent.human_tool.ui      # UI only
"""

from .server import CompletionQueue, completion_queue
from .ui import HumanCompletionUI, create_ui

__all__ = ["CompletionQueue", "completion_queue", "HumanCompletionUI", "create_ui"]
