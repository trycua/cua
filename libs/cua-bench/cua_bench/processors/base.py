"""Base processor interface for snapshot processing."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Tuple


@dataclass
class ProcessorArgs:
    """Arguments for processor execution."""

    outputs_path: Path
    dataset_name: str | None = None
    save_dir: Path | None = None
    push_to_hub: bool = False
    repo_id: str | None = None
    private: bool = False
    max_samples: int | None = None


class BaseProcessor(ABC):
    """Base class for snapshot processors.

    A processor converts batch dump outputs (screenshots + snapshots)
    into a specific dataset format.
    """

    def __init__(self, args: ProcessorArgs):
        """Initialize the processor with arguments."""
        self.args = args

    @abstractmethod
    def process(self) -> List[Dict[str, Any]]:
        """Process the snapshots and return a list of dataset rows.

        Returns:
            List of dictionaries, where each dict is a row in the dataset.
            The schema depends on the specific processor implementation.
        """
        pass

    @abstractmethod
    def get_dataset_name(self) -> str:
        """Get the default dataset name for this processor."""
        pass

    def _find_task_pairs(self) -> List[Tuple[Path, str, int, Dict[str, Any]]]:
        """Find all (screenshot_path, snapshot_html, task_id, setup_config) tuples from HuggingFace dataset directories.

        New format only: outputs/task_N_trace/{arrow files}
        Each trace directory contains a dataset with event data including reset events with snapshots.

        Returns:
            List of tuples (screenshot_path, snapshot_html_string, task_id, setup_config)
        """
        import json

        from datasets import load_from_disk

        pairs: List[Tuple[Path, str, int, Dict[str, Any]]] = []
        outputs_path = self.args.outputs_path

        for trace_dir in sorted(outputs_path.glob("task_*_trace")):
            try:
                # Extract task ID from directory name
                tid = int(trace_dir.name.split("_")[1])

                # Load the dataset from disk
                ds = load_from_disk(str(trace_dir))

                # Find the 'reset' event which contains the snapshot
                reset_row = None
                screenshot_image = None

                for row in ds:
                    if row["event_name"] == "reset":
                        reset_row = row
                        # Get the screenshot image from data_images
                        if row.get("data_images") and len(row["data_images"]) > 0:
                            screenshot_image = row["data_images"][0]
                        break

                if not reset_row:
                    continue

                # Parse the data_json to extract snapshot HTML and setup_config
                data_json = json.loads(reset_row["data_json"])
                snapshot_data = data_json.get("snapshot", {})
                windows = snapshot_data.get("windows", [])
                setup_config = data_json.get("setup_config", {})

                # Find the webview window which contains the actual task HTML
                snapshot_html = None
                for window in windows:
                    if window.get("window_type") == "webview":
                        snapshot_html = window.get("html", "")
                        break

                if not snapshot_html or not screenshot_image:
                    continue

                # Save screenshot to a temporary file

                # Create a temp file for the screenshot
                temp_dir = trace_dir / "temp"
                temp_dir.mkdir(exist_ok=True)
                screenshot_path = temp_dir / f"task_{tid}_screenshot.png"

                # Save the PIL image
                screenshot_image.save(screenshot_path)

                pairs.append((screenshot_path, snapshot_html, tid, setup_config))

            except Exception as e:
                # Skip directories that can't be loaded
                print(f"Warning: Could not load {trace_dir}: {e}")
                import traceback

                traceback.print_exc()
                continue

        # Apply max_samples limit if specified
        if self.args.max_samples is not None:
            pairs = pairs[: self.args.max_samples]

        return pairs

    def save_jsonl(self, rows: List[Dict[str, Any]], save_dir: Path, dataset_name: str) -> Path:
        """Save dataset rows as JSONL file.

        Args:
            rows: List of dataset row dictionaries
            save_dir: Directory to save to
            dataset_name: Name of the dataset file (without extension)

        Returns:
            Path to the saved file
        """
        import json

        save_dir.mkdir(parents=True, exist_ok=True)
        out_path = save_dir / f"{dataset_name}.jsonl"
        with out_path.open("w", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        return out_path

    def save_to_disk(self, rows: List[Dict[str, Any]], save_dir: Path, dataset_name: str) -> Path:
        """Save dataset rows using HuggingFace's save_to_disk method.

        This method properly handles PIL images and other complex data types
        that cannot be serialized to JSON.

        Args:
            rows: List of dataset row dictionaries
            save_dir: Directory to save to
            dataset_name: Name of the dataset directory

        Returns:
            Path to the saved dataset directory
        """
        from datasets import Dataset

        save_dir.mkdir(parents=True, exist_ok=True)
        out_path = save_dir / dataset_name

        # Create HuggingFace Dataset from rows
        ds = Dataset.from_list(rows)

        # Save to disk using HuggingFace's format
        ds.save_to_disk(str(out_path))

        return out_path

    def push_to_hub(self, rows: List[Dict[str, Any]], repo_id: str, private: bool) -> None:
        """Push dataset to Hugging Face Hub.

        Args:
            rows: List of dataset row dictionaries
            repo_id: HuggingFace repository ID (e.g., "username/dataset-name")
            private: Whether to make the dataset private
        """
        from datasets import Dataset

        # This is a basic implementation - subclasses may override for custom schemas
        ds = Dataset.from_list(rows)
        ds.push_to_hub(repo_id, private=private)
