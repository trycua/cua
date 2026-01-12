"""GUI-R1 processor - generates low-level click instructions in GUI-R1 format.

Outputs schema per row:
- images: [PIL.Image]
- annotated_images: [PIL.Image]
- gt_bbox: [normalized x1, y1, x2, y2]
- instruction: "click the UI element {aria label}"
- gt_action: "click"
- gt_input_text: "no input text"
- history: "None"
- task_type: "low"
- os_type: str (e.g., "win7", "macos", "ios")
- resolution: str (e.g., "1024x768")
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Tuple

from bs4 import BeautifulSoup
from tqdm.auto import tqdm

from .base import BaseProcessor


class GuiR1Processor(BaseProcessor):
    """Processor for gui-r1 format (low-level click instructions)."""

    def get_dataset_name(self) -> str:
        return "gui_r1_dataset"

    def process(self) -> List[Dict[str, Any]]:
        """Process snapshots into gui-r1 format."""
        pairs = self._find_task_pairs()
        rows: List[Dict[str, Any]] = []

        for shot, snapshot_html, tid, setup_config in tqdm(
            pairs, desc="Processing GUI-R1", unit="task"
        ):
            # snapshot_html is already a string from _find_task_pairs
            # setup_config contains os_type, resolution, etc.

            # Extract os_type and resolution from setup_config
            os_type = setup_config.get("os_type", "unknown")
            resolution = f"{setup_config.get('width', 0)}x{setup_config.get('height', 0)}"

            # Extract clickable elements with aria-labels and bboxes
            clickable_items = self._extract_clickable_elements(snapshot_html)

            # Get image dimensions for normalization
            from PIL import Image

            with Image.open(shot) as im:
                W, H = im.size

            # Create annotated image
            annotated_path = self._annotate_image(shot, snapshot_html, clickable_items)

            # Load images as PIL objects for HuggingFace datasets
            screenshot_img = Image.open(shot)
            annotated_img = Image.open(annotated_path)

            # Generate one row per clickable element
            for aria_label, bbox in clickable_items:
                # Normalize bbox to [x1, y1, x2, y2] format with values in [0, 1]
                x1 = max(0.0, min(bbox["x"] / W, 1.0))
                y1 = max(0.0, min(bbox["y"] / H, 1.0))
                x2 = max(0.0, min((bbox["x"] + bbox["width"]) / W, 1.0))
                y2 = max(0.0, min((bbox["y"] + bbox["height"]) / H, 1.0))

                gt_bbox = [round(x1, 4), round(y1, 4), round(x2, 4), round(y2, 4)]

                row = {
                    "images": [screenshot_img],
                    "annotated_images": [annotated_img],
                    "gt_bbox": gt_bbox,
                    "instruction": f"click the UI element {aria_label}",
                    "gt_action": "click",
                    "gt_input_text": "no input text",
                    "history": "None",
                    "task_type": "low",
                    "os_type": os_type,
                    "resolution": resolution,
                }
                rows.append(row)

        return rows

    def _extract_clickable_elements(self, snapshot_html: str) -> List[Tuple[str, Dict[str, float]]]:
        """Extract clickable elements with aria-labels and bboxes.

        Returns:
            List of tuples (aria_label, bbox) for clickable elements
        """
        soup = BeautifulSoup(snapshot_html, "html5lib")
        items: List[Tuple[str, Dict[str, float]]] = []
        seen = set()

        # Look for elements with aria-label that are likely clickable
        # We'll prioritize elements with both aria-label and data-instruction
        for tag in soup.find_all(attrs={"aria-label": True}):
            aria_label = tag.get("aria-label")
            if not isinstance(aria_label, str):
                continue
            aria_label = aria_label.strip()
            if not aria_label:
                continue

            # Require center-hit to be true
            if tag.get("data-bbox-center-hit") != "true":
                continue

            # Extract bbox
            try:
                x = float(tag.get("data-bbox-x"))
                y = float(tag.get("data-bbox-y"))
                w = float(tag.get("data-bbox-width"))
                h = float(tag.get("data-bbox-height"))
                bbox = {"x": x, "y": y, "width": w, "height": h}
            except (TypeError, ValueError):
                continue

            # Deduplicate by aria_label + bbox
            key = (aria_label, x, y, w, h)
            if key in seen:
                continue
            seen.add(key)
            items.append((aria_label, bbox))

        # If no aria-labels found, fall back to data-instruction elements
        if not items:
            for tag in soup.find_all(attrs={"data-instruction": True}):
                instruction = tag.get("data-instruction")
                if not isinstance(instruction, str):
                    continue
                instruction = instruction.strip()
                if not instruction:
                    continue

                if tag.get("data-bbox-center-hit") != "true":
                    continue

                try:
                    x = float(tag.get("data-bbox-x"))
                    y = float(tag.get("data-bbox-y"))
                    w = float(tag.get("data-bbox-width"))
                    h = float(tag.get("data-bbox-height"))
                    bbox = {"x": x, "y": y, "width": w, "height": h}
                except (TypeError, ValueError):
                    continue

                key = (instruction, x, y, w, h)
                if key in seen:
                    continue
                seen.add(key)
                items.append((instruction, bbox))

        return items

    def _annotate_image(
        self,
        shot_path: Path,
        snapshot_html: str,
        clickable_items: List[Tuple[str, Dict[str, float]]],
    ) -> Path:
        """Create an annotated copy of the screenshot with bboxes and labels."""
        try:
            from PIL import Image as _PIL_Image
            from PIL import ImageDraw as _PIL_Draw
            from PIL import ImageFont as _PIL_Font
        except Exception:
            return shot_path

        try:
            im = _PIL_Image.open(shot_path).convert("RGB")
        except Exception:
            return shot_path

        draw = _PIL_Draw.Draw(im)
        try:
            font = _PIL_Font.load_default()
        except Exception:
            font = None

        # Draw each bbox and label
        for label, bbox in clickable_items:
            try:
                x = int(round(bbox.get("x", 0)))
                y = int(round(bbox.get("y", 0)))
                w = int(round(bbox.get("width", 0)))
                h = int(round(bbox.get("height", 0)))
            except Exception:
                continue

            if w <= 0 or h <= 0:
                continue

            x2 = x + w
            y2 = y + h
            color = (30, 144, 255)  # DodgerBlue color for GUI-R1
            draw.rectangle((x, y, x2, y2), outline=color, width=3)

            # Draw label
            if label:
                pad = 2
                if font:
                    try:
                        tw, th = font.getsize(label)
                    except Exception:
                        tw, th = (len(label) * 6, 12)
                else:
                    tw, th = (len(label) * 6, 12)

                bx0 = x
                by0 = max(0, y - th - 6)
                bx1 = bx0 + tw + pad * 2
                by1 = by0 + th + pad * 2
                draw.rectangle((bx0, by0, bx1, by1), fill=color)
                tx = bx0 + pad
                ty = by0 + pad
                if font:
                    draw.text((tx, ty), label, fill=(255, 255, 255), font=font)

        out_path = shot_path.with_name(shot_path.stem + "_gui_r1_annotated" + shot_path.suffix)
        try:
            im.save(out_path)
            return out_path
        except Exception:
            return shot_path
