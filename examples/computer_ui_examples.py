#!/usr/bin/env python3
"""
Simple example script for the Computer Interface Gradio UI.

This script launches the advanced Gradio UI for the Computer Interface
with full model selection and configuration options.
It can be run directly from the command line.
"""


from utils import load_dotenv_files

load_dotenv_files()

# Import the create_gradio_ui function
from computer.ui.gradio.app import create_gradio_ui

if __name__ == "__main__":
    print("Launching Computer Interface Gradio UI with advanced features...")
    app = create_gradio_ui()
    app.launch(
        share=False,
        server_name="0.0.0.0",
        server_port=7860,
    )

    # Optional: Using the saved dataset
    # import datasets
    # from computer.ui.utils import convert_to_unsloth
    # ds = datasets.load_dataset("ddupont/highquality-cua-demonstrations")
    # ds = convert_to_unsloth(ds)
