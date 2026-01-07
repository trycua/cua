"""AgUVis Stage 1 processor - generates action augmentation dataset.

Outputs schema per row:
- images: [screenshot_path]
- annotated_images: [annotated_screenshot_path]
- texts: list of {assistant: str, user: str}
- source: "cua-bench"
"""

from __future__ import annotations

import json
import random
from pathlib import Path
from typing import Any, Dict, List, Tuple

from bs4 import BeautifulSoup
from tqdm.auto import tqdm

from .base import BaseProcessor


class AgUVisStage1Processor(BaseProcessor):
    """Processor for aguvis-stage-1 format (action augmentation dataset)."""
    
    def get_dataset_name(self) -> str:
        return "aguvis_stage_1_dataset"
    
    def process(self) -> List[Dict[str, Any]]:
        """Process snapshots into aguvis-stage-1 format."""
        pairs = self._find_task_pairs()
        rows: List[Dict[str, Any]] = []
        
        for shot, snapshot_html, tid, setup_config in tqdm(pairs, desc="Processing", unit="task"):
            # Seed for reproducibility per task
            random.seed(tid)
            # snapshot_html is already a string from _find_task_pairs
            # setup_config contains os_type, resolution, etc. (not used in aguvis-stage-1)
            
            # Extract all data sources
            action_items = self._extract_actions(snapshot_html)
            instr_items = self._extract_instructions_with_bbox(snapshot_html)
            aria_items = self._extract_aria_with_bbox(snapshot_html)
            text_items = self._extract_text_with_bbox(snapshot_html)
            
            # Build augmented text pairs
            texts = self._build_augmented_texts(
                instr_items, aria_items, text_items, shot, n=5, action_items=action_items
            )
            
            # Create annotated image
            annotated_path = self._annotate_image(shot, snapshot_html)
            
            # Load images as PIL objects for HuggingFace datasets
            from PIL import Image
            screenshot_img = Image.open(shot)
            annotated_img = Image.open(annotated_path)
            
            row = {
                "images": [screenshot_img],
                "annotated_images": [annotated_img],
                "texts": texts,
                "source": "cua-bench",
            }
            rows.append(row)
        
        return rows
    
    def _extract_instructions_with_bbox(self, snapshot_html: str) -> List[Tuple[str, Dict[str, float]]]:
        """Return list of (instruction_text, bbox) with bbox from data-bbox-* on the element."""
        soup = BeautifulSoup(snapshot_html, "html5lib")
        items: List[Tuple[str, Dict[str, float]]] = []
        seen = set()
        for tag in soup.find_all(attrs={"data-instruction": True}):
            val = tag.get("data-instruction")
            if not isinstance(val, str):
                continue
            text = val.strip()
            if not text:
                continue
            # Require center-hit to be true
            if tag.get("data-bbox-center-hit") != 'true':
                continue
            # Extract bbox from this tag if present
            try:
                x = float(tag.get("data-bbox-x"))
                y = float(tag.get("data-bbox-y"))
                w = float(tag.get("data-bbox-width"))
                h = float(tag.get("data-bbox-height"))
                bbox = {"x": x, "y": y, "width": w, "height": h}
            except (TypeError, ValueError):
                # Skip items without bbox
                continue
            if text not in seen:
                items.append((text, bbox))
                seen.add(text)
        return items

    def _extract_aria_with_bbox(self, snapshot_html: str) -> List[Tuple[str, Dict[str, float]]]:
        """Return list of (aria-label, bbox) for elements that have aria-label and bbox."""
        soup = BeautifulSoup(snapshot_html, "html5lib")
        out: List[Tuple[str, Dict[str, float]]] = []
        seen = set()
        for tag in soup.find_all(attrs={"aria-label": True}):
            val = tag.get("aria-label")
            if not isinstance(val, str):
                continue
            label = val.strip()
            if not label:
                continue
            if tag.get("data-bbox-center-hit") != 'true':
                continue
            try:
                x = float(tag.get("data-bbox-x"))
                y = float(tag.get("data-bbox-y"))
                w = float(tag.get("data-bbox-width"))
                h = float(tag.get("data-bbox-height"))
                bbox = {"x": x, "y": y, "width": w, "height": h}
            except (TypeError, ValueError):
                continue
            key = (label, x, y, w, h)
            if key in seen:
                continue
            out.append((label, bbox))
            seen.add(key)
        return out

    def _extract_actions(self, snapshot_html: str) -> List[Dict[str, str]]:
        """Extract pre-defined action pairs from any element's data-actions JSON."""
        soup = BeautifulSoup(snapshot_html, "html5lib")
        out: List[Dict[str, str]] = []
        seen: set[tuple[str, str]] = set()
        for el in soup.find_all(attrs={"data-actions": True}):
            val = el.get("data-actions")
            if not isinstance(val, str) or not val.strip():
                continue
            try:
                data = json.loads(val)
            except Exception:
                continue
            if not isinstance(data, list):
                continue
            for item in data:
                if not isinstance(item, dict):
                    continue
                u = item.get("user")
                a = item.get("assistant")
                if isinstance(u, str) and isinstance(a, str) and u and a:
                    key = (u, a)
                    if key in seen:
                        continue
                    seen.add(key)
                    out.append({"user": u, "assistant": a})
        return out

    def _extract_text_with_bbox(self, snapshot_html: str, min_len: int = 3, max_len: int = 80) -> List[Tuple[str, Dict[str, float]]]:
        """Return list of (text_content, bbox) for elements with textual content and bbox."""
        soup = BeautifulSoup(snapshot_html, "html5lib")
        out: List[Tuple[str, Dict[str, float]]] = []
        seen = set()
        # Iterate elements (not raw NavigableStrings) to access attributes
        for el in soup.find_all(True):
            # Only consider elements with no element children (leaf nodes)
            if el.find(True, recursive=False) is not None:
                continue
            txt = el.get_text(strip=True)
            if not txt:
                continue
            # Normalize whitespace
            txt = " ".join(txt.split())
            if len(txt) < min_len or len(txt) > max_len:
                continue
            # Find bbox on this element or its ancestor
            cur = el
            bbox = None
            while cur is not None:
                try:
                    x = float(cur.get("data-bbox-x"))
                    y = float(cur.get("data-bbox-y"))
                    w = float(cur.get("data-bbox-width"))
                    h = float(cur.get("data-bbox-height"))
                    bbox = {"x": x, "y": y, "width": w, "height": h}
                    # Require that the center hits the element bearing the bbox
                    if cur.get("data-bbox-center-hit") != 'true':
                        bbox = None
                    break
                except (TypeError, ValueError):
                    cur = cur.parent if hasattr(cur, "parent") else None
            if not bbox:
                continue
            key = (txt, bbox["x"], bbox["y"], bbox["width"], bbox["height"])
            if key in seen:
                continue
            out.append((txt, bbox))
            seen.add(key)
        return out

    def _center_from_bbox(self, bbox: Dict[str, float]) -> Tuple[float, float]:
        return bbox["x"] + bbox["width"] / 2.0, bbox["y"] + bbox["height"] / 2.0

    def _annotate_image(self, shot_path: Path, snapshot_html: str) -> Path:
        """Create an annotated copy of the screenshot with instruction bboxes and labels."""
        try:
            from PIL import Image as _PIL_Image  # type: ignore
            from PIL import ImageDraw as _PIL_Draw  # type: ignore
            from PIL import ImageFont as _PIL_Font  # type: ignore
        except Exception:
            # If PIL not available, return original image as a fallback
            return shot_path

        items = self._extract_instructions_with_bbox(snapshot_html)
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
        for text, b in items:
            try:
                x = int(round(b.get("x", 0)))
                y = int(round(b.get("y", 0)))
                w = int(round(b.get("width", 0)))
                h = int(round(b.get("height", 0)))
            except Exception:
                continue
            if w <= 0 or h <= 0:
                continue
            x2 = x + w
            y2 = y + h
            color = (220, 30, 30)
            draw.rectangle((x, y, x2, y2), outline=color, width=3)
            label = str(text)
            if label:
                pad = 2
                # Measure text size if possible
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

        out_path = shot_path.with_name(shot_path.stem + "_annotated" + shot_path.suffix)
        try:
            im.save(out_path)
            return out_path
        except Exception:
            return shot_path

    def _build_augmented_texts(
        self,
        instr_items: List[Tuple[str, Dict[str, float]]],
        aria_items: List[Tuple[str, Dict[str, float]]],
        text_items: List[Tuple[str, Dict[str, float]]],
        shot_path: Path,
        n: int = 5,
        action_items: List[Dict[str, str]] | None = None,
    ) -> List[Dict[str, str]]:
        """Build texts following action augmentations mapping."""
        verbs_to = [("Double-click to", "double_click"), ("Right-click to", "right_click"), ("Click to", "click"), ("Move to", "move_mouse")]
        verbs_the = [("Double-click the", "double_click"), ("Right-click the", "right_click"), ("Click the", "click"), ("Move to the", "move_mouse")]
        verbs_at = [("Double-click the text", "double_click"), ("Right-click the text", "right_click"), ("Click the text", "click"), ("Move to the text", "move_mouse")]
        out: List[Dict[str, str]] = []

        # Compute image size once
        from PIL import Image  # type: ignore
        with Image.open(shot_path) as im:
            W, H = im.size
            px = im.load()

            def _shrink_bbox_to_content(b: Dict[str, float]) -> Dict[str, float] | None:
                # Convert to integer box within image bounds
                x = max(0, min(int(round(b.get("x", 0))), W - 1))
                y = max(0, min(int(round(b.get("y", 0))), H - 1))
                w = max(1, int(round(b.get("width", 0))))
                h = max(1, int(round(b.get("height", 0))))
                if x + w > W:
                    w = max(1, W - x)
                if y + h > H:
                    h = max(1, H - y)
                if w < 2 or h < 2:
                    return None

                # Helper comparators
                def col_equal(c0: int, c1: int) -> bool:
                    for yy in range(y, y + h):
                        if px[c0, yy] != px[c1, yy]:
                            return False
                    return True

                def row_equal(r0: int, r1: int) -> bool:
                    for xx in range(x, x + w):
                        if px[xx, r0] != px[xx, r1]:
                            return False
                    return True

                # Left
                while w > 2 and x + 1 < W and col_equal(x, x + 1):
                    x += 1
                    w -= 1
                # Top
                while h > 2 and y + 1 < H and row_equal(y, y + 1):
                    y += 1
                    h -= 1
                # Right
                while w > 2 and x + w - 1 < W and x + w - 2 >= 0 and col_equal(x + w - 1, x + w - 2):
                    w -= 1
                # Bottom
                while h > 2 and y + h - 1 < H and y + h - 2 >= 0 and row_equal(y + h - 1, y + h - 2):
                    h -= 1

                if w < 2 or h < 2:
                    return None
                return {"x": float(x), "y": float(y), "width": float(w), "height": float(h)}

        # Prepare filtered pools: only keep items with normalized centers strictly inside (0,1)
        def to_pool(items: List[Tuple[str, Dict[str, float]]], *, shrink: bool = False) -> List[Tuple[str, float, float]]:
            pool: List[Tuple[str, float, float]] = []
            for ref, bbox in items:
                if bbox.get("x", 0) <= 0.0 or bbox.get("y", 0) <= 0.0 or bbox.get("width", 0) >= W or bbox.get("height", 0) >= H:
                    continue
                    
                if shrink:
                    sb = _shrink_bbox_to_content(bbox)
                    if not sb:
                        continue
                    cx, cy = self._center_from_bbox(sb)
                else:
                    cx, cy = self._center_from_bbox(bbox)
                nx = round(min(max(cx / W, 0.0), 1.0), 4)
                ny = round(min(max(cy / H, 0.0), 1.0), 4)
                if 0.0 < nx < 1.0 and 0.0 < ny < 1.0:
                    pool.append((ref, nx, ny))
            return pool

        instr_pool = to_pool(instr_items, shrink=False)
        aria_pool = to_pool(aria_items, shrink=False)
        text_pool = to_pool(text_items, shrink=True)

        # Prepare action pool (pre-defined pairs), normalizing any pixel coords to normalized [0,1]
        action_pool: List[Tuple[str, str]] = []
        if action_items:
            import re as _re
            click_xy = _re.compile(r"x=([0-9.]+)\s*,\s*y=([0-9.]+)")
            drag_xy = _re.compile(r"drag\(.*?from_coord=\[\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\]\s*,\s*to_coord=\[\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\].*?\)")
            def _norm_val(v: float, denom: float) -> float:
                # If v > 1, treat as pixels; else assume already normalized
                if v > 1.0:
                    return round(min(max(v / denom, 0.0), 1.0), 4)
                return round(min(max(v, 0.0), 1.0), 4)
            for item in action_items:
                u = item.get("user") if isinstance(item, dict) else None
                a = item.get("assistant") if isinstance(item, dict) else None
                if not (isinstance(u, str) and isinstance(a, str) and u and a):
                    continue
                # Normalize click x=, y=
                m = click_xy.search(a)
                if m:
                    x = float(m.group(1)); y = float(m.group(2))
                    nx = _norm_val(x, W); ny = _norm_val(y, H)
                    if not (0.0 < nx < 1.0 and 0.0 < ny < 1.0):
                        continue
                    a = click_xy.sub(f"x={nx}, y={ny}", a)
                    action_pool.append((u, a))
                    continue
                # Normalize drag from/to coords
                m = drag_xy.search(a)
                if m:
                    fx = float(m.group(1)); fy = float(m.group(2)); tx = float(m.group(3)); ty = float(m.group(4))
                    nfx = _norm_val(fx, W); nfy = _norm_val(fy, H)
                    ntx = _norm_val(tx, W); nty = _norm_val(ty, H)
                    # Require both endpoints inside (0,1)
                    if not (0.0 < nfx < 1.0 and 0.0 < nfy < 1.0 and 0.0 < ntx < 1.0 and 0.0 < nty < 1.0):
                        continue
                    a = drag_xy.sub(
                        f"drag(from_coord=[{nfx},{nfy}], to_coord=[{ntx},{nty}])",
                        a,
                    )
                    action_pool.append((u, a))
                    continue
                # If no coords found, keep as-is
                action_pool.append((u, a))

        attempts = 0
        max_attempts = max(n * 10, 50)
        while len(out) < n and attempts < max_attempts:
            attempts += 1
            # Include 'action' pool if available
            choices = ["to", "the", "at"] + (["action"] if action_pool else [])
            pool_choice = random.choice(choices)  # equal probability among available pools
            if pool_choice == "to":
                if not instr_pool:
                    continue
                ref, nx, ny = random.choice(instr_pool)
                verb_text, act = random.choice(verbs_to)
            elif pool_choice == "the":
                if not aria_pool:
                    continue
                ref, nx, ny = random.choice(aria_pool)
                verb_text, act = random.choice(verbs_the)
            elif pool_choice == "at":
                if not text_pool:
                    continue
                ref, nx, ny = random.choice(text_pool)
                verb_text, act = random.choice(verbs_at)
            else:
                # Pre-defined action pair; use as-is
                if not action_pool:
                    continue
                u, a = random.choice(action_pool)
                out.append({"assistant": a, "user": u})
                continue

            user = f"{verb_text} '{ref}'" if ref else verb_text
            assistant = f"{act}(x={nx}, y={ny})"
            out.append({"assistant": assistant, "user": user})

        # Deduplicate by hash(user + assistant) while preserving order
        seen_keys = set()
        unique: List[Dict[str, str]] = []
        for pair in out:
            u = str(pair.get("user", ""))
            a = str(pair.get("assistant", ""))
            key = u + "\u241F" + a  # use an unlikely separator
            if key in seen_keys:
                continue
            seen_keys.add(key)
            unique.append(pair)
        return unique
