"""Core classes and functions for cua-bench."""

import importlib.util
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional


@dataclass
class Task:
    """Represents a single task to be executed."""

    description: str
    task_id: Optional[str] = None
    metadata: Optional[dict] = None
    computer: Optional[dict] = None


def make(env_name: str, *, split: str = "train") -> Any:
    """Create an Environment by loading the env's main.py as a module.

    Args:
        env_name: Path to the environment directory (must contain main.py)
        split: Dataset split to use for decorated functions (e.g., 'train', 'test')

    Returns:
        Environment instance
    """
    from .environment import Environment

    env_path = Path(env_name)
    if not env_path.exists():
        raise FileNotFoundError(f"Environment not found: {env_name}")

    main_file = env_path / "main.py"
    if not main_file.exists():
        raise FileNotFoundError(f"main.py not found in `{env_path.resolve().name}`")

    spec = importlib.util.spec_from_file_location("env_module", main_file)
    if spec is None or spec.loader is None:
        raise ImportError(f"Could not load module from {main_file}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    return Environment.make_from_module(module, env_path=env_path, split=split)


def interact(env_path: str, task_id: int = 0) -> None:
    """Run an environment interactively with simplified output.

    Args:
        env_path: Path to the environment directory
        task_id: Task ID to run (default: 0)
    """
    # ANSI colors
    GREEN = "\033[92m"
    GREY = "\033[90m"
    RED = "\033[91m"
    RESET = "\033[0m"

    from .cli.main import print_banner

    print_banner()
    print(f"{GREY}Loading environment {env_path}...{RESET}")

    # If env_path is a file, use the parent directory
    if Path(env_path).is_file():
        env_path = Path(env_path).parent

    try:
        # Create environment with interactive settings
        env = make(env_path)
        env.headless = False
        env.print_actions = True

        # Run task setup
        _t0 = time.perf_counter()
        _screenshot, _task_cfg = env.reset(task_id=task_id)
        _elapsed = time.perf_counter() - _t0

        # Yield control to user
        print(f"{GREEN}✓ Setup complete in {_elapsed:.2f}s{RESET}")
        print(f"{GREY}Press Enter to close...{RESET}")
        input()

        # Run task evaluation
        if hasattr(env, "evaluate_task_fn") and env.evaluate_task_fn:
            result = env.evaluate()
            print(f"{GREEN}✓ Evaluation result: {result}{RESET}")
        else:
            print(f"{GREY}✓ Evaluation not implemented{RESET}")
    except Exception as e:
        print(f"{RED}Error: {e}{RESET}")
        import traceback

        traceback.print_exc()
    finally:
        if "env" in locals():
            env.close()
