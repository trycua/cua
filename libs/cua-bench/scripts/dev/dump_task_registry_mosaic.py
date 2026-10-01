import json
import sys
import time
from io import BytesIO
from pathlib import Path

try:
    pass  # Python 3.11+
except Exception:  # pragma: no cover
    pass  # type: ignore


# Root registry path (~ expands to user home)
# task_registry = Path("~/cua-bench-registry").expanduser()
task_registry = Path(r"F:\Projects\cua\cua-bench-registry")

# Expected structure:
#   meta.json
#   datasets/
#     <dataset_id>/
#       <environment_id>/
#         pyproject.toml
#         main.py (optional)
task_datasets = task_registry / "datasets"
task_metadata = task_registry / "meta.json"
output_root = Path(__file__).parent / Path(".out")

# Filter datasets to process (None = process all datasets)
DATASETS_TO_PROCESS = [
    # "cua-bench-basic",
    "cua-bench-real"
]

# Maximum number of tasks to process per environment (None = process all tasks)
MAX_TASKS_PER_ENVIRONMENT = 1


def read_meta(meta_path: Path):
    with open(meta_path, "r", encoding="utf-8") as f:
        return json.load(f)


def generate_screenshots_for_env(env_dir: Path, output_dir: Path, max_tasks: int | None = None):
    """Run setup for each task and save screenshot as PNG.
    Returns a list of screenshot paths.
    """
    from cua_bench import make
    from PIL import Image  # type: ignore

    screenshots = []
    main_py = env_dir / "main.py"
    if not main_py.exists():
        return screenshots

    env = make(str(env_dir))
    if env.tasks_config_fn is None:
        env.close()
        return screenshots

    tasks = env.tasks_config_fn() or []

    # Limit number of tasks if max_tasks is set
    num_tasks = len(tasks) if max_tasks is None else min(len(tasks), max_tasks)

    for i in range(num_tasks):
        try:
            screenshot_bytes, task_cfg = env.reset(task_id=i)

            # Wait a bit for the environment to stabilize
            time.sleep(5)

            # Take a fresh screenshot after waiting
            screenshot_bytes = env.provider.screenshot()

            # Convert to PNG and save
            img = Image.open(BytesIO(screenshot_bytes)).convert("RGB")
            screenshot_path = output_dir / f"{env_dir.name}_task_{i}.png"
            img.save(screenshot_path, format="PNG")
            screenshots.append(screenshot_path)

            env.close()  # Close environment to reset before next task
        except Exception as e:
            print(f"Error processing task {i} in {env_dir.name}: {e}")
            continue

    env.close()
    return screenshots


def create_mosaic(screenshot_paths: list[Path], output_path: Path, max_cols: int = 5):
    """Create a mosaic image from a list of screenshot paths."""
    from PIL import Image  # type: ignore

    if not screenshot_paths:
        return

    # Load all images
    images = [Image.open(p) for p in screenshot_paths]

    # Get dimensions (assume all images are the same size)
    if not images:
        return

    img_width, img_height = images[0].size

    # Calculate grid dimensions
    num_images = len(images)
    num_cols = min(num_images, max_cols)
    num_rows = (num_images + num_cols - 1) // num_cols  # Ceiling division

    # Create mosaic canvas
    mosaic_width = img_width * num_cols
    mosaic_height = img_height * num_rows
    mosaic = Image.new("RGB", (mosaic_width, mosaic_height), color=(255, 255, 255))

    # Paste images into mosaic
    for idx, img in enumerate(images):
        row = idx // num_cols
        col = idx % num_cols
        x = col * img_width
        y = row * img_height
        mosaic.paste(img, (x, y))

    # Save mosaic
    mosaic.save(output_path, format="PNG")
    print(f"Created mosaic: {output_path}")


def process_dataset(dataset_path: Path, dataset_id: str):
    """Process a single dataset: generate screenshots and mosaic."""
    dataset_output = output_root / dataset_id
    dataset_output.mkdir(parents=True, exist_ok=True)

    all_screenshots = []

    # Process each environment in the dataset
    for env_dir in sorted([p for p in dataset_path.iterdir() if p.is_dir()]):
        if env_dir.name.startswith("."):  # Skip hidden directories
            continue

        env_id = env_dir.name
        print(f"Processing {dataset_id}/{env_id}...")

        try:
            screenshots = generate_screenshots_for_env(
                env_dir, dataset_output, max_tasks=MAX_TASKS_PER_ENVIRONMENT
            )
            all_screenshots.extend(screenshots)
            print(f"  Generated {len(screenshots)} screenshots")
        except Exception as e:
            print(f"  Error processing {env_id}: {e}")
            continue

    # Create mosaic for the entire dataset
    if all_screenshots:
        mosaic_path = dataset_output / f"{dataset_id}_mosaic.png"
        create_mosaic(all_screenshots, mosaic_path, max_cols=5)
        print(f"Dataset {dataset_id}: {len(all_screenshots)} total screenshots")
    else:
        print(f"Dataset {dataset_id}: No screenshots generated")


def main():
    if not task_metadata.exists():
        print(f"meta.json not found at {task_metadata}")
        return 1
    if not task_datasets.exists():
        print(f"datasets directory not found at {task_datasets}")
        return 1

    # Create output root directory
    output_root.mkdir(parents=True, exist_ok=True)

    # Read metadata to get dataset list
    meta_entries = read_meta(task_metadata)

    # Process each dataset
    for entry in meta_entries:
        ds_id = entry.get("id")

        # Filter datasets if DATASETS_TO_PROCESS is set
        if DATASETS_TO_PROCESS is not None and ds_id not in DATASETS_TO_PROCESS:
            print(f"Skipping dataset: {ds_id} (not in filter list)")
            continue

        ds_path = task_datasets / ds_id

        if not ds_path.exists():
            print(f"Dataset directory not found: {ds_path}")
            continue

        print(f"\n{'='*60}")
        print(f"Processing dataset: {ds_id}")
        print(f"{'='*60}")
        process_dataset(ds_path, ds_id)

    print(f"\n{'='*60}")
    print(f"All outputs saved to: {output_root}")
    print(f"{'='*60}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
