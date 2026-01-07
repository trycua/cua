"""Agent management commands for cua-bench CLI."""

import json
import os
import re
from pathlib import Path

# ANSI color codes
RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

# Default configuration for image push
DEFAULT_API_URL = "https://api.cua.ai"
CONFIG_DIR = Path.home() / ".config" / "cua-bench"
TOKEN_FILE = CONFIG_DIR / "token.json"


def register_parser(subparsers):
    """Register the agent subcommand and its subcommands."""
    agent_parser = subparsers.add_parser(
        "agent",
        help="Agent management commands",
        description="Create and manage custom agents for cua-bench.",
    )

    agent_subparsers = agent_parser.add_subparsers(
        dest="agent_command",
        title="commands",
        description="Available agent commands",
    )

    # cb agent init <name>
    init_parser = agent_subparsers.add_parser(
        "init",
        help="Create a new agent from template",
        description="Scaffold a new custom agent with working sample code.",
    )
    init_parser.add_argument(
        "name",
        help="Agent name (e.g., 'my-agent')",
    )
    init_parser.add_argument(
        "--output-dir",
        "-o",
        help="Output directory (default: ./<name>)",
    )

    # cb agent build <path>
    build_parser = agent_subparsers.add_parser(
        "build",
        help="Build agent Docker image",
        description="Build a Docker image for an agent directory containing a Dockerfile.",
    )
    build_parser.add_argument(
        "path",
        help="Path to agent directory (containing Dockerfile)",
    )
    build_parser.add_argument(
        "--tag",
        "-t",
        help="Image tag (default: agent directory name)",
    )

    # cb agent push <image-name>
    push_parser = agent_subparsers.add_parser(
        "push",
        help="Push agent Docker image to CUA registry",
        description="Push a Docker image through the CUA API proxy with workspace namespacing.",
    )
    push_parser.add_argument(
        "image_name",
        nargs="?",
        default="cua-bench:latest",
        help="Docker image to push (default: cua-bench:latest)",
    )
    push_parser.add_argument(
        "--registry",
        help=f"Custom API proxy URL (default: {DEFAULT_API_URL})",
    )


def execute(args):
    """Execute agent subcommands."""
    if not hasattr(args, "agent_command") or args.agent_command is None:
        print("Usage: cb agent <command>")
        print("Commands: init, build, push")
        print("Run 'cb agent <command> --help' for more information.")
        return 1

    if args.agent_command == "init":
        return create_agent_scaffold(args.name, args.output_dir)
    elif args.agent_command == "build":
        return build_agent(args.path, getattr(args, "tag", None))
    elif args.agent_command == "push":
        return push_image(args.image_name, getattr(args, "registry", None))

    print(f"Unknown agent command: {args.agent_command}")
    return 1


def load_token() -> dict | None:
    """Load the authentication token from the config file."""
    if not TOKEN_FILE.exists():
        return None
    try:
        with open(TOKEN_FILE, "r") as f:
            return json.load(f)
    except (json.JSONDecodeError, IOError):
        return None


def build_agent(agent_path: str, tag: str | None = None) -> int:
    """Build Docker image for an agent.

    Args:
        agent_path: Path to agent directory (containing Dockerfile)
        tag: Optional image tag (default: agent name from directory)

    Returns:
        Exit code (0 for success, 1 for error)
    """
    import subprocess

    agent_dir = Path(agent_path).resolve()
    dockerfile = agent_dir / "Dockerfile"

    if not agent_dir.exists():
        print(f"{RED}Error: Directory not found: {agent_dir}{RESET}")
        return 1

    if not dockerfile.exists():
        print(f"{RED}Error: No Dockerfile found in {agent_dir}{RESET}")
        print(f"{GREY}Run 'cb agent init <name>' to create a new agent with Dockerfile{RESET}")
        return 1

    # Determine tag from directory name if not specified
    image_tag = tag or agent_dir.name

    print(f"{CYAN}Building agent image...{RESET}")
    print(f"  {GREY}Source:{RESET} {agent_dir}")
    print(f"  {GREY}Tag:{RESET} {image_tag}")

    # Build image
    result = subprocess.run(
        ["docker", "build", "-t", image_tag, "."],
        cwd=agent_dir,
    )

    if result.returncode == 0:
        print(f"\n{GREEN}✓ Successfully built {image_tag}{RESET}")
        print(f"\n{GREY}To run locally:{RESET}")
        print(f"  cb run <task> --agent-import-path {agent_dir}")
        print(f"\n{GREY}To push to cloud:{RESET}")
        print(f"  cb agent push {image_tag}")
    else:
        print(f"\n{RED}✗ Build failed{RESET}")

    return result.returncode


def push_image(image_name: str, registry_url: str | None = None) -> int:
    """Push a Docker image to the CUA registry via API proxy.

    Args:
        image_name: Docker image name (e.g., 'cua-bench:latest')
        registry_url: Custom API proxy URL (default: https://api.cua.ai)

    Returns:
        Exit code (0 for success, 1 for error)
    """
    # Check for authentication
    token_data = load_token()
    if not token_data or not token_data.get("token"):
        print(f"{RED}Error: Not authenticated{RESET}")
        print(f"{GREY}Run {RESET}{CYAN}cb login{RESET}{GREY} first to authenticate.{RESET}")
        return 1

    token = token_data["token"]

    # Get API URL
    api_url = registry_url or os.getenv("CUA_API_URL", DEFAULT_API_URL)
    api_host = api_url.replace("https://", "").replace("http://", "")

    try:
        import docker
    except ImportError:
        print(f"{RED}Error: docker package not installed{RESET}")
        print(f"{GREY}Install with: pip install docker{RESET}")
        return 1

    try:
        client = docker.from_env()
    except docker.errors.DockerException as e:
        print(f"{RED}Error: Could not connect to Docker daemon{RESET}")
        print(f"{GREY}{e}{RESET}")
        return 1

    # Check if the image exists locally
    try:
        image = client.images.get(image_name)
    except docker.errors.ImageNotFound:
        print(f"{RED}Error: Image '{image_name}' not found locally{RESET}")
        print(f"{GREY}Build the image first with: docker build -t {image_name} .{RESET}")
        return 1

    # Extract the image name and tag
    parts = image_name.split("/")
    name_with_tag = parts[-1]
    if ":" in name_with_tag:
        base_name, tag = name_with_tag.rsplit(":", 1)
    else:
        base_name = name_with_tag
        tag = "latest"

    # Create the remote tag
    remote_tag = f"{api_host}/{base_name}:{tag}"

    print(f"{CYAN}Tagging image...{RESET}")
    print(f"  {GREY}Source:{RESET} {image_name}")
    print(f"  {GREY}Target:{RESET} {remote_tag}")

    try:
        image.tag(remote_tag)
    except docker.errors.APIError as e:
        print(f"{RED}Error tagging image: {e}{RESET}")
        return 1

    print(f"\n{CYAN}Pushing through API proxy...{RESET}")
    print(f"  {GREY}Endpoint:{RESET} {api_url}")

    # Create auth config for the push
    auth_config = {
        "username": "_token",
        "password": token,
    }

    try:
        for line in client.images.push(
            remote_tag, auth_config=auth_config, stream=True, decode=True
        ):
            if "status" in line:
                status = line["status"]
                progress = line.get("progress", "")
                layer_id = line.get("id", "")

                if layer_id:
                    print(f"  {GREY}{layer_id}:{RESET} {status} {progress}", end="\r")
                else:
                    print(f"  {status} {progress}")

            if "error" in line:
                print(f"\n{RED}Error: {line['error']}{RESET}")
                return 1

        print(f"\n{GREEN}✓ Successfully pushed {remote_tag}{RESET}")
        return 0

    except docker.errors.APIError as e:
        print(f"\n{RED}Error pushing image: {e}{RESET}")
        return 1


def create_agent_scaffold(name: str, output_dir: str | None = None) -> int:
    """Create a new agent from template.

    Creates a complete agent package with:
    - agent.py: Agent implementation (BaseAgent subclass)
    - main.py: Container entry point
    - Dockerfile: For building agent container
    - requirements.txt: Python dependencies
    - .dockerignore: Docker build ignores

    Args:
        name: Agent name (e.g., 'my-agent')
        output_dir: Output directory (default: ./<name>)

    Returns:
        Exit code (0 for success, 1 for error)
    """
    # Validate agent name
    if not re.match(r"^[a-zA-Z][a-zA-Z0-9_-]*$", name):
        print(f"{RED}Error: Invalid agent name '{name}'{RESET}")
        print("Agent name must start with a letter and contain only letters, numbers, underscores, and hyphens.")
        return 1

    # Determine output directory
    if output_dir:
        agent_dir = Path(output_dir)
    else:
        agent_dir = Path.cwd() / name

    # Check if directory already exists
    if agent_dir.exists():
        print(f"{RED}Error: Directory already exists: {agent_dir}{RESET}")
        return 1

    # Create class name from agent name
    # my-agent -> MyAgent, my_agent -> MyAgent
    class_name = "".join(word.capitalize() for word in re.split(r"[-_]", name))

    # Get templates directory
    templates_dir = Path(__file__).parent.parent / "templates" / "agent"

    if not templates_dir.exists():
        print(f"{RED}Error: Templates directory not found: {templates_dir}{RESET}")
        return 1

    # Create agent directory
    agent_dir.mkdir(parents=True, exist_ok=True)

    # Template variables
    variables = {
        "{{agent_name}}": name,
        "{{class_name}}": class_name,
    }

    # Copy and process templates
    template_files = [
        ("__init__.py.template", "__init__.py"),
        ("agent.py.template", "agent.py"),
        ("main.py.template", "main.py"),
        ("requirements.txt.template", "requirements.txt"),
        ("Dockerfile.template", "Dockerfile"),
        (".dockerignore.template", ".dockerignore"),
    ]

    for template_name, output_name in template_files:
        template_path = templates_dir / template_name
        output_path = agent_dir / output_name

        if not template_path.exists():
            print(f"{YELLOW}Warning: Template not found: {template_path}{RESET}")
            continue

        # Read and process template
        content = template_path.read_text()
        for placeholder, value in variables.items():
            content = content.replace(placeholder, value)

        # Write output file
        output_path.write_text(content)

    print(f"{GREEN}✓ Created agent '{name}' at {agent_dir}{RESET}")
    print()
    print(f"{BOLD}Next steps:{RESET}")
    print(f"  1. {CYAN}cd {agent_dir}{RESET}")
    print(f"  2. Edit {BOLD}agent.py{RESET} to implement your agent logic")
    print(f"  3. Add dependencies to {BOLD}requirements.txt{RESET}")
    print()
    print(f"{BOLD}Build and run locally:{RESET}")
    print(f"  {CYAN}cb agent build ./{name}{RESET}")
    print(f"  {CYAN}cb run <task> --agent-import-path ./{name}{RESET}")
    print()
    print(f"{BOLD}Push to CUA Cloud:{RESET}")
    print(f"  {CYAN}cb agent push {name}:latest{RESET}")

    return 0
