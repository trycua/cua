"""
CLI chat interface for agent - Computer Use Agent

Usage:
    python -m agent.cli <model_string>
    
Examples:
    python -m agent.cli openai/computer-use-preview
    python -m agent.cli anthropic/claude-3-5-sonnet-20241022
    python -m agent.cli omniparser+anthropic/claude-3-5-sonnet-20241022
"""

try:
    import asyncio
    import argparse
    import os
    import sys
    import json
    from typing import List, Dict, Any
    import dotenv
    from yaspin import yaspin
except ImportError:
    if __name__ == "__main__":
        raise ImportError(
            "CLI dependencies not found. "
            "Please install with: pip install \"cua-agent[cli]\""
        )

# Load environment variables
dotenv.load_dotenv()

# Color codes for terminal output
class Colors:
    RESET = '\033[0m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    
    # Text colors
    RED = '\033[31m'
    GREEN = '\033[32m'
    YELLOW = '\033[33m'
    BLUE = '\033[34m'
    MAGENTA = '\033[35m'
    CYAN = '\033[36m'
    WHITE = '\033[37m'
    GRAY = '\033[90m'
    
    # Background colors
    BG_RED = '\033[41m'
    BG_GREEN = '\033[42m'
    BG_YELLOW = '\033[43m'
    BG_BLUE = '\033[44m'

def print_colored(text: str, color: str = "", bold: bool = False, dim: bool = False, end: str = "\n", right: str = ""):
    """Print colored text to terminal with optional right-aligned text."""
    prefix = ""
    if bold:
        prefix += Colors.BOLD
    if dim:
        prefix += Colors.DIM
    if color:
        prefix += color
    
    if right:
        # Get terminal width (default to 80 if unable to determine)
        try:
            import shutil
            terminal_width = shutil.get_terminal_size().columns
        except:
            terminal_width = 80

        # Add right margin
        terminal_width -= 1
        
        # Calculate padding needed
        # Account for ANSI escape codes not taking visual space
        visible_left_len = len(text)
        visible_right_len = len(right)
        padding = terminal_width - visible_left_len - visible_right_len
        
        if padding > 0:
            output = f"{prefix}{text}{' ' * padding}{right}{Colors.RESET}"
        else:
            # If not enough space, just put a single space between
            output = f"{prefix}{text} {right}{Colors.RESET}"
    else:
        output = f"{prefix}{text}{Colors.RESET}"
    
    print(output, end=end)


def print_action(action_type: str, details: Dict[str, Any], total_cost: float):
    """Print computer action with nice formatting."""
    # Format action details
    args_str = ""
    if action_type == "click" and "x" in details and "y" in details:
        args_str = f"_{details.get('button', 'left')}({details['x']}, {details['y']})"
    elif action_type == "type" and "text" in details:
        text = details["text"]
        if len(text) > 50:
            text = text[:47] + "..."
        args_str = f'("{text}")'
    elif action_type == "key" and "text" in details:
        args_str = f"('{details['text']}')"
    elif action_type == "scroll" and "x" in details and "y" in details:
        args_str = f"({details['x']}, {details['y']})"
    
    if total_cost > 0:
        print_colored(f"🛠️  {action_type}{args_str}", dim=True, right=f"💸 ${total_cost:.2f}")
    else:
        print_colored(f"🛠️  {action_type}{args_str}", dim=True)

def print_welcome(model: str, agent_loop: str, container_name: str):
    """Print welcome message."""
    print_colored(f"Connected to {container_name} ({model}, {agent_loop})")
    print_colored("Type 'exit' to quit.", dim=True)

async def ainput(prompt: str = ""):
    return await asyncio.to_thread(input, prompt)

async def chat_loop(agent, model: str, container_name: str, initial_prompt: str = "", show_usage: bool = True):
    """Main chat loop with the agent."""
    print_welcome(model, agent.agent_config_info.agent_class.__name__, container_name)
    
    history = []
    
    if initial_prompt:
        history.append({"role": "user", "content": initial_prompt})
    
    total_cost = 0

    while True:
        if len(history) == 0 or history[-1].get("role") != "user":
            # Get user input with prompt
            print_colored("> ", end="")
            user_input = await ainput()
            
            if user_input.lower() in ['exit', 'quit', 'q']:
                print_colored("\n👋 Goodbye!")
                break
                
            if not user_input:
                continue
                
            # Add user message to history
            history.append({"role": "user", "content": user_input})
        
        # Stream responses from the agent with spinner
        with yaspin(text="Thinking...", spinner="line", attrs=["dark"]) as spinner:
            spinner.hide()
            
            async for result in agent.run(history):
                # Add agent responses to history
                history.extend(result.get("output", []))

                if show_usage:
                    total_cost += result.get("usage", {}).get("response_cost", 0)
                
                # Process and display the output
                for item in result.get("output", []):
                    if item.get("type") == "message":
                        # Display agent text response
                        content = item.get("content", [])
                        for content_part in content:
                            if content_part.get("text"):
                                text = content_part.get("text", "").strip()
                                if text:
                                    spinner.hide()
                                    print_colored(text)
                    
                    elif item.get("type") == "computer_call":
                        # Display computer action
                        action = item.get("action", {})
                        action_type = action.get("type", "")
                        if action_type:
                            spinner.hide()
                            print_action(action_type, action, total_cost)
                            spinner.text = f"Performing {action_type}..."
                            spinner.show()
                    
                    elif item.get("type") == "function_call":
                        # Display function call
                        function_name = item.get("name", "")
                        spinner.hide()
                        print_colored(f"🔧 Calling function: {function_name}", dim=True)
                        spinner.text = f"Calling {function_name}..."
                        spinner.show()
                    
                    elif item.get("type") == "function_call_output":
                        # Display function output (dimmed)
                        output = item.get("output", "")
                        if output and len(output.strip()) > 0:
                            spinner.hide()
                            print_colored(f"📤 {output}", dim=True)
            
            spinner.hide()
            if show_usage and total_cost > 0:
                print_colored(f"Total cost: ${total_cost:.2f}", dim=True)
        

async def main():
    """Main CLI function."""
    parser = argparse.ArgumentParser(
        description="CUA Agent CLI - Interactive computer use assistant",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python -m agent.cli openai/computer-use-preview
  python -m agent.cli anthropic/claude-3-5-sonnet-20241022
  python -m agent.cli omniparser+anthropic/claude-3-5-sonnet-20241022
  python -m agent.cli huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B
        """
    )
    
    parser.add_argument(
        "model",
        help="Model string (e.g., 'openai/computer-use-preview', 'anthropic/claude-3-5-sonnet-20241022')"
    )
    
    parser.add_argument(
        "--images",
        type=int,
        default=3,
        help="Number of recent images to keep in context (default: 3)"
    )
    
    parser.add_argument(
        "--trajectory",
        action="store_true",
        help="Save trajectory for debugging"
    )
    
    parser.add_argument(
        "--budget",
        type=float,
        help="Maximum budget for the session (in dollars)"
    )
    
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Enable verbose logging"
    )

    parser.add_argument(
        "-p", "--prompt",
        type=str,
        help="Initial prompt to send to the agent. Leave blank for interactive mode."
    )

    parser.add_argument(
        "-c", "--cache",
        action="store_true",
        help="Tell the API to enable caching"
    )

    parser.add_argument(
        "-u", "--usage",
        action="store_true",
        help="Show total cost of the agent runs"
    )

    parser.add_argument(
        "-r", "--max-retries",
        type=int,
        default=3,
        help="Maximum number of retries for the LLM API calls"
    )
    
    args = parser.parse_args()
    
    # Check for required environment variables
    container_name = os.getenv("CUA_CONTAINER_NAME")
    cua_api_key = os.getenv("CUA_API_KEY")
    
    # Prompt for missing environment variables
    if not container_name:
        print_colored("CUA_CONTAINER_NAME not set.", dim=True)
        print_colored("You can get a CUA container at https://www.trycua.com/", dim=True)
        container_name = input("Enter your CUA container name: ").strip()
        if not container_name:
            print_colored("❌ Container name is required.")
            sys.exit(1)
    
    if not cua_api_key:
        print_colored("CUA_API_KEY not set.", dim=True)
        cua_api_key = input("Enter your CUA API key: ").strip()
        if not cua_api_key:
            print_colored("❌ API key is required.")
            sys.exit(1)
    
    # Check for provider-specific API keys based on model
    provider_api_keys = {
        "openai/": "OPENAI_API_KEY",
        "anthropic/": "ANTHROPIC_API_KEY",
        "omniparser+": "OPENAI_API_KEY",
        "omniparser+": "ANTHROPIC_API_KEY",
    }
    
    # Find matching provider and check for API key
    for prefix, env_var in provider_api_keys.items():
        if args.model.startswith(prefix):
            if not os.getenv(env_var):
                print_colored(f"{env_var} not set.", dim=True)
                api_key = input(f"Enter your {env_var.replace('_', ' ').title()}: ").strip()
                if not api_key:
                    print_colored(f"❌ {env_var.replace('_', ' ').title()} is required.")
                    sys.exit(1)
                # Set the environment variable for the session
                os.environ[env_var] = api_key
            break
    
    # Import here to avoid import errors if dependencies are missing
    try:
        from agent import ComputerAgent
        from computer import Computer
    except ImportError as e:
        print_colored(f"❌ Import error: {e}", Colors.RED, bold=True)
        print_colored("Make sure agent and computer libraries are installed.", Colors.YELLOW)
        sys.exit(1)
    
    # Create computer instance
    async with Computer(
        os_type="linux",
        provider_type="cloud",
        name=container_name,
        api_key=cua_api_key
    ) as computer:
        
        # Create agent
        agent_kwargs = {
            "model": args.model,
            "tools": [computer],
            "trust_remote_code": True, # needed for some local models (e.g., InternVL, OpenCUA)
            "verbosity": 20 if args.verbose else 30,  # DEBUG vs WARNING
            "max_retries": args.max_retries
        }

        if args.images > 0:
            agent_kwargs["only_n_most_recent_images"] = args.images
        
        if args.trajectory:
            agent_kwargs["trajectory_dir"] = "trajectories"
        
        if args.budget:
            agent_kwargs["max_trajectory_budget"] = {
                "max_budget": args.budget,
                "raise_error": True,
                "reset_after_each_run": False
            }

        if args.cache:
            agent_kwargs["use_prompt_caching"] = True
        
        agent = ComputerAgent(**agent_kwargs)
        
        # Start chat loop
        await chat_loop(agent, args.model, container_name, args.prompt, args.usage)



if __name__ == "__main__":
    try:
        asyncio.run(main())
    except (KeyboardInterrupt, EOFError) as _:
        print_colored("\n\n👋 Goodbye!")