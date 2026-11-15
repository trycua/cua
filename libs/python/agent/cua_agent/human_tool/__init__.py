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
    python -m cua_agent.human_tool

    # Or run components separately
    python -m cua_agent.human_tool.server  # API server only
    python -m cua_agent.human_tool.ui      # UI only

Security:
    The server listens on 127.0.0.1 only. To accept connections from other
    machines pass ``--host 0.0.0.0`` (or set CUA_HUMAN_TOOL_HOST).

    Every API route needs ``Authorization: Bearer <token>``, and the web page
    asks for the same token as its password. The server makes a random token
    at each start and saves it to ~/.cua/human_tool/token (owner-only), where
    the agent adapter and UI on the same machine pick it up automatically.
    To use a client on another machine, set CUA_HUMAN_TOOL_TOKEN to the same
    value on both sides.
"""

from .server import CompletionQueue, completion_queue
from .ui import HumanCompletionUI, create_ui

__all__ = ["CompletionQueue", "completion_queue", "HumanCompletionUI", "create_ui"]
