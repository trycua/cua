"""
Main entry point for computer.ui module.

This allows running the computer UI with:
    python -m computer.ui

Instead of:
    python -m computer.ui.gradio.app
"""

from .gradio.app import create_gradio_ui

if __name__ == "__main__":
    app = create_gradio_ui()
    app.launch()
