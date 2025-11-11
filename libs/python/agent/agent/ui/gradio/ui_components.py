"""
UI Components for the Gradio interface
"""

import asyncio
import json
import logging
import os
import platform
from pathlib import Path
from typing import Any, Dict, List, Optional, cast

import gradio as gr
from gradio.components.chatbot import MetadataDict

from .app import (
    create_agent,
    get_model_string,
    get_ollama_models,
    global_agent,
    global_computer,
    load_settings,
    save_settings,
)

# Global messages array to maintain conversation history
global_messages = []


def create_gradio_ui() -> gr.Blocks:
    """Create a Gradio UI for the Computer-Use Agent."""

    # Load settings
    saved_settings = load_settings()

    # Check for API keys
    openai_api_key = os.environ.get("OPENAI_API_KEY", "")
    anthropic_api_key = os.environ.get("ANTHROPIC_API_KEY", "")
    cua_api_key = os.environ.get("CUA_API_KEY", "")

    # Model choices
    openai_models = ["OpenAI: Computer-Use Preview"]
    anthropic_models = [
        "Anthropic: Claude 4 Opus (20250514)",
        "Anthropic: Claude 4 Sonnet (20250514)",
        "Anthropic: Claude 3.7 Sonnet (20250219)",
        "Anthropic: Claude 3.5 Sonnet (20241022)",
    ]
    omni_models = [
        "OMNI: OpenAI GPT-4o",
        "OMNI: OpenAI GPT-4o mini",
        "OMNI: Claude 3.7 Sonnet (20250219)",
        "OMNI: Claude 3.5 Sonnet (20241022)",
    ]

    # Check if API keys are available
    has_openai_key = bool(openai_api_key)
    has_anthropic_key = bool(anthropic_api_key)
    has_cua_key = bool(cua_api_key)

    # Get Ollama models for OMNI
    ollama_models = get_ollama_models()
    if ollama_models:
        omni_models += ollama_models

    # Detect platform
    is_mac = platform.system().lower() == "darwin"

    # Format model choices
    provider_to_models = {
        "OPENAI": openai_models,
        "ANTHROPIC": anthropic_models,
        "OMNI": omni_models + ["Custom model (OpenAI compatible API)", "Custom model (ollama)"],
        "UITARS": (
            [
                "huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B",
            ]
            if is_mac
            else []
        )
        + ["Custom model (OpenAI compatible API)"],
    }

    # Apply saved settings
    initial_loop = saved_settings.get("agent_loop", "OMNI")
    available_models_for_loop = provider_to_models.get(initial_loop, [])
    saved_model_choice = saved_settings.get("model_choice")
    if saved_model_choice and saved_model_choice in available_models_for_loop:
        initial_model = saved_model_choice
    else:
        if initial_loop == "OPENAI":
            initial_model = openai_models[0] if openai_models else "No models available"
        elif initial_loop == "ANTHROPIC":
            initial_model = anthropic_models[0] if anthropic_models else "No models available"
        else:  # OMNI
            initial_model = (
                omni_models[0] if omni_models else "Custom model (OpenAI compatible API)"
            )

    initial_custom_model = saved_settings.get("custom_model", "Qwen2.5-VL-7B-Instruct")
    initial_provider_base_url = saved_settings.get("provider_base_url", "http://localhost:1234/v1")
    initial_save_trajectory = saved_settings.get("save_trajectory", True)
    initial_recent_images = saved_settings.get("recent_images", 3)

    # Example prompts
    example_messages = [
        "Create a Python virtual environment, install pandas and matplotlib, then plot stock data",
        "Open a PDF in Preview, add annotations, and save it as a compressed version",
        "Open Safari, search for 'macOS automation tools', and save the first three results as bookmarks",
        "Configure SSH keys and set up a connection to a remote server",
    ]

    def generate_python_code(
        agent_loop_choice,
        model_name,
        tasks,
        recent_images=3,
        save_trajectory=True,
        computer_os="linux",
        computer_provider="cloud",
        container_name="",
        cua_cloud_api_key="",
        max_budget=None,
    ):
        """Generate Python code for the current configuration and tasks."""
        tasks_str = ""
        for task in tasks:
            if task and task.strip():
                tasks_str += f'            "{task}",\n'

        model_string = get_model_string(model_name, agent_loop_choice)

        computer_args = []
        if computer_os != "macos":
            computer_args.append(f'os_type="{computer_os}"')
        if computer_provider != "lume":
            computer_args.append(f'provider_type="{computer_provider}"')
        if container_name:
            computer_args.append(f'name="{container_name}"')
        if cua_cloud_api_key:
            computer_args.append(f'api_key="{cua_cloud_api_key}"')

        computer_args_str = ", ".join(computer_args)
        if computer_args_str:
            computer_args_str = f"({computer_args_str})"
        else:
            computer_args_str = "()"

        code = f"""import asyncio
from computer import Computer
from agent import ComputerAgent

async def main():
    async with Computer{computer_args_str} as computer:
        agent = ComputerAgent(
            model="{model_string}",
            tools=[computer],
            only_n_most_recent_images={recent_images},"""

        if save_trajectory:
            code += """
            trajectory_dir="trajectories","""

        if max_budget:
            code += f"""
            max_trajectory_budget={{"max_budget": {max_budget}, "raise_error": True}},"""

        code += """
        )
        """

        if tasks_str:
            code += f"""
        # Prompts for the computer-use agent
        tasks = [
{tasks_str.rstrip()}
        ]

        for task in tasks:
            print(f"Executing task: {{task}}")
            messages = [{{"role": "user", "content": task}}]
            async for result in agent.run(messages):
                for item in result["output"]:
                    if item["type"] == "message":
                        print(item["content"][0]["text"])"""
        else:
            code += """
        # Execute a single task
        task = "Search for information about CUA on GitHub"
        print(f"Executing task: {task}")
        messages = [{"role": "user", "content": task}]
        async for result in agent.run(messages):
            for item in result["output"]:
                if item["type"] == "message":
                    print(item["content"][0]["text"])"""

        code += """

if __name__ == "__main__":
    asyncio.run(main())"""

        return code

    # Create the Gradio interface
    with gr.Blocks(title="Computer-Use Agent") as demo:
        with gr.Row():
            # Left column for settings
            with gr.Column(scale=1):
                # Logo
                gr.HTML(
                    """
                    <div style="display: flex; justify-content: center; margin-bottom: 0.5em">
                        <img alt="CUA Logo" style="width: 80px;"
                             src="https://github.com/trycua/cua/blob/main/img/logo_white.png?raw=true" />
                    </div>
                    """
                )

                # Python code accordion
                with gr.Accordion("Python Code", open=False):
                    code_display = gr.Code(
                        language="python",
                        value=generate_python_code(initial_loop, "gpt-4o", []),
                        interactive=False,
                    )

                with gr.Accordion("Computer Configuration", open=True):
                    is_windows = platform.system().lower() == "windows"
                    is_mac = platform.system().lower() == "darwin"

                    providers = ["cloud", "localhost", "docker"]
                    if is_mac:
                        providers += ["lume"]
                    if is_windows:
                        providers += ["winsandbox"]

                    # Remove unavailable options
                    # MacOS is unavailable if Lume is not available
                    # Windows is unavailable if Winsandbox is not available
                    # Linux is always available
                    # This should be removed once we support macOS and Windows on the cloud provider
                    computer_choices = ["macos", "linux", "windows"]
                    if not is_mac or "lume" not in providers:
                        computer_choices.remove("macos")
                    if not is_windows or "winsandbox" not in providers:
                        computer_choices.remove("windows")

                    computer_os = gr.Radio(
                        choices=computer_choices,
                        label="Operating System",
                        value=computer_choices[0],
                        info="Select the operating system for the computer",
                    )

                    computer_provider = gr.Radio(
                        choices=providers,
                        label="Provider",
                        value="lume" if is_mac else "cloud",
                        info="Select the computer provider",
                    )

                    container_name = gr.Textbox(
                        label="Container Name",
                        placeholder="Enter container name (optional)",
                        value=os.environ.get("CUA_CONTAINER_NAME", ""),
                        info="Optional name for the container",
                    )

                    cua_cloud_api_key = gr.Textbox(
                        label="CUA Cloud API Key",
                        placeholder="Enter your CUA Cloud API key",
                        value=os.environ.get("CUA_API_KEY", ""),
                        type="password",
                        info="Required for cloud provider",
                        visible=(not has_cua_key),
                    )

                with gr.Accordion("Agent Configuration", open=True):
                    agent_loop = gr.Dropdown(
                        choices=["OPENAI", "ANTHROPIC", "OMNI", "UITARS"],
                        label="Agent Loop",
                        value=initial_loop,
                        info="Select the agent loop provider",
                    )

                    # Model selection dropdowns
                    with gr.Group() as model_selection_group:
                        openai_model_choice = gr.Dropdown(
                            choices=openai_models,
                            label="OpenAI Model",
                            value=openai_models[0] if openai_models else "No models available",
                            info="Select OpenAI model",
                            interactive=True,
                            visible=(initial_loop == "OPENAI"),
                        )

                        anthropic_model_choice = gr.Dropdown(
                            choices=anthropic_models,
                            label="Anthropic Model",
                            value=(
                                anthropic_models[0] if anthropic_models else "No models available"
                            ),
                            info="Select Anthropic model",
                            interactive=True,
                            visible=(initial_loop == "ANTHROPIC"),
                        )

                        omni_model_choice = gr.Dropdown(
                            choices=omni_models
                            + ["Custom model (OpenAI compatible API)", "Custom model (ollama)"],
                            label="OMNI Model",
                            value=(
                                omni_models[0]
                                if omni_models
                                else "Custom model (OpenAI compatible API)"
                            ),
                            info="Select OMNI model or choose a custom model option",
                            interactive=True,
                            visible=(initial_loop == "OMNI"),
                        )

                        uitars_model_choice = gr.Dropdown(
                            choices=provider_to_models.get("UITARS", ["No models available"]),
                            label="UITARS Model",
                            value=(
                                provider_to_models.get("UITARS", ["No models available"])[0]
                                if provider_to_models.get("UITARS")
                                else "No models available"
                            ),
                            info="Select UITARS model",
                            interactive=True,
                            visible=(initial_loop == "UITARS"),
                        )

                        model_choice = gr.Textbox(visible=False)

                    # API key inputs
                    with gr.Group(
                        visible=not has_openai_key
                        and (initial_loop == "OPENAI" or initial_loop == "OMNI")
                    ) as openai_key_group:
                        openai_api_key_input = gr.Textbox(
                            label="OpenAI API Key",
                            placeholder="Enter your OpenAI API key",
                            value=os.environ.get("OPENAI_API_KEY", ""),
                            interactive=True,
                            type="password",
                            info="Required for OpenAI models",
                        )

                    with gr.Group(
                        visible=not has_anthropic_key
                        and (initial_loop == "ANTHROPIC" or initial_loop == "OMNI")
                    ) as anthropic_key_group:
                        anthropic_api_key_input = gr.Textbox(
                            label="Anthropic API Key",
                            placeholder="Enter your Anthropic API key",
                            value=os.environ.get("ANTHROPIC_API_KEY", ""),
                            interactive=True,
                            type="password",
                            info="Required for Anthropic models",
                        )

                    # API key handlers
                    def set_openai_api_key(key):
                        if key and key.strip():
                            os.environ["OPENAI_API_KEY"] = key.strip()
                            print("DEBUG - Set OpenAI API key environment variable")
                        return key

                    def set_anthropic_api_key(key):
                        if key and key.strip():
                            os.environ["ANTHROPIC_API_KEY"] = key.strip()
                            print("DEBUG - Set Anthropic API key environment variable")
                        return key

                    openai_api_key_input.change(
                        fn=set_openai_api_key,
                        inputs=[openai_api_key_input],
                        outputs=[openai_api_key_input],
                        queue=False,
                    )

                    anthropic_api_key_input.change(
                        fn=set_anthropic_api_key,
                        inputs=[anthropic_api_key_input],
                        outputs=[anthropic_api_key_input],
                        queue=False,
                    )

                    # UI update function
                    def update_ui(
                        loop=None,
                        openai_model=None,
                        anthropic_model=None,
                        omni_model=None,
                        uitars_model=None,
                    ):
                        loop = loop or agent_loop.value

                        model_value = None
                        if loop == "OPENAI" and openai_model:
                            model_value = openai_model
                        elif loop == "ANTHROPIC" and anthropic_model:
                            model_value = anthropic_model
                        elif loop == "OMNI" and omni_model:
                            model_value = omni_model
                        elif loop == "UITARS" and uitars_model:
                            model_value = uitars_model

                        openai_visible = loop == "OPENAI"
                        anthropic_visible = loop == "ANTHROPIC"
                        omni_visible = loop == "OMNI"
                        uitars_visible = loop == "UITARS"

                        show_openai_key = not has_openai_key and (
                            loop == "OPENAI"
                            or (
                                loop == "OMNI"
                                and model_value
                                and "OpenAI" in model_value
                                and "Custom" not in model_value
                            )
                        )
                        show_anthropic_key = not has_anthropic_key and (
                            loop == "ANTHROPIC"
                            or (
                                loop == "OMNI"
                                and model_value
                                and "Claude" in model_value
                                and "Custom" not in model_value
                            )
                        )

                        is_custom_openai_api = model_value == "Custom model (OpenAI compatible API)"
                        is_custom_ollama = model_value == "Custom model (ollama)"
                        is_any_custom = is_custom_openai_api or is_custom_ollama

                        model_choice_value = model_value if model_value else ""

                        return [
                            gr.update(visible=openai_visible),
                            gr.update(visible=anthropic_visible),
                            gr.update(visible=omni_visible),
                            gr.update(visible=uitars_visible),
                            gr.update(visible=show_openai_key),
                            gr.update(visible=show_anthropic_key),
                            gr.update(visible=is_any_custom),
                            gr.update(visible=is_custom_openai_api),
                            gr.update(visible=is_custom_openai_api),
                            gr.update(value=model_choice_value),
                        ]

                    # Custom model inputs
                    custom_model = gr.Textbox(
                        label="Custom Model Name",
                        placeholder="Enter custom model name (e.g., Qwen2.5-VL-7B-Instruct or llama3)",
                        value=initial_custom_model,
                        visible=(
                            initial_model == "Custom model (OpenAI compatible API)"
                            or initial_model == "Custom model (ollama)"
                        ),
                        interactive=True,
                    )

                    provider_base_url = gr.Textbox(
                        label="Provider Base URL",
                        placeholder="Enter provider base URL (e.g., http://localhost:1234/v1)",
                        value=initial_provider_base_url,
                        visible=(initial_model == "Custom model (OpenAI compatible API)"),
                        interactive=True,
                    )

                    provider_api_key = gr.Textbox(
                        label="Provider API Key",
                        placeholder="Enter provider API key (if required)",
                        value="",
                        visible=(initial_model == "Custom model (OpenAI compatible API)"),
                        interactive=True,
                        type="password",
                    )

                    # Provider visibility update function
                    def update_provider_visibility(provider):
                        """Update visibility of container name and API key based on selected provider."""
                        is_localhost = provider == "localhost"
                        return [
                            gr.update(visible=not is_localhost),  # container_name
                            gr.update(
                                visible=not is_localhost and not has_cua_key
                            ),  # cua_cloud_api_key
                        ]

                    # Connect provider change event
                    computer_provider.change(
                        fn=update_provider_visibility,
                        inputs=[computer_provider],
                        outputs=[container_name, cua_cloud_api_key],
                        queue=False,
                    )

                    # Connect UI update events
                    for dropdown in [
                        agent_loop,
                        omni_model_choice,
                        uitars_model_choice,
                        openai_model_choice,
                        anthropic_model_choice,
                    ]:
                        dropdown.change(
                            fn=update_ui,
                            inputs=[
                                agent_loop,
                                openai_model_choice,
                                anthropic_model_choice,
                                omni_model_choice,
                                uitars_model_choice,
                            ],
                            outputs=[
                                openai_model_choice,
                                anthropic_model_choice,
                                omni_model_choice,
                                uitars_model_choice,
                                openai_key_group,
                                anthropic_key_group,
                                custom_model,
                                provider_base_url,
                                provider_api_key,
                                model_choice,
                            ],
                            queue=False,
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

                    max_budget = gr.Number(
                        label="Max Budget ($)",
                        value=lambda: None,
                        minimum=-1,
                        maximum=100.0,
                        step=0.1,
                        info="Optional budget limit for trajectory (0 = no limit)",
                        interactive=True,
                    )

            # Right column for chat interface
            with gr.Column(scale=2):
                gr.Markdown(
                    "Ask me to perform tasks in a virtual environment.<br>Built with <a href='https://github.com/trycua/cua' target='_blank'>github.com/trycua/cua</a>."
                )

                chatbot_history = gr.Chatbot(type="messages")
                msg = gr.Textbox(placeholder="Ask me to perform tasks in a virtual environment")
                clear = gr.Button("Clear")
                cancel_button = gr.Button("Cancel", variant="stop")

                # Add examples
                example_group = gr.Examples(examples=example_messages, inputs=msg)

                # Chat submission function
                def chat_submit(message, history):
                    history.append(gr.ChatMessage(role="user", content=message))
                    return "", history

                # Cancel function
                async def cancel_agent_task(history):
                    global global_agent
                    if global_agent:
                        print("DEBUG - Cancelling agent task")
                        history.append(
                            gr.ChatMessage(
                                role="assistant",
                                content="Task cancelled by user",
                                metadata={"title": "❌ Cancelled"},
                            )
                        )
                    else:
                        history.append(
                            gr.ChatMessage(
                                role="assistant",
                                content="No active agent task to cancel",
                                metadata={"title": "ℹ️ Info"},
                            )
                        )
                    return history

                # Process response function
                async def process_response(
                    history,
                    openai_model_value,
                    anthropic_model_value,
                    omni_model_value,
                    uitars_model_value,
                    custom_model_value,
                    agent_loop_choice,
                    save_traj,
                    recent_imgs,
                    custom_url_value=None,
                    custom_api_key=None,
                    openai_key_input=None,
                    anthropic_key_input=None,
                    computer_os="linux",
                    computer_provider="cloud",
                    container_name="",
                    cua_cloud_api_key="",
                    max_budget_value=None,
                ):
                    if not history:
                        yield history
                        return

                    # Get the last user message
                    last_user_message = history[-1]["content"]

                    # Get the appropriate model value based on the agent loop
                    if agent_loop_choice == "OPENAI":
                        model_choice_value = openai_model_value
                    elif agent_loop_choice == "ANTHROPIC":
                        model_choice_value = anthropic_model_value
                    elif agent_loop_choice == "OMNI":
                        model_choice_value = omni_model_value
                    elif agent_loop_choice == "UITARS":
                        model_choice_value = uitars_model_value
                    else:
                        model_choice_value = "No models available"

                    # Determine if this is a custom model selection
                    is_custom_model_selected = model_choice_value in [
                        "Custom model (OpenAI compatible API)",
                        "Custom model (ollama)",
                    ]

                    # Determine the model name string to analyze
                    if is_custom_model_selected:
                        model_string_to_analyze = custom_model_value
                    else:
                        model_string_to_analyze = model_choice_value

                    try:
                        # Get the model string
                        model_string = get_model_string(model_string_to_analyze, agent_loop_choice)

                        # Set API keys if provided
                        if openai_key_input:
                            os.environ["OPENAI_API_KEY"] = openai_key_input
                        if anthropic_key_input:
                            os.environ["ANTHROPIC_API_KEY"] = anthropic_key_input
                        if cua_cloud_api_key:
                            os.environ["CUA_API_KEY"] = cua_cloud_api_key

                        # Save settings
                        current_settings = {
                            "agent_loop": agent_loop_choice,
                            "model_choice": model_choice_value,
                            "custom_model": custom_model_value,
                            "provider_base_url": custom_url_value,
                            "save_trajectory": save_traj,
                            "recent_images": recent_imgs,
                            "computer_os": computer_os,
                            "computer_provider": computer_provider,
                            "container_name": container_name,
                        }
                        save_settings(current_settings)

                        # Create agent
                        global_agent = create_agent(
                            model_string=model_string,
                            save_trajectory=save_traj,
                            only_n_most_recent_images=recent_imgs,
                            custom_model_name=(
                                custom_model_value if is_custom_model_selected else None
                            ),
                            computer_os=computer_os,
                            computer_provider=computer_provider,
                            computer_name=container_name,
                            computer_api_key=cua_cloud_api_key,
                            verbosity=logging.DEBUG,
                            max_trajectory_budget=(
                                max_budget_value
                                if max_budget_value and max_budget_value > 0
                                else None
                            ),
                        )

                        if global_agent is None:
                            history.append(
                                gr.ChatMessage(
                                    role="assistant",
                                    content="Failed to create agent. Check API keys and configuration.",
                                )
                            )
                            yield history
                            return

                        # Add user message to global history
                        global global_messages
                        global_messages.append({"role": "user", "content": last_user_message})

                        # Stream responses from the agent
                        async for result in global_agent.run(global_messages):
                            global_messages += result.get("output", [])
                            # print(f"DEBUG - Agent response ------- START")
                            # from pprint import pprint
                            # pprint(result)
                            # print(f"DEBUG - Agent response ------- END")

                            # Process the result output
                            for item in result.get("output", []):
                                if item.get("type") == "message":
                                    content = item.get("content", [])
                                    for content_part in content:
                                        if content_part.get("text"):
                                            history.append(
                                                gr.ChatMessage(
                                                    role=item.get("role", "assistant"),
                                                    content=content_part.get("text", ""),
                                                    metadata=content_part.get("metadata", {}),
                                                )
                                            )
                                elif item.get("type") == "computer_call":
                                    action = item.get("action", {})
                                    action_type = action.get("type", "")
                                    if action_type:
                                        action_title = f"🛠️ Performing {action_type}"
                                        if action.get("x") and action.get("y"):
                                            action_title += f" at ({action['x']}, {action['y']})"
                                        history.append(
                                            gr.ChatMessage(
                                                role="assistant",
                                                content=f"```json\n{json.dumps(action)}\n```",
                                                metadata={"title": action_title},
                                            )
                                        )
                                elif item.get("type") == "function_call":
                                    function_name = item.get("name", "")
                                    arguments = item.get("arguments", "{}")
                                    history.append(
                                        gr.ChatMessage(
                                            role="assistant",
                                            content=f"🔧 Calling function: {function_name}\n```json\n{arguments}\n```",
                                            metadata={"title": f"Function Call: {function_name}"},
                                        )
                                    )
                                elif item.get("type") == "function_call_output":
                                    output = item.get("output", "")
                                    history.append(
                                        gr.ChatMessage(
                                            role="assistant",
                                            content=f"📤 Function output:\n```\n{output}\n```",
                                            metadata={"title": "Function Output"},
                                        )
                                    )
                                elif item.get("type") == "computer_call_output":
                                    output = item.get("output", {}).get("image_url", "")
                                    image_markdown = f"![Computer output]({output})"
                                    history.append(
                                        gr.ChatMessage(
                                            role="assistant",
                                            content=image_markdown,
                                            metadata={"title": "🖥️ Computer Output"},
                                        )
                                    )

                            yield history

                    except Exception as e:
                        import traceback

                        traceback.print_exc()
                        history.append(gr.ChatMessage(role="assistant", content=f"Error: {str(e)}"))
                        yield history

                # Connect the submit button
                submit_event = msg.submit(
                    fn=chat_submit,
                    inputs=[msg, chatbot_history],
                    outputs=[msg, chatbot_history],
                    queue=False,
                ).then(
                    fn=process_response,
                    inputs=[
                        chatbot_history,
                        openai_model_choice,
                        anthropic_model_choice,
                        omni_model_choice,
                        uitars_model_choice,
                        custom_model,
                        agent_loop,
                        save_trajectory,
                        recent_images,
                        provider_base_url,
                        provider_api_key,
                        openai_api_key_input,
                        anthropic_api_key_input,
                        computer_os,
                        computer_provider,
                        container_name,
                        cua_cloud_api_key,
                        max_budget,
                    ],
                    outputs=[chatbot_history],
                    queue=True,
                )

                # Clear button functionality
                def clear_chat():
                    global global_messages
                    global_messages.clear()
                    return None

                clear.click(clear_chat, None, chatbot_history, queue=False)

                # Connect cancel button
                cancel_button.click(
                    cancel_agent_task, [chatbot_history], [chatbot_history], queue=False
                )

                # Code display update function
                def update_code_display(
                    agent_loop,
                    model_choice_val,
                    custom_model_val,
                    chat_history,
                    recent_images_val,
                    save_trajectory_val,
                    computer_os,
                    computer_provider,
                    container_name,
                    cua_cloud_api_key,
                    max_budget_val,
                ):
                    messages = []
                    if chat_history:
                        for msg in chat_history:
                            if isinstance(msg, dict) and msg.get("role") == "user":
                                messages.append(msg.get("content", ""))

                    return generate_python_code(
                        agent_loop,
                        model_choice_val or custom_model_val or "gpt-4o",
                        messages,
                        recent_images_val,
                        save_trajectory_val,
                        computer_os,
                        computer_provider,
                        container_name,
                        cua_cloud_api_key,
                        max_budget_val,
                    )

                # Update code display when configuration changes
                for component in [
                    agent_loop,
                    model_choice,
                    custom_model,
                    chatbot_history,
                    recent_images,
                    save_trajectory,
                    computer_os,
                    computer_provider,
                    container_name,
                    cua_cloud_api_key,
                    max_budget,
                ]:
                    component.change(
                        update_code_display,
                        inputs=[
                            agent_loop,
                            model_choice,
                            custom_model,
                            chatbot_history,
                            recent_images,
                            save_trajectory,
                            computer_os,
                            computer_provider,
                            container_name,
                            cua_cloud_api_key,
                            max_budget,
                        ],
                        outputs=[code_display],
                    )

    return demo
