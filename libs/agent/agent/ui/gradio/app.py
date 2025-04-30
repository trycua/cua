"""
Advanced Gradio UI for Computer-Use Agent

This is a Gradio interface for the Computer-Use Agent
with an advanced UI for model selection and configuration.

Supported Agent Loops and Models:
- AgentLoop.OPENAI: Uses OpenAI Operator CUA model
  • computer_use_preview

- AgentLoop.ANTHROPIC: Uses Anthropic Computer-Use models
  • claude-3-5-sonnet-20240620
  • claude-3-7-sonnet-20250219

- AgentLoop.OMNI (experimental): Uses OmniParser for element pixel-detection
  • claude-3-5-sonnet-20240620
  • claude-3-7-sonnet-20250219
  • gpt-4.5-preview
  • gpt-4o
  • gpt-4

Requirements:
    - Mac with Apple Silicon (M1/M2/M3/M4)
    - macOS 14 (Sonoma) or newer
    - Python 3.10+
    - Lume CLI installed (https://github.com/trycua/cua)
    - OpenAI or Anthropic API key
"""

import os
import asyncio
import logging
import json
from pathlib import Path
from typing import Dict, List, Optional, AsyncGenerator, Any, Tuple, Union
import gradio as gr
from gradio.components.chatbot import MetadataDict
from typing import cast

# Import from agent package
from agent.core.types import AgentResponse
from agent.core.callbacks import DefaultCallbackHandler
from agent.providers.omni.parser import ParseResult
from computer import Computer

from agent import ComputerAgent, AgentLoop, LLM, LLMProvider

# Global variables
global_agent = None
global_computer = None
SETTINGS_FILE = Path(".gradio_settings.json")

# We'll use asyncio.run() instead of a persistent event loop


# --- Settings Load/Save Functions ---
def load_settings() -> Dict[str, Any]:
    """Loads settings from the JSON file."""
    if SETTINGS_FILE.exists():
        try:
            with open(SETTINGS_FILE, "r") as f:
                settings = json.load(f)
                # Basic validation (can be expanded)
                if isinstance(settings, dict):
                    print(f"DEBUG - Loaded settings from {SETTINGS_FILE}")
                    return settings
        except (json.JSONDecodeError, IOError) as e:
            print(f"Warning: Could not load settings from {SETTINGS_FILE}: {e}")
    return {}


def save_settings(settings: Dict[str, Any]):
    """Saves settings to the JSON file."""
    # Ensure sensitive keys are not saved
    settings.pop("provider_api_key", None)
    try:
        with open(SETTINGS_FILE, "w") as f:
            json.dump(settings, f, indent=4)
        print(f"DEBUG - Saved settings to {SETTINGS_FILE}")
    except IOError as e:
        print(f"Warning: Could not save settings to {SETTINGS_FILE}: {e}")


# --- End Settings Load/Save ---


# Custom Screenshot Handler for Gradio chat
class GradioChatScreenshotHandler(DefaultCallbackHandler):
    """Custom handler that adds screenshots to the Gradio chatbot and updates annotated image."""

    def __init__(self, chatbot_history: List[gr.ChatMessage]):
        """Initialize with reference to chat history and annotated image component.

        Args:
            chatbot_history: Reference to the Gradio chatbot history list
            annotated_image: Reference to the annotated image component
        """
        self.chatbot_history = chatbot_history
        print("GradioChatScreenshotHandler initialized")

    async def on_screenshot(
        self,
        screenshot_base64: str,
        action_type: str = "",
        parsed_screen: Optional[ParseResult] = None,
    ) -> None:
        """Add screenshot to chatbot when a screenshot is taken and update the annotated image.

        Args:
            screenshot_base64: Base64 encoded screenshot
            action_type: Type of action that triggered the screenshot

        Returns:
            Original screenshot (does not modify it)
        """
        # Create a markdown image element for the screenshot
        image_markdown = (
            f"![Screenshot after {action_type}](data:image/png;base64,{screenshot_base64})"
        )

        # Simply append the screenshot as a new message
        if self.chatbot_history is not None:
            self.chatbot_history.append(
                gr.ChatMessage(
                    role="assistant",
                    content=image_markdown,
                    metadata={"title": f"🖥️ Screenshot - {action_type}", "status": "done"},
                )
            )


# Map model names to specific provider model names
MODEL_MAPPINGS = {
    "openai": {
        # Default to operator CUA model
        "default": "computer_use_preview",
        # Map standard OpenAI model names to CUA-specific model names
        "gpt-4-turbo": "computer_use_preview",
        "gpt-4o": "computer_use_preview",
        "gpt-4": "computer_use_preview",
        "gpt-4.5-preview": "computer_use_preview",
        "gpt-4o-mini": "gpt-4o-mini",
    },
    "anthropic": {
        # Default to newest model
        "default": "claude-3-7-sonnet-20250219",
        # Specific Claude models for CUA
        "claude-3-5-sonnet-20240620": "claude-3-5-sonnet-20240620",
        "claude-3-7-sonnet-20250219": "claude-3-7-sonnet-20250219",
        # Map standard model names to CUA-specific model names
        "claude-3-opus": "claude-3-7-sonnet-20250219",
        "claude-3-sonnet": "claude-3-5-sonnet-20240620",
        "claude-3-5-sonnet": "claude-3-5-sonnet-20240620",
        "claude-3-7-sonnet": "claude-3-7-sonnet-20250219",
    },
    "omni": {
        # OMNI works with any of these models
        "default": "gpt-4o",
        "gpt-4o": "gpt-4o",
        "gpt-4o-mini": "gpt-4o-mini",
        "gpt-4": "gpt-4",
        "gpt-4.5-preview": "gpt-4.5-preview",
        "claude-3-5-sonnet-20240620": "claude-3-5-sonnet-20240620",
        "claude-3-7-sonnet-20250219": "claude-3-7-sonnet-20250219",
    },
    "uitars": {
        # UI-TARS models default to custom endpoint
        "default": "ByteDance-Seed/UI-TARS-1.5-7B",
    },
    "ollama": {
        # For Ollama models, we keep the original name
        "default": "llama3",  # A common default model
        # Don't map other models - we'll use the original name
    },
    "oaicompat": {
        # Default for OpenAI-compatible providers like VLLM
        "default": "Qwen2.5-VL-7B-Instruct",
    },
}


def get_provider_and_model(model_name: str, loop_provider: str) -> tuple:
    """
    Determine the provider and actual model name to use based on the input.

    Args:
        model_name: The requested model name
        loop_provider: The requested agent loop provider

    Returns:
        tuple: (provider, model_name_to_use, agent_loop)
    """
    # Get the agent loop
    loop_provider_map = {
        "OPENAI": AgentLoop.OPENAI,
        "ANTHROPIC": AgentLoop.ANTHROPIC,
        "OMNI": AgentLoop.OMNI,
        "OMNI-OLLAMA": AgentLoop.OMNI,  # Special case for Ollama models with OMNI parser
        "UITARS": AgentLoop.UITARS,     # UI-TARS implementation
    }
    agent_loop = loop_provider_map.get(loop_provider, AgentLoop.OPENAI)

    # Set up the provider and model based on the loop and model_name
    if agent_loop == AgentLoop.OPENAI:
        provider = LLMProvider.OPENAI
        model_name_to_use = MODEL_MAPPINGS["openai"].get(
            model_name.lower(), MODEL_MAPPINGS["openai"]["default"]
        )
    elif agent_loop == AgentLoop.ANTHROPIC:
        provider = LLMProvider.ANTHROPIC
        model_name_to_use = MODEL_MAPPINGS["anthropic"].get(
            model_name.lower(), MODEL_MAPPINGS["anthropic"]["default"]
        )
    elif agent_loop == AgentLoop.OMNI:
        # Determine provider and clean model name based on the full string from UI
        cleaned_model_name = model_name  # Default to using the name as-is (for custom)

        if model_name == "Custom model...":
            # Actual model name comes from custom_model_value via model_to_use.
            # Assume OAICOMPAT for custom models unless overridden by URL/key later?
            # get_provider_and_model determines the *initial* provider/model.
            # The custom URL/key in process_response ultimately dictates the OAICOMPAT setup.
            provider = LLMProvider.OAICOMPAT
            # We set cleaned_model_name below outside the checks based on model_to_use
            cleaned_model_name = ""  # Placeholder, will be set by custom value later
        elif model_name.startswith("OMNI: Ollama "):
            provider = LLMProvider.OLLAMA
            # Extract the part after "OMNI: Ollama "
            cleaned_model_name = model_name.split("OMNI: Ollama ", 1)[1]
        elif model_name.startswith("OMNI: Claude "):
            provider = LLMProvider.ANTHROPIC
            # Extract the canonical model name based on the UI string
            # e.g., "OMNI: Claude 3.7 Sonnet (20250219)" -> "3.7 Sonnet" and "20250219"
            parts = model_name.split(" (")
            model_key_part = parts[0].replace("OMNI: Claude ", "")
            date_part = parts[1].replace(")", "") if len(parts) > 1 else ""

            # Normalize the extracted key part for comparison
            # "3.7 Sonnet" -> "37sonnet"
            model_key_part_norm = model_key_part.lower().replace(".", "").replace(" ", "")

            cleaned_model_name = MODEL_MAPPINGS["omni"]["default"]  # Default if not found
            # Find the canonical name in the main Anthropic map
            for key_anthropic, val_anthropic in MODEL_MAPPINGS["anthropic"].items():
                # Normalize the canonical key for comparison
                # "claude-3-7-sonnet-20250219" -> "claude37sonnet20250219"
                key_anthropic_norm = key_anthropic.lower().replace("-", "")

                # Check if the normalized canonical key starts with "claude" + normalized extracted part
                # AND contains the date part.
                if (
                    key_anthropic_norm.startswith("claude" + model_key_part_norm)
                    and date_part in key_anthropic_norm
                ):
                    cleaned_model_name = (
                        val_anthropic  # Use the canonical name like "claude-3-7-sonnet-20250219"
                    )
                    break
        elif model_name.startswith("OMNI: OpenAI "):
            provider = LLMProvider.OPENAI
            # Extract the model part, e.g., "GPT-4o mini"
            model_key_part = model_name.replace("OMNI: OpenAI ", "")
            # Normalize the extracted part: "gpt4omini"
            model_key_part_norm = model_key_part.lower().replace("-", "").replace(" ", "")

            cleaned_model_name = MODEL_MAPPINGS["omni"]["default"]  # Default if not found
            # Find the canonical name in the main OMNI map for OpenAI models
            for key_omni, val_omni in MODEL_MAPPINGS["omni"].items():
                # Normalize the omni map key: "gpt-4o-mini" -> "gpt4omini"
                key_omni_norm = key_omni.lower().replace("-", "").replace(" ", "")
                # Check if the normalized omni key matches the normalized extracted part
                if key_omni_norm == model_key_part_norm:
                    cleaned_model_name = (
                        val_omni  # Use the value from the OMNI map (e.g., gpt-4o-mini)
                    )
                    break
            # Note: No fallback needed here as we explicitly check against omni keys

        else:  # Handles unexpected formats or the raw custom name if "Custom model..." selected
            # Should only happen if user selected "Custom model..."
            # Or if a model name format isn't caught above
            provider = LLMProvider.OAICOMPAT
            cleaned_model_name = (
                model_name.strip() if model_name.strip() else MODEL_MAPPINGS["oaicompat"]["default"]
            )

        # Assign the determined model name
        model_name_to_use = cleaned_model_name
        # agent_loop remains AgentLoop.OMNI
    elif agent_loop == AgentLoop.UITARS:
        provider = LLMProvider.OAICOMPAT
        model_name_to_use = MODEL_MAPPINGS["uitars"]["default"]  # Default 
    else:
        # Default to OpenAI if unrecognized loop
        provider = LLMProvider.OPENAI
        model_name_to_use = MODEL_MAPPINGS["openai"]["default"]
        agent_loop = AgentLoop.OPENAI

    return provider, model_name_to_use, agent_loop


def get_ollama_models() -> List[str]:
    """Get available models from Ollama if installed."""
    try:
        import subprocess

        result = subprocess.run(["ollama", "list"], capture_output=True, text=True)
        if result.returncode == 0:
            lines = result.stdout.strip().split("\n")
            if len(lines) < 2:  # No models or just header
                return []

            models = []
            # Skip header line
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

def create_computer_instance(verbosity: int = logging.INFO) -> Computer:
    """Create or get the global Computer instance."""
    global global_computer

    if global_computer is None:
        global_computer = Computer(verbosity=verbosity)

    return global_computer


def create_agent(
    provider: LLMProvider,
    agent_loop: AgentLoop,
    model_name: str,
    api_key: Optional[str] = None,
    save_trajectory: bool = True,
    only_n_most_recent_images: int = 3,
    verbosity: int = logging.INFO,
    use_oaicompat: bool = False,
    provider_base_url: Optional[str] = None,
) -> ComputerAgent:
    """Create or update the global agent with the specified parameters."""
    global global_agent

    # Create the computer if not already done
    computer = create_computer_instance(verbosity=verbosity)

    # Get API key from environment if not provided
    if api_key is None:
        if provider == LLMProvider.OPENAI:
            api_key = os.environ.get("OPENAI_API_KEY", "")
        elif provider == LLMProvider.ANTHROPIC:
            api_key = os.environ.get("ANTHROPIC_API_KEY", "")

    # Use provided provider_base_url if available, otherwise use default
    default_base_url = "http://localhost:1234/v1" if use_oaicompat else None
    custom_base_url = provider_base_url or default_base_url

    if use_oaicompat:
        # Special handling for OAICOMPAT - use OAICOMPAT provider with custom base URL
        print(f"DEBUG - Creating OAICOMPAT agent with model: {model_name}, URL: {custom_base_url}")
        llm = LLM(
            provider=LLMProvider.OAICOMPAT,  # Set to OAICOMPAT instead of using original provider
            name=model_name,
            provider_base_url=custom_base_url,
        )
        print(f"DEBUG - LLM provider is now: {llm.provider}, base URL: {llm.provider_base_url}")
        # Note: Don't pass use_oaicompat to the agent, as it doesn't accept this parameter
    elif provider == LLMProvider.OAICOMPAT:
        # This path is unlikely to be taken with our current approach
        llm = LLM(provider=provider, name=model_name, provider_base_url=custom_base_url)
    else:
        # For other providers, just use standard parameters
        llm = LLM(provider=provider, name=model_name)

    # Create or update the agent
    global_agent = ComputerAgent(
        computer=computer,
        loop=agent_loop,
        model=llm,
        api_key=api_key,
        save_trajectory=save_trajectory,
        only_n_most_recent_images=only_n_most_recent_images,
        verbosity=verbosity,
    )

    return global_agent

def create_gradio_ui(
    provider_name: str = "openai",
    model_name: str = "gpt-4o",
) -> gr.Blocks:
    """Create a Gradio UI for the Computer-Use Agent.

    Args:
        provider_name: The provider to use (e.g., "openai", "anthropic")
        model_name: The model to use (e.g., "gpt-4o", "claude-3-7-sonnet")

    Returns:
        A Gradio Blocks application
    """
    # --- Load Settings ---
    saved_settings = load_settings()
    # --- End Load Settings ---

    # Check for API keys
    openai_api_key = os.environ.get("OPENAI_API_KEY", "")
    anthropic_api_key = os.environ.get("ANTHROPIC_API_KEY", "")

    # Prepare model choices based on available API keys
    openai_models = []
    anthropic_models = []
    omni_models = []

    if openai_api_key:
        openai_models = ["OpenAI: Computer-Use Preview"]
        omni_models += [
            "OMNI: OpenAI GPT-4o",
            "OMNI: OpenAI GPT-4o mini",
            "OMNI: OpenAI GPT-4.5-preview",
        ]

    if anthropic_api_key:
        anthropic_models = [
            "Anthropic: Claude 3.7 Sonnet (20250219)",
            "Anthropic: Claude 3.5 Sonnet (20240620)",
        ]
        omni_models += ["OMNI: Claude 3.7 Sonnet (20250219)", "OMNI: Claude 3.5 Sonnet (20240620)"]

    # Get Ollama models for OMNI
    ollama_models = get_ollama_models()
    if ollama_models:
        omni_models += ollama_models

    # Format model choices
    provider_to_models = {
        "OPENAI": openai_models,
        "ANTHROPIC": anthropic_models,
        "OMNI": omni_models + ["Custom model..."],  # Add custom model option
        "UITARS": ["Custom model..."],  # UI-TARS options
    }

    # --- Apply Saved Settings (override defaults if available) ---
    initial_loop = saved_settings.get("agent_loop", "OMNI")
    # Ensure the saved model is actually available in the choices for the loaded loop
    available_models_for_loop = provider_to_models.get(initial_loop, [])
    saved_model_choice = saved_settings.get("model_choice")
    if saved_model_choice and saved_model_choice in available_models_for_loop:
        initial_model = saved_model_choice
    else:
        # If saved model isn't valid for the loop, reset to default for that loop
        if initial_loop == "OPENAI":
            initial_model = (
                "OpenAI: Computer-Use Preview" if openai_models else "No models available"
            )
        elif initial_loop == "ANTHROPIC":
            initial_model = anthropic_models[0] if anthropic_models else "No models available"
        else:  # OMNI
            initial_model = omni_models[0] if omni_models else "No models available"
            if "Custom model..." in available_models_for_loop:
                initial_model = (
                    "Custom model..."  # Default to custom if available and no other default fits
                )

    initial_custom_model = saved_settings.get("custom_model", "Qwen2.5-VL-7B-Instruct")
    initial_provider_base_url = saved_settings.get("provider_base_url", "http://localhost:1234/v1")
    initial_save_trajectory = saved_settings.get("save_trajectory", True)
    initial_recent_images = saved_settings.get("recent_images", 3)
    # --- End Apply Saved Settings ---

    # Example prompts
    example_messages = [
        "Create a Python virtual environment, install pandas and matplotlib, then plot stock data",
        "Open a PDF in Preview, add annotations, and save it as a compressed version",
        "Open Safari, search for 'macOS automation tools', and save the first three results as bookmarks",
        "Configure SSH keys and set up a connection to a remote server",
    ]

    # Function to update model choices based on agent loop selection
    def update_model_choices(loop):
        models = provider_to_models.get(loop, [])
        if loop == "OMNI":
            # For OMNI, include the custom model option
            if not models:
                models = ["Custom model..."]
            elif "Custom model..." not in models:
                models.append("Custom model...")

            return gr.update(
                choices=models, value=models[0] if models else "Custom model...", interactive=True
            )
        else:
            # For other providers, use standard dropdown without custom option
            if not models:
                return gr.update(
                    choices=["No models available"], value="No models available", interactive=True
                )
            return gr.update(choices=models, value=models[0] if models else None, interactive=True)

    # Create the Gradio interface with advanced UI
    with gr.Blocks(title="Computer-Use Agent") as demo:
        with gr.Row():
            # Left column for settings
            with gr.Column(scale=1):
                # Logo with theme-aware styling
                gr.HTML(
                    """
                    <style>
                    .light-logo, .dark-logo {
                        display: block;
                        margin: 0 auto;
                        width: 80px;
                    }
                    /* Hide dark logo in light mode */
                    .dark-logo {
                        display: none;
                    }
                    /* In dark mode, hide light logo and show dark logo */
                    .dark .light-logo {
                        display: none;
                    }
                    .dark .dark-logo {
                        display: block;
                    }
                    </style>
                    <div style="display: flex; justify-content: center; margin-bottom: 0.5em">
                        <img class="light-logo" alt="CUA Logo" 
                             src="https://github.com/trycua/cua/blob/main/img/logo_black.png?raw=true" />
                        <img class="dark-logo" alt="CUA Logo" 
                             src="https://github.com/trycua/cua/blob/main/img/logo_white.png?raw=true" />
                    </div>
                    """
                )

                # Add installation prerequisites as a collapsible section
                with gr.Accordion("Prerequisites & Installation", open=False):
                    gr.Markdown(
                        """
                    ## Prerequisites
                    
                    Before using the Computer-Use Agent, you need to set up the Lume daemon and pull the macOS VM image.
                    
                    ### 1. Install Lume daemon
                    
                    While a lume binary is included with Computer, we recommend installing the standalone version with brew, and starting the lume daemon service:
                    
                    ```bash
                    sudo /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
                    ```
                    
                    ### 2. Start the Lume daemon service
                    
                    In a separate terminal:
                    
                    ```bash
                    lume serve
                    ```
                    
                    ### 3. Pull the pre-built macOS image
                    
                    ```bash
                    lume pull macos-sequoia-cua:latest
                    ```
                    
                    Initial download requires 80GB storage, but reduces to ~30GB after first run due to macOS's sparse file system.
                    
                    VMs are stored in `~/.lume`, and locally cached images are stored in `~/.lume/cache`.
                    
                    ### 4. Test the sandbox
                    
                    ```bash
                    lume run macos-sequoia-cua:latest
                    ```
                    
                    For more detailed instructions, visit the [CUA GitHub repository](https://github.com/trycua/cua).
                    """
                    )

                with gr.Accordion("Configuration", open=True):
                    # Configuration options
                    agent_loop = gr.Dropdown(
                        choices=["OPENAI", "ANTHROPIC", "OMNI", "UITARS"],
                        label="Agent Loop",
                        value=initial_loop,
                        info="Select the agent loop provider",
                    )

                    # Create model selection dropdown with custom value support for OMNI
                    model_choice = gr.Dropdown(
                        choices=provider_to_models.get(initial_loop, ["No models available"]),
                        label="LLM Provider and Model",
                        value=initial_model,
                        info="Select model or choose 'Custom model...' to enter a custom name",
                        interactive=True,
                    )

                    # Add custom model textbox (only visible when "Custom model..." is selected)
                    custom_model = gr.Textbox(
                        label="Custom Model Name",
                        placeholder="Enter custom model name (e.g., Qwen2.5-VL-7B-Instruct)",
                        value=initial_custom_model,
                        visible=(initial_model == "Custom model..."),
                        interactive=True,
                    )

                    # Add custom provider base URL textbox (only visible when "Custom model..." is selected)
                    provider_base_url = gr.Textbox(
                        label="Provider Base URL",
                        placeholder="Enter provider base URL (e.g., http://localhost:1234/v1)",
                        value=initial_provider_base_url,
                        visible=(initial_model == "Custom model..."),
                        interactive=True,
                    )

                    # Add custom API key textbox (only visible when "Custom model..." is selected)
                    provider_api_key = gr.Textbox(
                        label="Provider API Key",
                        placeholder="Enter provider API key (if required)",
                        value="",
                        visible=(initial_model == "Custom model..."),
                        interactive=True,
                        type="password",
                    )

                    save_trajectory = gr.Checkbox(
                        label="Save Trajectory",
                        value=initial_save_trajectory,
                        info="Save the agent's trajectory for debugging",
                        interactive=True,
                    )

                    recent_images = gr.Slider(
                        label="Recent Images",
                        minimum=1,
                        maximum=10,
                        value=initial_recent_images,
                        step=1,
                        info="Number of recent images to keep in context",
                        interactive=True,
                    )

            # Right column for chat interface
            with gr.Column(scale=2):
                # Add instruction text before the chat interface
                gr.Markdown(
                    "Ask me to perform tasks in a virtual macOS environment.<br>Built with <a href='https://github.com/trycua/cua' target='_blank'>github.com/trycua/cua</a>."
                )

                chatbot_history = gr.Chatbot(type="messages")
                msg = gr.Textbox(
                    placeholder="Ask me to perform tasks in a virtual macOS environment"
                )
                clear = gr.Button("Clear")

                # Add examples
                example_group = gr.Examples(examples=example_messages, inputs=msg)

                # Function to handle chat submission
                def chat_submit(message, history):
                    # Add user message to history
                    history.append(gr.ChatMessage(role="user", content=message))
                    return "", history

                # Function to process agent response after user input
                async def process_response(
                    history,
                    model_choice_value,
                    custom_model_value,
                    agent_loop_choice,
                    save_traj,
                    recent_imgs,
                    custom_url_value=None,
                    custom_api_key=None,
                ):
                    if not history:
                        yield history
                        return

                    # Get the last user message
                    last_user_message = history[-1]["content"]

                    # Determine the model name string to analyze: custom or from dropdown
                    model_string_to_analyze = (
                        custom_model_value
                        if model_choice_value == "Custom model..."
                        else model_choice_value  # Use the full UI string initially
                    )

                    # Determine if this is a custom model selection
                    is_custom_model_selected = model_choice_value == "Custom model..."

                    try:
                        # Get the provider, *cleaned* model name, and agent loop type
                        provider, cleaned_model_name_from_func, agent_loop_type = (
                            get_provider_and_model(model_string_to_analyze, agent_loop_choice)
                        )
                        
                        print(f"provider={provider} cleaned_model_name_from_func={cleaned_model_name_from_func} agent_loop_type={agent_loop_type} agent_loop_choice={agent_loop_choice}")

                        # Determine the final model name to send to the agent
                        # If custom selected, use the custom text box value, otherwise use the cleaned name
                        final_model_name_to_send = (
                            custom_model_value
                            if is_custom_model_selected
                            else cleaned_model_name_from_func
                        )

                        # Determine if OAICOMPAT should be used (only if custom model explicitly selected)
                        is_oaicompat = is_custom_model_selected

                        # Get API key based on provider determined by get_provider_and_model
                        if is_oaicompat and custom_api_key:
                            # Use custom API key if provided for custom model
                            api_key = custom_api_key
                            print(
                                f"DEBUG - Using custom API key for model: {final_model_name_to_send}"
                            )
                        elif provider == LLMProvider.OPENAI:
                            api_key = openai_api_key or os.environ.get("OPENAI_API_KEY", "")
                        elif provider == LLMProvider.ANTHROPIC:
                            api_key = anthropic_api_key or os.environ.get("ANTHROPIC_API_KEY", "")
                        else:
                            # For Ollama or default OAICOMPAT (without custom key), no key needed/expected
                            api_key = ""

                        # --- Save Settings Before Running Agent ---
                        current_settings = {
                            "agent_loop": agent_loop_choice,
                            "model_choice": model_choice_value,
                            "custom_model": custom_model_value,
                            "provider_base_url": custom_url_value,
                            "save_trajectory": save_traj,
                            "recent_images": recent_imgs,
                        }
                        save_settings(current_settings)
                        # --- End Save Settings ---

                        # Create or update the agent
                        create_agent(
                            # Provider determined by get_provider_and_model unless custom model selected
                            provider=LLMProvider.OAICOMPAT if is_oaicompat else provider,
                            agent_loop=agent_loop_type,
                            # Pass the FINAL determined model name (cleaned or custom)
                            model_name=final_model_name_to_send,
                            api_key=api_key,
                            save_trajectory=save_traj,
                            only_n_most_recent_images=recent_imgs,
                            use_oaicompat=is_oaicompat,  # Set flag if custom model was selected
                            # Pass custom URL only if custom model was selected
                            provider_base_url=custom_url_value if is_oaicompat else None,
                            verbosity=logging.DEBUG,  # Added verbosity here
                        )

                        if global_agent is None:
                            # Add initial empty assistant message
                            history.append(
                                gr.ChatMessage(
                                    role="assistant",
                                    content="Failed to create agent. Check API keys and configuration.",
                                )
                            )
                            yield history
                            return

                            # Add the screenshot handler to the agent's loop if available
                        if global_agent and hasattr(global_agent, "_loop"):
                            print("DEBUG - Adding screenshot handler to agent loop")

                            # Create the screenshot handler with references to UI components
                            screenshot_handler = GradioChatScreenshotHandler(history)

                            # Add the handler to the callback manager if it exists AND is not None
                            if (
                                hasattr(global_agent._loop, "callback_manager")
                                and global_agent._loop.callback_manager is not None
                            ):
                                global_agent._loop.callback_manager.add_handler(screenshot_handler)
                                print(
                                    f"DEBUG - Screenshot handler added to callback manager with history: {id(history)}"
                                )
                            else:
                                # Optional: Log a warning if the callback manager is missing/None for a specific loop
                                print(
                                    f"WARNING - Callback manager not found or is None for loop type: {type(global_agent._loop)}. Screenshot handler not added."
                                )

                        # Stream responses from the agent
                        async for result in global_agent.run(last_user_message):
                            print(f"DEBUG - Agent response ------- START")
                            from pprint import pprint
                            pprint(result)
                            print(f"DEBUG - Agent response ------- END")
                            
                            def generate_gradio_messages():
                                if result.get("content"):
                                    yield gr.ChatMessage(
                                        role="assistant",
                                        content=result.get("content", ""),
                                        metadata=cast(MetadataDict, result.get("metadata", {}))
                                    )
                                else:
                                    outputs = result.get("output", [])
                                    for output in outputs:
                                        if output.get("type") == "message":
                                            content = output.get("content", [])
                                            for content_part in content:
                                                if content_part.get("text"):
                                                    yield gr.ChatMessage(
                                                        role=output.get("role", "assistant"),
                                                        content=content_part.get("text", ""),
                                                        metadata=content_part.get("metadata", {})
                                                    )
                                        elif output.get("type") == "reasoning":
                                            # if it's openAI, we only have access to a summary of the reasoning
                                            summary_content = output.get("summary", [])
                                            if summary_content:
                                                for summary_part in summary_content:
                                                    if summary_part.get("type") == "summary_text":
                                                        yield gr.ChatMessage(
                                                            role="assistant",
                                                            content=summary_part.get("text", "")
                                                        )
                                            else:
                                                summary_content = output.get("text", "")
                                                if summary_content:
                                                    yield gr.ChatMessage(
                                                        role="assistant",
                                                        content=summary_content,
                                                    )
                                        elif output.get("type") == "computer_call":
                                            action = output.get("action", {})
                                            action_type = action.get("type", "")
                                            if action_type:
                                                action_title = f"🛠️ Performing {action_type}"
                                                if action.get("x") and action.get("y"):
                                                    action_title += f" at ({action['x']}, {action['y']})"
                                                yield gr.ChatMessage(
                                                    role="assistant",
                                                    content=f"```json\n{json.dumps(action)}\n```",
                                                    metadata={"title": action_title}
                                                )
                            
                            for message in generate_gradio_messages():
                                history.append(message)
                                yield history
                            
                    except Exception as e:
                        import traceback

                        traceback.print_exc()
                        # Update with error message
                        history.append(gr.ChatMessage(role="assistant", content=f"Error: {str(e)}"))
                        yield history

                # Connect the components
                msg.submit(chat_submit, [msg, chatbot_history], [msg, chatbot_history]).then(
                    process_response,
                    [
                        chatbot_history,
                        model_choice,
                        custom_model,
                        agent_loop,
                        save_trajectory,
                        recent_images,
                        provider_base_url,
                        provider_api_key,
                    ],
                    [chatbot_history],
                )

                # Clear button functionality
                clear.click(lambda: None, None, chatbot_history, queue=False)

                # Connect agent_loop changes to model selection
                agent_loop.change(
                    fn=update_model_choices,
                    inputs=[agent_loop],
                    outputs=[model_choice],
                    queue=False,  # Process immediately without queueing
                )

                # Show/hide custom model, provider base URL, and API key textboxes based on dropdown selection
                def update_custom_model_visibility(model_value):
                    is_custom = model_value == "Custom model..."
                    return (
                        gr.update(visible=is_custom),
                        gr.update(visible=is_custom),
                        gr.update(visible=is_custom),
                    )

                model_choice.change(
                    fn=update_custom_model_visibility,
                    inputs=[model_choice],
                    outputs=[custom_model, provider_base_url, provider_api_key],
                    queue=False,  # Process immediately without queueing
                )

    return demo


def test_cua():
    """Standalone function to launch the Gradio app."""
    demo = create_gradio_ui()
    demo.launch(share=False)  # Don't create a public link


if __name__ == "__main__":
    test_cua()
