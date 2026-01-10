import base64
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from io import BytesIO
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except Exception:  # pragma: no cover
    import tomli as tomllib  # type: ignore


# Root registry path (~ expands to user home)
# task_registry = Path("~/cua-bench-registry").expanduser()
task_registry = Path(r"F:\Projects\cua\cua-bench-registry")
task_registry_url = "https://github.com/trycua/cua-bench-registry/tree/main/datasets/"

# Expected structure:
#   meta.json
#   datasets/
#     <dataset_id>/
#       <environment_id>/
#         pyproject.toml
#         main.py (optional)
task_datasets = task_registry / "datasets"
task_metadata = task_registry / "meta.json"
output_metadata = Path(__file__).parent / Path("./task_registry.json")


def read_meta(meta_path: Path):
    with open(meta_path, "r", encoding="utf-8") as f:
        return json.load(f)


def parse_pyproject(env_dir: Path):
    data = {
        "description": None,
        "license": None,
        "version": None,
        "authors": None,
        "difficulty": None,
        "category": None,
        "tags": None,
    }
    pp = env_dir / "pyproject.toml"
    if not pp.exists():
        return data
    with open(pp, "rb") as f:
        toml = tomllib.load(f)
    proj = toml.get("project", {})
    tool = toml.get("tool", {})
    cb = tool.get("cua-bench", {})
    data["description"] = cb.get("description") or proj.get("description")
    data["license"] = proj.get("license")
    data["version"] = proj.get("version")
    data["authors"] = proj.get("authors")
    data["difficulty"] = cb.get("difficulty")
    data["category"] = cb.get("category")
    data["tags"] = cb.get("tags")
    return data


def count_tasks_in_env(env_dir: Path) -> int:
    """Attempt to count tasks by importing the environment and calling tasks_config."""
    main_py = env_dir / "main.py"
    if not main_py.exists():
        return 0
    from cua_bench import make

    env = make(str(env_dir))
    if env.tasks_config_fn is None:
        env.close()
        return 0
    tasks = env.tasks_config_fn()
    n = len(tasks) if tasks is not None else 0
    env.close()
    return n


def generate_previews(dataset_id: str, env_dir: Path, max_previews: int = 5):
    """Run setup for first N tasks and save screenshot + task cfg.
    Returns a list of preview dicts with file paths and task info.
    """
    previews = []
    main_py = env_dir / "main.py"
    if not main_py.exists():
        return previews
    from cua_bench import make

    env = make(str(env_dir))
    if env.tasks_config_fn is None:
        env.close()
        return previews
    tasks = env.tasks_config_fn() or []
    count = min(len(tasks), max_previews)
    for i in range(count):
        screenshot_bytes, task_cfg = env.reset(task_id=i)

        # Wait a bit for the environment to stabilize
        time.sleep(5)

        # Take a fresh screenshot after waiting
        screenshot_bytes = env.provider.screenshot()

        # Convert to JPEG (quality 95) and encode as base64
        screenshot_b64 = None
        mime = "image/jpeg"
        from PIL import Image  # type: ignore

        img = Image.open(BytesIO(screenshot_bytes)).convert("RGB")
        buf = BytesIO()
        img.save(buf, format="JPEG", quality=95)
        screenshot_b64 = base64.b64encode(buf.getvalue()).decode("utf-8")

        task_dict = {
            "description": getattr(task_cfg, "description", None),
            "task_id": getattr(task_cfg, "task_id", None) or i,
            "metadata": getattr(task_cfg, "metadata", None),
        }
        previews.append(
            {
                "index": i,
                "screenshot": f"data:{mime};base64,{screenshot_b64}",
                "task": task_dict,
            }
        )

        env.close()  # Close environment to reset before next preview
    env.close()
    return previews


def process_environment(ds_id: str, ds_github: str, env_dir: Path):
    """Process a single environment directory."""
    env_id = env_dir.name
    meta = parse_pyproject(env_dir)
    num_tasks = count_tasks_in_env(env_dir)
    env_previews = generate_previews(ds_id, env_dir, max_previews=5)
    return {
        "id": env_id,
        "github_url": f"{ds_github}{env_id}",
        "description": meta["description"],
        "num_tasks": num_tasks,
        "license": meta["license"],
        "version": meta["version"],
        "authors": meta["authors"],
        "difficulty": meta["difficulty"],
        "category": meta["category"],
        "tags": meta["tags"],
        "previews": env_previews,
    }


def build_registry(task_datasets_root: Path, meta_entries, parallelism: int = 16):
    output = []
    for entry in meta_entries:
        ds_id = entry.get("id")
        ds_desc = entry.get("description")
        ds_path = task_datasets_root / ds_id
        ds_github = f"{task_registry_url}{ds_id}/"
        if not ds_path.exists():
            output.append(
                {
                    "id": ds_id,
                    "github_url": ds_github,
                    "description": ds_desc,
                    "num_environments": 0,
                    "num_tasks": 0,
                    "environments": [],
                }
            )
            continue

        env_dirs = sorted([p for p in ds_path.iterdir() if p.is_dir()])
        envs = []
        total_tasks = 0

        # Process environments in parallel
        with ThreadPoolExecutor(max_workers=parallelism) as executor:
            future_to_env = {
                executor.submit(process_environment, ds_id, ds_github, env_dir): env_dir
                for env_dir in env_dirs
            }

            for future in as_completed(future_to_env):
                try:
                    env_data = future.result()
                    envs.append(env_data)
                    total_tasks += env_data["num_tasks"]
                except Exception as e:
                    env_dir = future_to_env[future]
                    print(f"Error processing {env_dir}: {e}", file=sys.stderr)

        output.append(
            {
                "id": ds_id,
                "github_url": ds_github,
                "description": ds_desc,
                "num_environments": len(envs),
                "num_tasks": total_tasks,
                "environments": envs,
            }
        )
    return output


def main():
    import argparse

    parser = argparse.ArgumentParser(description="Dump task registry to JSON")
    parser.add_argument(
        "--parallelism",
        type=int,
        default=16,
        help="Number of parallel workers for processing environments (default: 16)",
    )
    args = parser.parse_args()

    if not task_metadata.exists():
        print(f"meta.json not found at {task_metadata}")
        return 1
    if not task_datasets.exists():
        print(f"datasets directory not found at {task_datasets}")
        return 1
    meta_entries = read_meta(task_metadata)
    reg = build_registry(task_datasets, meta_entries, parallelism=args.parallelism)
    output_metadata.write_text(json.dumps(reg, indent=2), encoding="utf-8")
    print(str(output_metadata))
    return 0


if __name__ == "__main__":
    sys.exit(main())
