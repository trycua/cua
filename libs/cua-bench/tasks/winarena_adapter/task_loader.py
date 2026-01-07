"""Load WAA task definitions from embedded JSON files."""

import json
from pathlib import Path
from typing import List, Optional

import cua_bench as cb

# Path to embedded task data
DATA_PATH = Path(__file__).parent / "data"

# All WAA domains
DOMAINS = [
    "chrome",
    "clock",
    "file_explorer",
    "libreoffice_calc",
    "libreoffice_writer",
    "microsoft_paint",
    "msedge",
    "notepad",
    "settings",
    "vlc",
    "vs_code",
    "windows_calc",
]


def load_waa_tasks(
    domains: Optional[List[str]] = None,
    difficulty: str = "normal",
) -> List[cb.Task]:
    """Load WAA benchmark tasks from embedded JSON files.

    Args:
        domains: List of domains to load. If None, loads all domains.
        difficulty: "normal" for examples/, "hard" for examples_noctxt/

    Returns:
        List of cb.Task objects
    """
    tasks = []

    # Select task directory based on difficulty
    if difficulty == "hard":
        examples_dir = DATA_PATH / "examples_noctxt"
    else:
        examples_dir = DATA_PATH / "examples"

    # Use all domains if not specified
    target_domains = domains or DOMAINS

    for domain in target_domains:
        domain_path = examples_dir / domain
        if not domain_path.exists():
            continue

        for task_file in sorted(domain_path.glob("*.json")):
            try:
                task_data = json.loads(task_file.read_text(encoding="utf-8"))
            except (json.JSONDecodeError, UnicodeDecodeError) as e:
                print(f"Warning: Failed to load {task_file}: {e}")
                continue

            # Extract task fields
            task_id = task_data.get("id", task_file.stem)
            instruction = task_data.get("instruction", "")
            config = task_data.get("config", [])
            evaluator = task_data.get("evaluator", {})
            snapshot = task_data.get("snapshot")
            related_apps = task_data.get("related_apps", [])
            source = task_data.get("source")

            tasks.append(
                cb.Task(
                    description=instruction,
                    task_id=task_id,
                    metadata={
                        "domain": domain,
                        "difficulty": difficulty,
                        "config": config,
                        "evaluator": evaluator,
                        "snapshot": snapshot,
                        "related_apps": related_apps,
                        "source": source,
                    },
                    computer={
                        "provider": "native",
                        "setup_config": {
                            "os_type": "win11",
                            "width": 1920,
                            "height": 1080,
                            "image": "trycua/winarena:latest",
                        },
                    },
                )
            )

    return tasks


def get_task_by_id(task_id: str, difficulty: str = "normal") -> Optional[cb.Task]:
    """Get a specific task by its ID.

    Args:
        task_id: The task ID to find
        difficulty: "normal" or "hard"

    Returns:
        The matching task, or None if not found
    """
    tasks = load_waa_tasks(difficulty=difficulty)
    for task in tasks:
        if task.task_id == task_id:
            return task
    return None


def get_tasks_by_domain(domain: str, difficulty: str = "normal") -> List[cb.Task]:
    """Get all tasks for a specific domain.

    Args:
        domain: The domain name (e.g., "chrome", "notepad")
        difficulty: "normal" or "hard"

    Returns:
        List of tasks for that domain
    """
    return load_waa_tasks(domains=[domain], difficulty=difficulty)


def list_domains() -> List[str]:
    """Return list of available domains."""
    return DOMAINS.copy()


def count_tasks_by_domain(difficulty: str = "normal") -> dict:
    """Count tasks per domain.

    Returns:
        Dict mapping domain name to task count
    """
    counts = {}
    for domain in DOMAINS:
        tasks = get_tasks_by_domain(domain, difficulty)
        counts[domain] = len(tasks)
    return counts
