#!/usr/bin/env python3
"""
Human-in-the-Loop Completion Server and UI

This module combines the FastAPI server for handling completion requests
with a Gradio UI for human interaction.
"""

import gradio as gr
from fastapi import FastAPI

from .server import app as fastapi_app
from .ui import create_ui

# Create the Gradio demo
gradio_demo = create_ui()

# Mount Gradio on FastAPI
CUSTOM_PATH = "/gradio"
app = gr.mount_gradio_app(fastapi_app, gradio_demo, path=CUSTOM_PATH)


# Add a redirect from root to Gradio UI
@fastapi_app.get("/")
async def redirect_to_ui():
    """Redirect root to Gradio UI."""
    return {
        "message": "Human Completion Server is running",
        "ui_url": "/gradio",
        "api_docs": "/docs",
    }


if __name__ == "__main__":
    import uvicorn

    print("🚀 Starting Human-in-the-Loop Completion Server...")
    print("📊 API Server: http://localhost:8002")
    print("🎨 Gradio UI: http://localhost:8002/gradio")
    print("📚 API Docs: http://localhost:8002/docs")

    uvicorn.run(app, host="0.0.0.0", port=8002)
