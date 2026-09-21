"""Turn `cua_bench_s1.task.CuaTask` rows into the prepared, on-disk training
format `train_nano.py` reads: one `NanoElement`-shaped row per (element,
option-set) example, plus (multimodal only) a precomputed per-crop vision
feature tensor.

This mirrors the two-piece split a frozen-vision architecture needs: cheap
per-batch byte tokenization for options/text-context happens on the fly in
`cua_s1.nano.NanoByteCollator`, but the frozen vision backbone's forward pass
is expensive enough that it should run once per element, not once per epoch.
So for "multimodal":

  - each element is cropped out of its task's screenshot (using the
    element's `frame`; falls back to the whole screenshot if `frame` is
    missing or degenerate),
  - every crop is encoded once via a `cua_s1.nano` vision backbone,
  - `<name>.jsonl` gets a `feature_index` pointer into `<name>.features.pt`
    (an `(N, TOKENS_PER_CROP, FEATURE_DIM)` tensor).

For "text", no backbone pass is needed -- the byte-level context encoder
trains end-to-end from raw bytes -- so this just inlines the element's
rendered accessibility-tree excerpt as `context_text`.

Usage:
    python nano_data.py --tasks data/train.jsonl --modality multimodal \\
        --output data/nano-mm --name train --vision-backbone smolvlm
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import torch
from torch.utils.data import Dataset

try:
    from PIL import Image
except ImportError:  # pragma: no cover - only needed for the multimodal path
    Image = None  # type: ignore[assignment]

from cua_bench_s1.task import CuaTask, load_jsonl


@dataclass(frozen=True)
class PreparedExample:
    """One (element, option-set) row exploded out of a `CuaTask`."""

    element_id: str
    option_texts: tuple[str, ...]
    label: int
    action: str
    context_text: str | None = None
    screenshot: str | None = None
    frame: tuple[float, float, float, float] | None = None


def _render_option_text(option) -> str:
    """Render one `OptionSpec` as the flat text `NanoOptionEncoder` scores."""
    if option.entity_id:
        return f"{option.action}:{option.role}:{option.label}:{option.entity_id}"
    return f"{option.action}:{option.role}:{option.label}"


def _render_context_text(task: CuaTask, element_id: str) -> str:
    """Render a per-element accessibility-tree excerpt for the text modality.

    Falls back to the task's whole `ax_tree` if per-element slicing isn't
    possible; either way this is cheap, on-the-fly-tokenizable text, never a
    substitute for the (expensive) vision path.
    """
    element = next((e for e in task.elements if e.get("id") == element_id), None)
    header = f"# {task.app} / {task.family}\n"
    if element is not None:
        header += f"target: {element.get('role')} {element.get('label')!r}\n"
    return header + (task.ax_tree or "")


def explode_task(task: CuaTask, modality: str) -> list[PreparedExample]:
    """One example per element that has at least one candidate option.

    An element's label is the index, within its own options, of the option
    matching `task.expected[element_id]` -- the same closed-option-set
    classification shape `NanoScorer.forward` expects.
    """
    if modality not in ("text", "multimodal"):
        raise ValueError(f"unknown modality: {modality!r}")
    if modality == "multimodal" and not task.screenshot:
        raise ValueError(f"task {task.id}: multimodal requested but has no screenshot")
    if modality == "text" and not task.ax_tree:
        raise ValueError(f"task {task.id}: text requested but has no ax_tree")

    by_element: dict[str, list] = {}
    for option in task.options:
        by_element.setdefault(option.element_id, []).append(option)

    frames = {e["id"]: tuple(e["frame"]) for e in task.elements if e.get("frame")}

    examples: list[PreparedExample] = []
    for element_id, options in by_element.items():
        gold_action = task.expected.get(element_id)
        if gold_action is None:
            continue
        gold_index = next((i for i, o in enumerate(options) if o.action == gold_action), None)
        if gold_index is None:
            continue
        option_texts = tuple(_render_option_text(o) for o in options)
        if modality == "multimodal":
            examples.append(
                PreparedExample(
                    element_id=element_id,
                    option_texts=option_texts,
                    label=gold_index,
                    action=gold_action,
                    screenshot=task.screenshot,
                    frame=frames.get(element_id),
                )
            )
        else:
            examples.append(
                PreparedExample(
                    element_id=element_id,
                    option_texts=option_texts,
                    label=gold_index,
                    action=gold_action,
                    context_text=_render_context_text(task, element_id),
                )
            )
    return examples


def crop_element_image(screenshot_path: str, frame: tuple[float, float, float, float] | None):
    """Crop one element out of its task's screenshot, falling back to the
    whole image if the frame is missing or degenerate."""
    if Image is None:
        raise RuntimeError("cropping screenshots requires the 'pillow' package")
    image = Image.open(screenshot_path).convert("RGB")
    if frame is None:
        return image
    x, y, w, h = frame
    x0, y0 = max(0, int(x)), max(0, int(y))
    x1, y1 = min(image.width, int(x + w)), min(image.height, int(y + h))
    if x1 <= x0 or y1 <= y0:
        return image
    return image.crop((x0, y0, x1, y1))


def build_split(
    tasks_path: str | Path,
    modality: str,
    output_dir: str | Path,
    name: str,
    vision_backbone: str | None = None,
    encode_batch: int = 256,
) -> dict[str, Any]:
    """Prepare one named split (e.g. "train"/"validation") from a CuaTask
    jsonl file, writing `<output_dir>/<name>.jsonl` (+ `.features.pt` for
    multimodal) in the format `NanoPreparedDataset` reads."""
    output_dir = Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    tasks = load_jsonl(tasks_path)
    examples: list[PreparedExample] = []
    for task in tasks:
        examples.extend(explode_task(task, modality))
    if not examples:
        raise ValueError(f"no {modality} examples found in {tasks_path}")

    rows: list[dict[str, Any]] = []
    if modality == "multimodal":
        from cua_s1.nano import load_vision_backbone

        backbone_name = vision_backbone or "siglip"
        backbone = load_vision_backbone(backbone_name)
        feature_chunks = []
        for start in range(0, len(examples), encode_batch):
            chunk = examples[start : start + encode_batch]
            images = [crop_element_image(ex.screenshot, ex.frame) for ex in chunk]
            feature_chunks.append(backbone.encode(images, batch_size=encode_batch))
        features = torch.cat(feature_chunks, dim=0)
        torch.save(features, output_dir / f"{name}.features.pt")
        for i, ex in enumerate(examples):
            rows.append(
                {
                    "feature_index": i,
                    "element_id": ex.element_id,
                    "option_texts": list(ex.option_texts),
                    "label": ex.label,
                    "meta": {"action": ex.action},
                }
            )
    else:
        for ex in examples:
            rows.append(
                {
                    "context_text": ex.context_text,
                    "element_id": ex.element_id,
                    "option_texts": list(ex.option_texts),
                    "label": ex.label,
                    "meta": {"action": ex.action},
                }
            )

    with (output_dir / f"{name}.jsonl").open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False) + "\n")
    return {"examples": len(rows), "modality": modality}


class NanoPreparedDataset(Dataset):
    """Loads a split written by `build_split`, ready for `NanoPreparedCollator`."""

    def __init__(
        self, jsonl_path: str | Path, modality: str, features_path: str | Path | None = None
    ) -> None:
        if modality not in ("text", "multimodal"):
            raise ValueError(f"unknown modality: {modality!r}")
        self.modality = modality
        self.rows = [
            json.loads(line)
            for line in Path(jsonl_path).read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        if not self.rows:
            raise ValueError(f"no rows in {jsonl_path}")
        self.features = None
        if modality == "multimodal":
            if features_path is None:
                features_path = Path(jsonl_path).with_suffix("").with_suffix(".features.pt")
            self.features = torch.load(features_path, map_location="cpu", weights_only=True)

    def __len__(self) -> int:
        return len(self.rows)

    def __getitem__(self, index: int):
        row = self.rows[index]
        crop = self.features[row["feature_index"]] if self.features is not None else None
        return row["option_texts"], row["label"], row.get("context_text"), crop


def _byte_ids(text: str, length: int) -> list[int]:
    return [byte + 1 for byte in text.encode("utf-8", errors="replace")[:length]]


def _collate_options(option_lists: list[list[str]], option_tokens: int) -> dict[str, torch.Tensor]:
    option_rows = [[_byte_ids(opt, option_tokens) for opt in opts] for opts in option_lists]
    max_options = max(len(row) for row in option_rows)
    max_tokens = max((len(t) for row in option_rows for t in row), default=1) or 1
    batch = len(option_lists)
    option_ids = torch.zeros((batch, max_options, max_tokens), dtype=torch.long)
    option_mask = torch.zeros((batch, max_options), dtype=torch.bool)
    for r, row in enumerate(option_rows):
        option_mask[r, : len(row)] = True
        for c, tokens in enumerate(row):
            option_ids[r, c, : len(tokens)] = torch.tensor(tokens)
    return {
        "option_ids": option_ids,
        "option_token_mask": option_ids.ne(0),
        "option_mask": option_mask,
    }


def _collate_bytes(texts: list[str], length: int) -> tuple[torch.Tensor, torch.Tensor]:
    ids = [_byte_ids(t, length) for t in texts]
    max_len = max((len(t) for t in ids), default=1) or 1
    out = torch.zeros((len(ids), max_len), dtype=torch.long)
    for i, t in enumerate(ids):
        out[i, : len(t)] = torch.tensor(t)
    return out, out.ne(0)


class NanoPreparedCollator:
    """Collate `NanoPreparedDataset` rows into the tensor batches
    `NanoScorer.forward` expects, matching `NanoByteCollator`'s tensor shapes
    for the text modality and adding the multimodal `crop_features` path."""

    def __init__(self, modality: str, option_tokens: int = 96, context_tokens: int = 256) -> None:
        if modality not in ("text", "multimodal"):
            raise ValueError(f"unknown modality: {modality!r}")
        self.modality, self.option_tokens, self.context_tokens = (
            modality,
            option_tokens,
            context_tokens,
        )

    def __call__(self, examples: list[tuple]) -> dict[str, Any]:
        option_lists = [ex[0] for ex in examples]
        labels = [ex[1] for ex in examples]
        batch = _collate_options(option_lists, self.option_tokens)
        batch["labels"] = torch.tensor(labels, dtype=torch.long)
        batch["modality"] = self.modality
        if self.modality == "multimodal":
            crops = torch.stack([ex[3] for ex in examples])
            batch["crop_features"] = crops
            batch["crop_mask"] = torch.ones(crops.shape[:2], dtype=torch.bool)
        else:
            texts = [ex[2] or "" for ex in examples]
            batch["context_ids"], batch["context_token_mask"] = _collate_bytes(
                texts, self.context_tokens
            )
        return batch


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks", type=Path, required=True, help="CuaTask jsonl file")
    parser.add_argument("--modality", choices=["text", "multimodal"], required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--name", default="train")
    parser.add_argument(
        "--vision-backbone",
        choices=["smolvlm", "siglip"],
        default="siglip",
        help="only used for --modality multimodal",
    )
    args = parser.parse_args()
    summary = build_split(args.tasks, args.modality, args.output, args.name, args.vision_backbone)
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
