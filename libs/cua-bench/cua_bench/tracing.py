from __future__ import annotations

from dataclasses import asdict, is_dataclass
from datetime import datetime
from io import BytesIO
from typing import List, Dict, Any, Optional
from pathlib import Path
import json
import uuid
from PIL import Image
from datasets import Dataset, Features, Sequence, Value, Image as HFImage


class Tracing:
    """Lightweight trajectory tracing using Hugging Face Datasets.

    Records events with arbitrary JSON metadata and a list of PIL images.
    Exposes a datasets.Dataset-compatible interface for saving/pushing.
    """

    def __init__(self, env: Any) -> None:
        self.env = env
        self._trajectory_id: Optional[str] = None
        self._rows: List[Dict[str, Any]] = []
        self._dataset: Optional[Dataset] = None

    # --- Lifecycle ---
    def start(self, trajectory_id: Optional[str] = None) -> str:
        """Start a new trajectory. Resets any previously recorded rows.

        Returns the trajectory_id used.
        """
        self._trajectory_id = trajectory_id or str(uuid.uuid4())
        self._rows = []
        self._dataset = None
        return self._trajectory_id

    @property
    def trajectory_id(self) -> Optional[str]:
        return self._trajectory_id

    # --- Recording ---
    def record(self, event_name: str, data_dict: Dict[str, Any], data_images: List[Image.Image | bytes] | None = None) -> None:
        if self._trajectory_id is None:
            # If tracing hasn't been started, ignore silently to avoid surprising callers
            return
        # Ensure JSON-serializable dict
        payload: Dict[str, Any]
        if is_dataclass(data_dict):  # type: ignore[arg-type]
            payload = asdict(data_dict)  # type: ignore[assignment]
        else:
            payload = dict(data_dict)
        # Serialize JSON as string; leave images as PIL for HF to ingest
        row = {
            "event_name": str(event_name),
            "data_json": json.dumps(payload, ensure_ascii=False),
            "data_images": list(data_images or []),
            "trajectory_id": str(self._trajectory_id),
            "timestamp": datetime.utcnow().isoformat(timespec="milliseconds") + "Z",
        }
        self._rows.append(row)
        # Invalidate cached dataset; it can be rebuilt lazily
        self._dataset = None

    # --- Accessors ---
    @property
    def dataset(self) -> Dataset:
        """Return a HF Dataset built from current rows, constructing lazily.

        Features schema:
          - event_name: str
          - data_json: dict (JSON-like)
          - data_images: Sequence(Image)
          - trajectory_id: str
          - timestamp: str
        """
        if self._dataset is None:
            feats = Features(
                {
                    "event_name": Value("string"),
                    "data_json": Value("string"),
                    "data_images": Sequence(HFImage()),
                    "trajectory_id": Value("string"),
                    "timestamp": Value("string"),
                }
            )
            # Convert inputs to a format compatible with HF Image feature (prefer PIL Images)
            def _row_adapter(r: Dict[str, Any]) -> Dict[str, Any]:
                imgs = r.get("data_images") or []
                adapted = []
                for img in imgs:
                    if isinstance(img, Image.Image):
                        adapted.append(img)
                    elif isinstance(img, (bytes, bytearray)):
                        pil = Image.open(BytesIO(img)).convert("RGBA")
                        adapted.append(pil)
                    else:
                        raise ValueError(f"Unsupported image type: {type(img)}")
                out = dict(r)
                out["data_images"] = adapted
                return out
            rows = [_row_adapter(r) for r in self._rows]
            self._dataset = Dataset.from_list(rows, features=feats)
        return self._dataset

    # --- Persistence ---
    def save_to_disk(self, output_dir: str, save_pngs: bool = False, image_dir: Optional[str] = None, filter_events: Optional[List[str]] = None) -> None:
        if save_pngs:
            self._save_to_disk_with_separate_pngs(output_dir, image_dir, filter_events)
        else:
            if len(self._rows) == 0:
                raise RuntimeError("No tracing data to save")
            dataset_to_save = self.dataset
            if filter_events:
                dataset_to_save = self._filter_dataset_by_events(self.dataset, filter_events)
            dataset_to_save.save_to_disk(output_dir)
    
    def _save_to_disk_with_separate_pngs(self, output_dir: str, image_dir: Optional[str] = None, filter_events: Optional[List[str]] = None) -> None:
        """Save dataset with PNG images extracted to separate files."""
        import hashlib
        from pathlib import Path
        
        output_path = Path(output_dir)
        imgs_dir = Path(image_dir) if image_dir else output_path / "imgs"
        imgs_dir.mkdir(parents=True, exist_ok=True)

        print(f"Saving tracing dataset to {output_dir} with images in {imgs_dir}")

        imgs_dir_name = imgs_dir.name
        
        # Filter rows by event_name if specified
        rows_to_process = self._rows
        if filter_events:
            rows_to_process = [row for row in self._rows if row.get("event_name") in filter_events]
            print(f"  Filtered to {len(rows_to_process)} rows with events: {', '.join(filter_events)}")
        
        # Create modified dataset with image paths instead of PIL images
        modified_rows = []
        
        for row in rows_to_process:
            modified_row = dict(row)
            image_paths = []
            
            for img in row.get("data_images", []):
                if isinstance(img, Image.Image):
                    # Generate hash-based filename
                    img_bytes = BytesIO()
                    img.save(img_bytes, format='PNG')
                    img_bytes.seek(0)
                    img_data = img_bytes.getvalue()
                    
                    # Create hash from image data
                    img_hash = hashlib.sha256(img_data).hexdigest()[:16]
                    img_filename = f"{img_hash}.png"
                    img_path = imgs_dir / img_filename
                    
                    # Save PNG file
                    img.save(str(img_path), format='PNG')
                    
                    # Store relative path
                    image_paths.append(f"{imgs_dir_name}/{img_filename}")
                elif isinstance(img, (bytes, bytearray)):
                    # Convert bytes to PIL first
                    pil_img = Image.open(BytesIO(img)).convert("RGBA")
                    
                    # Generate hash-based filename
                    img_hash = hashlib.sha256(img).hexdigest()[:16]
                    img_filename = f"{img_hash}.png"
                    img_path = imgs_dir / img_filename
                    
                    # Save PNG file
                    pil_img.save(str(img_path), format='PNG')
                    
                    # Store relative path
                    image_paths.append(f"{imgs_dir_name}/{img_filename}")
            
            # Replace data_images with file paths
            modified_row["data_images"] = image_paths
            modified_rows.append(modified_row)
        
        # Create new dataset with string paths instead of Image features
        from datasets import Features, Sequence, Value
        
        feats = Features(
            {
                "event_name": Value("string"),
                "data_json": Value("string"),
                "data_images": Sequence(Value("string")),  # Changed to string paths
                "trajectory_id": Value("string"),
                "timestamp": Value("string"),
            }
        )
        
        modified_dataset = Dataset.from_list(modified_rows, features=feats)
        modified_dataset.save_to_disk(str(output_path))

    def _filter_dataset_by_events(self, dataset: Dataset, filter_events: List[str]) -> Dataset:
        """Filter dataset to only include rows with specified event_name values."""
        filtered_indices = [i for i, row in enumerate(dataset) if row["event_name"] in filter_events]
        filtered_dataset = dataset.select(filtered_indices)
        print(f"  Filtered to {len(filtered_dataset)} rows with events: {', '.join(filter_events)}")
        return filtered_dataset

    def push_to_hub(self, repo_id: str, private: bool | None = None) -> str:
        # Returns revision or commit hash
        return self.dataset.push_to_hub(repo_id=repo_id, private=bool(private))

    # --- Helpers ---
    @staticmethod
    def bytes_to_image(png_bytes: bytes) -> Image.Image:
        return Image.open(BytesIO(png_bytes)).convert("RGBA")
