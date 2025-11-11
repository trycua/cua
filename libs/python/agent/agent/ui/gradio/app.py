"""
Advanced Gradio UI for Computer-Use Agent (cua-agent)

This is a Gradio interface for the Computer-Use Agent v0.4.x (cua-agent)
with an advanced UI for model selection and configuration.

Supported Agent Models:
- OpenAI: openai/computer-use-preview
- Anthropic: anthropic/claude-3-5-sonnet-20241022, anthropic/claude-3-7-sonnet-20250219
- UI-TARS: huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B
- Omniparser: omniparser+anthropic/claude-3-5-sonnet-20241022, omniparser+ollama_chat/gemma3

Requirements:
    - Mac with Apple Silicon (M1/M2/M3/M4), Linux, or Windows
    - macOS 14 (Sonoma) or newer / Ubuntu 20.04+
    - Python 3.11+
    - Lume CLI installed (https://github.com/trycua/cua)
    - OpenAI or Anthropic API key
"""

import asyncio
import json
import logging
import os
import platform
from pathlib import Path
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, Union, cast

import gradio as gr

# Import from agent package
from agent import ComputerAgent
from agent.types import AgentResponse, Messages
from computer import Computer
from gradio.components.chatbot import MetadataDict

# Global variables
global_agent = None
global_computer = None
SETTINGS_FILE = Path(".gradio_settings.json")

logging.basicConfig(level=logging.INFO)

import dotenv

if dotenv.load_dotenv():
    print(f"DEBUG - Loaded environment variables from {dotenv.find_dotenv()}")
else:
    print("DEBUG - No .env file found")


# --- Settings Load/Save Functions ---
def load_settings() -> Dict[str, Any]:
    """Loads settings from the JSON file."""
    if SETTINGS_FILE.exists():
        try:
            with open(SETTINGS_FILE, "r") as f:
                settings = json.load(f)
                if isinstance(settings, dict):
                    print(f"DEBUG - Loaded settings from {SETTINGS_FILE}")
                    return settings
        except (json.JSONDecodeError, IOError) as e:
            print(f"Warning: Could not load settings from {SETTINGS_FILE}: {e}")
    return {}


def save_settings(settings: Dict[str, Any]):
    """Saves settings to the JSON file."""
    settings.pop("provider_api_key", None)
    try:
        with open(SETTINGS_FILE, "w") as f:
            json.dump(settings, f, indent=4)
        print(f"DEBUG - Saved settings to {SETTINGS_FILE}")
    except IOError as e:
        print(f"Warning: Could not save settings to {SETTINGS_FILE}: {e}")


# # Custom Screenshot Handler for Gradio chat
# class GradioChatScreenshotHandler:
#     """Custom handler that adds screenshots to the Gradio chatbot."""

#     def __init__(self, chatbot_history: List[gr.ChatMessage]):
#         self.chatbot_history = chatbot_history
#         print("GradioChatScreenshotHandler initialized")

#     async def on_screenshot(self, screenshot_base64: str, action_type: str = "") -> None:
#         """Add screenshot to chatbot when a screenshot is taken."""
#         image_markdown = f"![Screenshot after {action_type}](data:image/png;base64,{screenshot_base64})"

#         if self.chatbot_history is not None:
#             self.chatbot_history.append(
#                 gr.ChatMessage(
#                     role="assistant",
#                     content=image_markdown,
#                     metadata={"title": f"🖥️ Screenshot - {action_type}", "status": "done"},
#                 )
#             )


# Detect platform capabilities
is_mac = platform.system().lower() == "darwin"
is_lume_available = is_mac or (os.environ.get("PYLUME_HOST", "localhost") != "localhost")

print("PYLUME_HOST: ", os.environ.get("PYLUME_HOST", "localhost"))
print("is_mac: ", is_mac)
print("Lume available: ", is_lume_available)

# Map model names to agent model strings
MODEL_MAPPINGS = {
    "openai": {
        "default": "openai/computer-use-preview",
        "OpenAI: Computer-Use Preview": "openai/computer-use-preview",
    },
    "anthropic": {
        "default": "anthropic/claude-3-7-sonnet-20250219",
        "Anthropic: Claude 4 Opus (20250514)": "anthropic/claude-opus-4-20250514",
        "Anthropic: Claude 4 Sonnet (20250514)": "anthropic/claude-sonnet-4-20250514",
        "Anthropic: Claude 3.7 Sonnet (20250219)": "anthropic/claude-3-7-sonnet-20250219",
        "Anthropic: Claude 3.5 Sonnet (20241022)": "anthropic/claude-3-5-sonnet-20241022",
    },
    "omni": {
        "default": "omniparser+openai/gpt-4o",
        "OMNI: OpenAI GPT-4o": "omniparser+openai/gpt-4o",
        "OMNI: OpenAI GPT-4o mini": "omniparser+openai/gpt-4o-mini",
        "OMNI: Claude 3.7 Sonnet (20250219)": "omniparser+anthropic/claude-3-7-sonnet-20250219",
        "OMNI: Claude 3.5 Sonnet (20241022)": "omniparser+anthropic/claude-3-5-sonnet-20241022",
    },
    "uitars": {
        "default": "huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B" if is_mac else "ui-tars",
        "huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B": "huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B",
    },
}


def get_model_string(model_name: str, loop_provider: str) -> str:
    """Determine the agent model string based on the input."""
    if model_name == "Custom model (OpenAI compatible API)":
        return "custom_oaicompat"
    elif model_name == "Custom model (ollama)":
        return "custom_ollama"
    elif loop_provider == "OMNI-OLLAMA" or model_name.startswith("OMNI: Ollama "):
        if model_name.startswith("OMNI: Ollama "):
            ollama_model = model_name.split("OMNI: Ollama ", 1)[1]
            return f"omniparser+ollama_chat/{ollama_model}"
        return "omniparser+ollama_chat/llama3"

    # Map based on loop provider
    mapping = MODEL_MAPPINGS.get(loop_provider.lower(), MODEL_MAPPINGS["openai"])
    return mapping.get(model_name, mapping["default"])


def get_ollama_models() -> List[str]:
    """Get available models from Ollama if installed."""
    try:
        import subprocess

        result = subprocess.run(["ollama", "list"], capture_output=True, text=True)
        if result.returncode == 0:
            lines = result.stdout.strip().split("\n")
            if len(lines) < 2:
                return []
            models = []
            for line in lines[1:]:
                parts = line.split()
                if parts:
                    model_name = parts[0]
                    models.append(f"OMNI: Ollama {model_name}")
            return models
        return []
    except Exception as e:
        logging.error(f"Error getting Ollama models: {e}")
        return []


def create_computer_instance(
    verbosity: int = logging.INFO,
    os_type: str = "macos",
    provider_type: str = "lume",
    name: Optional[str] = None,
    api_key: Optional[str] = None,
) -> Computer:
    """Create or get the global Computer instance."""
    global global_computer
    if global_computer is None:
        if provider_type == "localhost":
            global_computer = Computer(
                verbosity=verbosity, os_type=os_type, use_host_computer_server=True
            )
        else:
            global_computer = Computer(
                verbosity=verbosity,
                os_type=os_type,
                provider_type=provider_type,
                name=name if name else "",
                api_key=api_key,
            )
    return global_computer


def create_agent(
    model_string: str,
    save_trajectory: bool = True,
    only_n_most_recent_images: int = 3,
    verbosity: int = logging.INFO,
    custom_model_name: Optional[str] = None,
    computer_os: str = "macos",
    computer_provider: str = "lume",
    computer_name: Optional[str] = None,
    computer_api_key: Optional[str] = None,
    max_trajectory_budget: Optional[float] = None,
) -> ComputerAgent:
    """Create or update the global agent with the specified parameters."""
    global global_agent

    # Create the computer
    computer = create_computer_instance(
        verbosity=verbosity,
        os_type=computer_os,
        provider_type=computer_provider,
        name=computer_name,
        api_key=computer_api_key,
    )

    # Handle custom models
    if model_string == "custom_oaicompat" and custom_model_name:
        model_string = custom_model_name
    elif model_string == "custom_ollama" and custom_model_name:
        model_string = f"omniparser+ollama_chat/{custom_model_name}"

    # Create agent kwargs
    agent_kwargs = {
        "model": model_string,
        "tools": [computer],
        "only_n_most_recent_images": only_n_most_recent_images,
        "verbosity": verbosity,
    }

    if save_trajectory:
        agent_kwargs["trajectory_dir"] = "trajectories"

    if max_trajectory_budget:
        agent_kwargs["max_trajectory_budget"] = {
            "max_budget": max_trajectory_budget,
            "raise_error": True,
        }

    global_agent = ComputerAgent(**agent_kwargs)
    return global_agent


def launch_ui():
    """Standalone function to launch the Gradio app."""
    from agent.ui.gradio.ui_components import create_gradio_ui

    print("Starting Gradio app for CUA Agent...")
    demo = create_gradio_ui()
    demo.launch(share=False, inbrowser=True)


if __name__ == "__main__":
    launch_ui()
