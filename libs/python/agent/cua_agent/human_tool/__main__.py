#!/usr/bin/env python3
"""
Human-in-the-Loop Completion Server and UI

This module combines the FastAPI server for handling completion requests
with a Gradio UI for human interaction.

The server listens on 127.0.0.1 by default and every API route needs a bearer
token (see ``auth.py``). The web page asks for the same token as its password.
"""

import argparse
from typing import List, Optional

import gradio as gr

from ..human_auth import announce, gradio_auth, resolve_host, server_token
from .server import app as fastapi_app
from .ui import create_ui

DEFAULT_PORT = 8002

# Create the Gradio demo
gradio_demo = create_ui()

# Mount Gradio on FastAPI. The page asks for the server token as its password.
CUSTOM_PATH = "/gradio"
app = gr.mount_gradio_app(
    fastapi_app,
    gradio_demo,
    path=CUSTOM_PATH,
    auth=gradio_auth(server_token),
)


# Add a redirect from root to Gradio UI
@fastapi_app.get("/")
async def redirect_to_ui():
    """Redirect root to Gradio UI."""
    return {
        "message": "Human Completion Server is running",
        "ui_url": "/gradio",
        "api_docs": "/docs",
    }


def main(argv: Optional[List[str]] = None) -> None:
    import uvicorn

    parser = argparse.ArgumentParser(description="Human-in-the-loop server and UI")
    parser.add_argument(
        "--host",
        default=None,
        help="Address to listen on (default: 127.0.0.1, or $CUA_HUMAN_TOOL_HOST). "
        "Use 0.0.0.0 to allow other machines to connect.",
    )
    args = parser.parse_args(argv)

    host = resolve_host(args.host)
    server_token()  # create the token (and its file) before accepting requests

    print("🚀 Starting Human-in-the-Loop Completion Server...")
    print(f"📊 API Server: http://{host}:{DEFAULT_PORT}")
    print(f"🎨 Gradio UI: http://{host}:{DEFAULT_PORT}{CUSTOM_PATH}  (password: the token)")
    print(f"📚 API Docs: http://{host}:{DEFAULT_PORT}/docs")
    announce(host, DEFAULT_PORT)

    uvicorn.run(app, host=host, port=DEFAULT_PORT)


if __name__ == "__main__":
    main()
