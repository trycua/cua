"""Task command - Inspect and manage task environments.

Usage:
    cb task info <path>                # Show task details (provider, variants, etc.)
    cb task list [path]                # List tasks in a directory
    cb task create [path]              # Scaffold a new task
    cb task generate "<prompt>" [dir]  # AI-generate a task with Claude
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

PROFILE_PATH = Path(os.path.expanduser("~/.cua/cbprofile"))

PYPROJECT_TEMPLATE = """
[project]
name = "{project_name}"
version = "0.1.0"
license = "{license_name}"
authors = [
    {{ name = "{author_name}", email = "{author_email}" }}
]
requires-python = ">=3.11"
dependencies = [
    "beautifulsoup4>=4.14.2",
    "html5lib>=1.1",
    "pyquery>=2.0.1",
]

[tool.cua-bench]
description = "{description}"
difficulty = "{difficulty}"
category = "{category}"
tags = [{tags_toml}]
"""


def register_parser(subparsers):
    """Register the task command parser."""
    task_parser = subparsers.add_parser("task", help="Inspect and manage task environments")
    task_subparsers = task_parser.add_subparsers(dest="task_command")

    # cb task info <path>
    info_parser = task_subparsers.add_parser("info", help="Show detailed information about a task")
    info_parser.add_argument("path", help="Path to task directory (containing main.py)")

    # cb task list [path]
    list_parser = task_subparsers.add_parser("list", help="List tasks in a directory")
    list_parser.add_argument(
        "path",
        nargs="?",
        default=".",
        help="Path to directory containing tasks (default: current directory)",
    )

    # cb task create [path]
    create_parser = task_subparsers.add_parser("create", help="Scaffold a new task environment")
    create_parser.add_argument(
        "path",
        nargs="?",
        default=".",
        help="Target directory to create the task in (default: current directory)",
    )

    # cb task generate "<prompt>" [output_dir]
    generate_parser = task_subparsers.add_parser(
        "generate", help="Generate a task from a prompt using Claude"
    )
    generate_parser.add_argument(
        "prompt", help='Natural language description of the task to generate (e.g., "2048 game")'
    )
    generate_parser.add_argument(
        "output",
        nargs="?",
        help="Output directory path (optional, auto-generates from prompt if not provided)",
    )
    generate_parser.add_argument(
        "--no-interaction",
        action="store_true",
        dest="no_interaction",
        help="Skip interactive prompts and run Claude non-interactively",
    )


def execute(args):
    """Execute the task command."""
    task_command = getattr(args, "task_command", None)

    if task_command == "info":
        return cmd_info(args)
    elif task_command == "list":
        return cmd_list(args)
    elif task_command == "create":
        return cmd_create(args)
    elif task_command == "generate":
        return cmd_generate(args)
    else:
        print(f"{YELLOW}Usage: cb task <command> [args]{RESET}")
        print(f"\n{GREY}Commands:{RESET}")
        print("  info <path>             Show detailed information about a task")
        print("  list [path]             List tasks in a directory")
        print("  create [path]           Scaffold a new task environment")
        print("  generate <prompt> [dir] Generate a task using Claude")
        return 1


def cmd_info(args) -> int:
    """Show detailed information about a task."""
    task_path = Path(args.path).resolve()

    # Check if path exists
    if not task_path.exists():
        print(f"{RED}Error: Path not found: {task_path}{RESET}")
        return 1

    # Check for main.py
    main_py = task_path / "main.py" if task_path.is_dir() else task_path
    if task_path.is_dir() and not main_py.exists():
        print(f"{RED}Error: No main.py found in {task_path}{RESET}")
        return 1

    try:
        from cua_bench import make

        # Load the environment
        env = make(str(task_path))

        # Get tasks
        tasks = []
        if env.tasks_config_fn:
            tasks = env.tasks_config_fn()

        # Analyze provider types
        providers = set()
        os_types = set()
        for task in tasks:
            if hasattr(task, "computer") and task.computer:
                provider = task.computer.get("provider", "simulated")
                # Normalize to new names
                if provider == "webtop":
                    provider = "simulated"
                elif provider == "computer":
                    provider = "native"
                providers.add(provider)

                setup_config = task.computer.get("setup_config", {})
                if "os_type" in setup_config:
                    os_types.add(setup_config["os_type"])
                if "env_type" in setup_config:
                    os_types.add(setup_config["env_type"])

        # Default if no provider specified
        if not providers:
            providers.add("simulated")

        # Print info
        print(f"\n{BOLD}Task: {task_path.name}{RESET}")
        print("=" * 50)

        # Provider info with description
        provider_list = list(providers)
        print(f"\n{CYAN}Provider:{RESET}")
        for p in provider_list:
            if p == "simulated":
                print(f"  {GREEN}simulated{RESET} {GREY}(Playwright - fast, no Docker){RESET}")
                print(f"    {GREY}HTML/CSS desktop simulation in headless browser{RESET}")
                print(f"    {GREY}Run with: cb interact {task_path.name}{RESET}")
            elif p == "native":
                print(f"  {GREEN}native{RESET} {GREY}(Docker/QEMU - real OS){RESET}")
                print(f"    {GREY}Actual desktop environment with real applications{RESET}")
                print(f"    {GREY}Run with: cb run {task_path.name}{RESET}")

        # OS types
        if os_types:
            print(f"\n{CYAN}OS Types:{RESET} {', '.join(sorted(os_types))}")

        # Variants
        print(f"\n{CYAN}Variants:{RESET} {len(tasks)}")

        # Show first few task descriptions
        if tasks:
            print(f"\n{CYAN}Sample Tasks:{RESET}")
            for i, task in enumerate(tasks[:5]):
                desc = task.description if hasattr(task, "description") else str(task)
                # Truncate long descriptions
                if len(desc) > 70:
                    desc = desc[:67] + "..."
                print(f"  {GREY}[{i}]{RESET} {desc}")
            if len(tasks) > 5:
                print(f"  {GREY}... and {len(tasks) - 5} more{RESET}")

        # Functions available
        print(f"\n{CYAN}Functions:{RESET}")
        print(f"  setup:    {'✓' if env.setup_task_fn else '✗'}")
        print(f"  solve:    {'✓' if env.solve_task_fn else '✗'}")
        print(f"  evaluate: {'✓' if env.evaluate_task_fn else '✗'}")

        # Quick commands
        print(f"\n{CYAN}Quick Commands:{RESET}")
        if "simulated" in providers:
            print(f"  {GREY}# Interactive mode (browser visible):{RESET}")
            print(f"  cb interact {args.path} --task-id 0")
            print(f"  {GREY}# With oracle solution:{RESET}")
            print(f"  cb interact {args.path} --task-id 0 --oracle")
        if "native" in providers:
            print(f"  {GREY}# Run with Docker:{RESET}")
            print(f"  cb run {args.path} --variant-id 0 --oracle")

        print()
        return 0

    except Exception as e:
        print(f"{RED}Error loading task: {e}{RESET}")
        import traceback

        traceback.print_exc()
        return 1


def cmd_list(args) -> int:
    """List tasks in a directory."""
    search_path = Path(args.path).resolve()

    if not search_path.exists():
        print(f"{RED}Error: Path not found: {search_path}{RESET}")
        return 1

    # Find all directories with main.py
    tasks = []
    if search_path.is_file() and search_path.name == "main.py":
        tasks.append(search_path.parent)
    else:
        for item in search_path.iterdir():
            if item.is_dir() and (item / "main.py").exists():
                tasks.append(item)

    if not tasks:
        print(f"{GREY}No tasks found in {search_path}{RESET}")
        return 0

    print(f"\n{BOLD}Tasks in {search_path}{RESET}")
    print("=" * 60)
    print(f"\n{BOLD}{'NAME':<25} {'PROVIDER':<12} {'VARIANTS':<10}{RESET}")
    print("-" * 60)

    for task_path in sorted(tasks, key=lambda p: p.name):
        try:
            from cua_bench import make

            env = make(str(task_path))

            # Get task count and provider
            task_count = 0
            provider = "unknown"

            if env.tasks_config_fn:
                task_list = env.tasks_config_fn()
                task_count = len(task_list)
                if task_list and hasattr(task_list[0], "computer") and task_list[0].computer:
                    provider = task_list[0].computer.get("provider", "simulated")
                    # Normalize names
                    if provider == "webtop":
                        provider = "simulated"
                    elif provider == "computer":
                        provider = "native"
                else:
                    provider = "simulated"

            # Color code provider
            if provider == "simulated":
                provider_colored = f"{GREEN}{provider}{RESET}"
            elif provider == "native":
                provider_colored = f"{CYAN}{provider}{RESET}"
            else:
                provider_colored = provider

            print(f"{task_path.name:<25} {provider_colored:<21} {task_count:<10}")

        except Exception as e:
            print(f"{task_path.name:<25} {RED}error{RESET}        {GREY}{str(e)[:30]}{RESET}")

    print()
    print(f"{GREY}Tip: Run 'cb task info <name>' for detailed information{RESET}")
    print()
    return 0


# ============================================================================
# cmd_create - Scaffold a new task environment
# ============================================================================


def _prompt_input(prompt_text: str, default: str | None = None) -> str:
    """Prompt user for input with optional default."""
    suffix = f" [{default}]" if default else ""
    val = input(f"{prompt_text}{suffix}: ").strip()
    return val if val else (default or "")


def cmd_create(args) -> int:
    """Scaffold a new task environment."""
    target_path = Path(getattr(args, "path", ".") or ".")
    if not target_path.is_absolute():
        target_path = Path.cwd() / target_path

    if target_path.exists():
        if not target_path.is_dir():
            print(f"{RED}Error: Target path is not a directory: {target_path}{RESET}")
            return 1
        if any(target_path.iterdir()):
            print(f"{RED}Error: Target directory is not empty: {target_path}{RESET}")
            return 1

    # Derive default task name from directory name
    dir_name = target_path.name
    default_project_name = f"{dir_name.replace('_', '-').replace(' ', '-')}"
    if not default_project_name.endswith("-env"):
        default_project_name = f"{default_project_name}-env"

    # Interactive prompts
    project_name = default_project_name
    author_name = _prompt_input("Author name")
    author_email = _prompt_input("Author email")
    license_name = _prompt_input("License", "MIT")
    description = _prompt_input("Task description")
    difficulty = _prompt_input("Task difficulty (easy|medium|hard)", "easy")
    category = _prompt_input("Task category (e.g., grounding, software-engineering)", "grounding")
    tags_csv = _prompt_input("Tags (comma-separated)", "")

    tags_list = [t.strip() for t in tags_csv.split(",") if t.strip()] if tags_csv else []
    tags_toml = ", ".join([f'"{t}"' for t in tags_list])

    # Prepare files
    pyproject_path = target_path / "pyproject.toml"
    main_py_path = target_path / "main.py"
    gui_dir_path = target_path / "gui"

    pyproject_content = PYPROJECT_TEMPLATE.format(
        project_name=project_name,
        license_name=license_name,
        author_name=author_name,
        author_email=author_email,
        description=description,
        difficulty=difficulty,
        category=category,
        tags_toml=tags_toml,
    ).strip()

    # Load template files
    template_root = Path(__file__).resolve().parents[2] / "templates" / "starter_env"
    template_main_py = (template_root / "main.py").read_text(encoding="utf-8")

    target_path.mkdir(parents=True, exist_ok=True)
    pyproject_path.write_text(pyproject_content, encoding="utf-8")
    main_py_path.write_text(template_main_py, encoding="utf-8")

    # Copy gui directory
    shutil.copytree(template_root / "gui", gui_dir_path)

    print(f"\n{GREEN}✓ Created task at: {BOLD}{target_path}{RESET}")
    print(f"  {GREY}- {RESET}{pyproject_path.relative_to(Path.cwd())}")
    print(f"  {GREY}- {RESET}{main_py_path.relative_to(Path.cwd())}")
    print(f"  {GREY}- {RESET}{gui_dir_path.relative_to(Path.cwd())} (copied)")
    return 0


# ============================================================================
# cmd_generate - Generate a task using Claude
# ============================================================================


def _slugify(s: str) -> str:
    """Convert string to slug for directory names."""
    s = s.lower().strip()
    s = re.sub(r"[^a-z0-9\s_-]+", "", s)
    s = re.sub(r"[\s_-]+", "-", s)
    return s or "task"


def _ensure_profile(no_interaction: bool = False) -> dict:
    """Ensure user profile exists, prompting if needed."""
    if PROFILE_PATH.exists():
        try:
            return json.loads(PROFILE_PATH.read_text(encoding="utf-8"))
        except Exception:
            pass

    if no_interaction:
        return {"name": "Claude", "email": "noreply@anthropic.com"}

    PROFILE_PATH.parent.mkdir(parents=True, exist_ok=True)
    name = input("Author name: ").strip()
    email = input("Author email: ").strip()
    profile = {"name": name, "email": email}
    PROFILE_PATH.write_text(json.dumps(profile, indent=2), encoding="utf-8")
    print(f"{GREEN}✓ Saved profile to {PROFILE_PATH}{RESET}")
    return profile


def _scaffold_starter(prompt: str, dest_dir: Path, profile: dict) -> SimpleNamespace:
    """Scaffold a starter environment from template."""
    if dest_dir.exists():
        if any(dest_dir.iterdir()):
            print(f"{RED}Error: Target directory is not empty: {dest_dir}{RESET}")
            raise SystemExit(1)
    dest_dir.mkdir(parents=True, exist_ok=True)

    pyproject_path = dest_dir / "pyproject.toml"
    main_py_path = dest_dir / "main.py"
    gui_dir_path = dest_dir / "gui"

    template_root = Path(__file__).resolve().parents[2] / "templates" / "starter_env"
    template_main_py = (template_root / "main.py").read_text(encoding="utf-8")

    project_name = dest_dir.name.replace("_", "-").replace(" ", "-")
    pyproject_content = PYPROJECT_TEMPLATE.format(
        project_name=project_name,
        license_name="MIT",
        author_name=profile.get("name", ""),
        author_email=profile.get("email", ""),
        description=prompt,
        difficulty="easy",
        category="grounding",
        tags_toml="",
    )

    pyproject_path.write_text(pyproject_content, encoding="utf-8")
    main_py_path.write_text(template_main_py, encoding="utf-8")
    shutil.copytree(template_root / "gui", gui_dir_path)

    # Copy CLAUDE.md if present
    claude_md = template_root / "CLAUDE.md"
    if claude_md.exists():
        shutil.copy2(str(claude_md), str(dest_dir / "CLAUDE.md"))

    print(f"\n{GREEN}✓ Created task at: {BOLD}{dest_dir}{RESET}")

    try:
        pyproject_rel = pyproject_path.relative_to(Path.cwd())
    except ValueError:
        pyproject_rel = pyproject_path
    try:
        main_py_rel = main_py_path.relative_to(Path.cwd())
    except ValueError:
        main_py_rel = main_py_path
    try:
        gui_dir_rel = gui_dir_path.relative_to(Path.cwd())
    except ValueError:
        gui_dir_rel = gui_dir_path

    print(f"  {GREY}- {RESET}{pyproject_rel}")
    print(f"  {GREY}- {RESET}{main_py_rel}")
    print(f"  {GREY}- {RESET}{gui_dir_rel} (copied)")
    return SimpleNamespace(pyproject=pyproject_path, main=main_py_path, gui=gui_dir_path)


def _launch_claude(
    session_prompt: str,
    work_dir: Path,
    skip_permissions: bool = False,
    no_interaction: bool = False,
) -> None:
    """Launch Claude CLI to modify the scaffolded task."""
    import sys

    exe = shutil.which("claude")
    if not exe:
        print(
            f"{YELLOW}Warning:{RESET} 'claude' CLI not found on PATH. Skipping interactive session."
        )
        print(
            f'{GREY}Tip:{RESET} Install Claude Code CLI and run manually:\n  claude --add-dir {work_dir} "{session_prompt}"'
        )
        return

    # Note: CLAUDE_CODE_OAUTH_TOKEN is optional - Claude CLI can authenticate via keychain or other methods

    try:
        cmd = [exe, session_prompt, "--add-dir", str(work_dir)]
        if skip_permissions:
            cmd.extend(
                [
                    "--dangerously-skip-permissions",
                    "--permission-mode",
                    "acceptEdits",
                    "--allowedTools",
                    ",".join(
                        [
                            "Task",
                            "Bash",
                            "Glob",
                            "Grep",
                            "LS",
                            "Read",
                            "Edit",
                            "MultiEdit",
                            "Write",
                            "NotebookRead",
                            "NotebookEdit",
                            "WebFetch",
                            "Batch",
                            "TodoRead",
                            "TodoWrite",
                            "WebSearch",
                        ]
                    ),
                ]
            )
        if no_interaction:
            print(f"\n{CYAN}Running Claude Code...{RESET}")
            sys.stdout.flush()
            result = subprocess.run(cmd, cwd=str(work_dir), capture_output=True, text=True)
            if result.returncode == 0:
                print(f"{GREEN}✓ Claude Code completed successfully{RESET}")
                if result.stdout.strip():
                    print(f"\n{CYAN}Output:{RESET}")
                    print(result.stdout)
            else:
                print(f"{RED}✗ Claude Code failed with exit code {result.returncode}{RESET}")
                if result.stderr.strip():
                    print(f"\n{RED}Error:{RESET}")
                    print(result.stderr)
        else:
            print(f"\n{CYAN}Launching Claude Code in interactive mode...{RESET}")
            print(f"{GREY}Working directory:{RESET} {work_dir}")
            print()
            subprocess.call(cmd, cwd=str(work_dir))

    except Exception as e:
        print(f"{YELLOW}Warning:{RESET} Failed to launch Claude: {e}")
        print(f'{GREY}Tip:{RESET} Run manually:\n  claude --add-dir {work_dir} "{session_prompt}"')


def cmd_generate(args) -> int:
    """Generate a task from a prompt using Claude."""
    prompt: str = args.prompt
    if not prompt or not prompt.strip():
        print(
            f'{RED}Error: A non-empty prompt is required (e.g., cb task generate "2048 game"){RESET}'
        )
        return 1

    no_interaction = getattr(args, "no_interaction", False)
    profile = _ensure_profile(no_interaction)

    # Determine destination directory
    if hasattr(args, "output") and args.output:
        dest_dir = Path(args.output)
        if not dest_dir.is_absolute():
            dest_dir = Path.cwd() / dest_dir
    else:
        base_slug = _slugify(prompt)
        if not base_slug.endswith("-env") and not base_slug.endswith("_env"):
            base_slug = f"{base_slug}_env"
        dest_dir = Path.cwd() / base_slug.replace("-", "_")

    try:
        _scaffold_starter(prompt, dest_dir, profile)
    except SystemExit:
        return 1

    # Compose Claude instruction
    session_prompt = (
        f"You use cua-bench to create computer-use RL environments. Modify this starter env to be: {prompt}.\n"
        f"- Keep the decorator structure (@cb.tasks_config, @cb.setup_task, @cb.solve_task, @cb.evaluate_task).\n"
        f"- Update gui/index.html, evaluation, and solution logic as needed.\n"
    )

    skip_permissions = getattr(args, "dangerously_skip_permissions", False) or no_interaction
    _launch_claude(session_prompt, dest_dir, skip_permissions, no_interaction)

    if no_interaction:
        print(f"\n{GREEN}✓ Environment generated successfully.{RESET}")
        return 0

    print(f"\n{CYAN}Next steps:{RESET}")
    try:
        rel_env = dest_dir.relative_to(Path.cwd())
    except Exception:
        rel_env = dest_dir
    print(f"  • {BOLD}Open Claude again{RESET}:\n     {YELLOW}claude{RESET} --add-dir {rel_env}")
    print(
        f"  • {BOLD}Run interactively{RESET}:\n     {YELLOW}cb{RESET} interact {rel_env} {GREY} --view{RESET}"
    )
    print(
        f"  • {BOLD}Run baseline solution{RESET}:\n     {YELLOW}cb{RESET} interact {rel_env} {GREY} --oracle --view{RESET}"
    )
    print(f"  • {BOLD}List tasks{RESET}:\n     {YELLOW}cb{RESET} task list {rel_env}")
    return 0
