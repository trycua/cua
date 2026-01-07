"""Dataset command - Manage datasets and build from outputs.

Usage:
    cb dataset list               # List available datasets from registry
    cb dataset build <outputs>    # Build a dataset from batch outputs
"""
from __future__ import annotations

import html as _html
import json
import re
import subprocess
import textwrap as _textwrap
from pathlib import Path
from typing import Any, Dict, List, Tuple

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def register_parser(subparsers):
    """Register the dataset command parser."""
    dataset_parser = subparsers.add_parser(
        'dataset',
        help='Manage datasets and build from outputs'
    )
    dataset_subparsers = dataset_parser.add_subparsers(dest='dataset_command')

    # cb dataset list
    list_parser = dataset_subparsers.add_parser(
        'list',
        help='List available datasets from registry'
    )

    # cb dataset build <outputs>
    build_parser = dataset_subparsers.add_parser(
        'build',
        help='Build a dataset from batch outputs'
    )
    build_parser.add_argument(
        'outputs_path',
        help='Path to outputs folder from batch dump (e.g., ./outputs or /tmp/td_output)'
    )
    build_parser.add_argument(
        'max_samples',
        nargs='?',
        type=int,
        help='Limit number of samples processed (useful for testing)'
    )
    build_parser.add_argument(
        '--mode',
        default='aguvis-stage-1',
        help="Processing mode: 'aguvis-stage-1' (action augmentation) or 'gui-r1' (low-level click). Default: 'aguvis-stage-1'"
    )
    build_parser.add_argument(
        '--dataset-name',
        help='Dataset name when saving to disk (default: td_<mode>_dataset)'
    )
    build_parser.add_argument(
        '--save-dir',
        help='Directory to save a JSONL dataset (default: <outputs>/processed if not pushing)'
    )
    build_parser.add_argument(
        '--push-to-hub',
        action='store_true',
        help='Push the dataset to the Hugging Face Hub'
    )
    build_parser.add_argument(
        '--repo-id',
        help='HF Hub repository ID (e.g., username/repo). Required with --push-to-hub.'
    )
    build_parser.add_argument(
        '--private',
        action='store_true',
        help='When pushing to hub, create/update the repo as private'
    )


def execute(args):
    """Execute the dataset command."""
    dataset_command = getattr(args, 'dataset_command', None)

    if dataset_command == 'list':
        return cmd_list(args)
    elif dataset_command == 'build':
        return cmd_build(args)
    else:
        print(f"{YELLOW}Usage: cb dataset <command> [args]{RESET}")
        print(f"\n{GREY}Commands:{RESET}")
        print(f"  list              List available datasets from registry")
        print(f"  build <outputs>   Build a dataset from batch outputs")
        return 1


# ============================================================================
# cmd_list - List available datasets from registry
# ============================================================================

def cmd_list(args) -> int:
    """List available datasets from the registry."""
    from .registry import ensure_registry

    try:
        registry_path = ensure_registry(update=True, verbose=True)
    except RuntimeError as e:
        print(f"{RED}Error: {e}{RESET}")
        return 1

    datasets_path = registry_path / 'datasets'
    if not datasets_path.exists():
        print(f"{RED}Error: Datasets directory not found in registry{RESET}")
        return 1

    datasets = [d.name for d in datasets_path.iterdir() if d.is_dir() and not d.name.startswith('.')]

    if not datasets:
        print(f"{YELLOW}No datasets found in registry{RESET}")
        return 0

    datasets.sort()

    print(f"\n{BOLD}Available Datasets:{RESET}")
    print(f"{GREY}Registry: {registry_path}{RESET}\n")

    for dataset in datasets:
        print(f"  • {CYAN}{dataset}{RESET}")

    print(f"\n{GREY}Use with: {RESET}cb run dataset <dataset-name>")
    print(f"{GREY}       or: {RESET}cb interact <task-name> --dataset <dataset-name>")

    return 0


# ============================================================================
# cmd_build - Build a dataset from batch outputs
# ============================================================================

def cmd_build(args) -> int:
    """Build a dataset from batch dump outputs."""
    from cua_bench.processors import get_processor
    from cua_bench.processors.base import ProcessorArgs

    pargs = ProcessorArgs(
        outputs_path=Path(args.outputs_path).expanduser().resolve(),
        dataset_name=args.dataset_name,
        save_dir=Path(args.save_dir).expanduser().resolve() if args.save_dir else None,
        push_to_hub=bool(args.push_to_hub),
        repo_id=args.repo_id,
        private=bool(args.private),
        max_samples=args.max_samples,
    )

    if not pargs.outputs_path.exists():
        print(f"{RED}Outputs path not found: {pargs.outputs_path}{RESET}")
        return 1

    processor_name = args.mode or "aguvis-stage-1"
    try:
        ProcessorClass = get_processor(processor_name)
    except ValueError as e:
        print(f"{RED}{e}{RESET}")
        return 1

    processor = ProcessorClass(pargs)

    print(f"{CYAN}Processing with {BOLD}{processor_name}{RESET}{CYAN} processor...{RESET}")
    rows = processor.process()
    print(f"{GREEN}✓ Processed {len(rows)} rows{RESET}")

    ds_name = pargs.dataset_name or processor.get_dataset_name()

    if pargs.save_dir:
        if pargs.dataset_name:
            out = processor.save_to_disk(rows, pargs.save_dir, ds_name)
            print(f"{GREEN}✓ Saved dataset to disk:{RESET} {out}")
        else:
            out = processor.save_jsonl(rows, pargs.save_dir, ds_name)
            print(f"{GREEN}✓ Saved JSONL dataset to:{RESET} {out}")
        prev = _write_previews(rows, pargs.save_dir, processor_name)
        print(f"{GREEN}✓ Saved HTML preview to:{RESET} {prev}")
        gif = _write_preview_gif(rows, pargs.save_dir, limit=50)
        print(f"{GREEN}✓ Saved GIF preview to:{RESET} {gif}")

    if pargs.push_to_hub:
        if not pargs.repo_id:
            print(f"{RED}--repo-id is required when --push-to-hub is set (e.g., username/repo){RESET}")
            return 1
        processor.push_to_hub(rows, pargs.repo_id, pargs.private)
        print(f"{GREEN}✓ Pushed dataset to hub:{RESET} {pargs.repo_id} {GREY}(private={pargs.private}){RESET}")

    if not pargs.save_dir and not pargs.push_to_hub:
        default_dir = pargs.outputs_path / "processed"
        if pargs.dataset_name:
            out = processor.save_to_disk(rows, default_dir, ds_name)
            print(f"{GREEN}✓ Saved dataset to disk:{RESET} {out}")
        else:
            out = processor.save_jsonl(rows, default_dir, ds_name)
            print(f"{GREEN}✓ Saved JSONL dataset to:{RESET} {out}")
        prev = _write_previews(rows, default_dir, processor_name)
        print(f"{GREEN}✓ Saved HTML preview to:{RESET} {prev}")
        gif = _write_preview_gif(rows, default_dir, limit=50)
        print(f"{GREEN}✓ Saved GIF preview to:{RESET} {gif}")

    return 0


def _write_previews(rows: List[Dict[str, Any]], base_dir: Path, processor_name: str, limit: int = 50) -> Path:
    """Write HTML preview of the dataset."""
    preview_dir = base_dir / "previews"
    preview_dir.mkdir(parents=True, exist_ok=True)
    index_path = preview_dir / "index.html"

    xy_re = re.compile(r"x=([0-9.]+)\s*,\s*y=([0-9.]+)")
    drag_re = re.compile(r"drag\(.*?from_coord=\[\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\]\s*,\s*to_coord=\[\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\].*?\)")

    parts: List[str] = []
    parts.append("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>cua-bench Previews</title>")
    parts.append(
        "<style>\n"
        ".item{{margin:16px 0;padding:12px;border:1px solid #ddd;border-radius:8px;}}\n"
        ".frame{{position:relative;display:inline-block;}}\n"
        ".frame img{{display:block;max-width:100%;height:auto;}}\n"
        ".cross{{position:absolute;width:24px;height:24px;margin-left:-12px;margin-top:-12px;pointer-events:none;}}\n"
        ".cross:before,.cross:after{{content:'';position:absolute;background:red;}}\n"
        ".cross:before{{left:50%;top:0;bottom:0;width:2px;transform:translateX(-50%);}}\n"
        ".cross:after{{top:50%;left:0;right:0;height:2px;transform:translateY(-50%);}}\n"
        ".cross.half-left:after{{left:0;right:50%;}}\n"
        ".cross.half-right:after{{left:50%;right:0;}}\n"
        ".cross .num{{position:absolute;top:-16px;left:10px;background:red;color:#fff;font:600 12px/12px ui-monospace,Menlo,monospace;padding:2px 4px;border-radius:10px;}}\n"
        "pre{{background:#f7f7f7;padding:8px;border-radius:6px;overflow:auto;white-space:pre-wrap;}}\n"
        "</style></head><body>\n<h1>cua-bench Previews - {0}</h1>\n".format(_html.escape(processor_name))
    )

    for i, r in enumerate(rows[:limit]):
        img_list = r.get("images", [])
        if not img_list:
            continue
        img_obj = img_list[0]
        if hasattr(img_obj, 'filename') and img_obj.filename:
            img_path = img_obj.filename
        else:
            continue

        try:
            from PIL import Image as _PIL
            with _PIL.open(img_path) as _im:
                _W, _H = _im.size
        except Exception:
            _W, _H = (1, 1)

        markers = []

        if "texts" in r:
            texts = r.get("texts", [])
            for idx, pair in enumerate(texts, start=1):
                user = str(pair.get("user", ""))
                assistant = str(pair.get("assistant", ""))
                md = drag_re.search(assistant)
                if md:
                    fx = float(md.group(1)); fy = float(md.group(2)); tx = float(md.group(3)); ty = float(md.group(4))
                    def _n(v, d):
                        return v / d if v > 1.0 else v
                    nfx, nfy = _n(fx, _W), _n(fy, _H)
                    ntx, nty = _n(tx, _W), _n(ty, _H)
                    nfx = min(max(nfx, 0.0), 1.0); nfy = min(max(nfy, 0.0), 1.0)
                    ntx = min(max(ntx, 0.0), 1.0); nty = min(max(nty, 0.0), 1.0)
                    markers.append((idx, nfx, nfy, user, assistant, "half-left"))
                    markers.append((idx, ntx, nty, user, assistant, "half-right"))
                else:
                    m = xy_re.search(assistant)
                    if m:
                        x = float(m.group(1)); y = float(m.group(2))
                        x = x / _W if x > 1.0 else x
                        y = y / _H if y > 1.0 else y
                    else:
                        continue
                    x = min(max(x, 0.0), 1.0); y = min(max(y, 0.0), 1.0)
                    markers.append((idx, x, y, user, assistant, ""))

        elif "gt_bbox" in r:
            gt_bbox = r.get("gt_bbox", [])
            instruction = r.get("instruction", "")
            if len(gt_bbox) == 4:
                x1, y1, x2, y2 = gt_bbox
                cx = (x1 + x2) / 2.0
                cy = (y1 + y2) / 2.0
                markers.append((1, cx, cy, instruction, "click", ""))

        parts.append('<div class="item">')
        parts.append('<div class="frame">')
        parts.append(f'<img src="file://{_html.escape(img_path)}" alt="preview_{i}">')
        for idx, x, y, user, assistant, cls in markers:
            cls_attr = f" cross {cls}" if cls else " cross"
            parts.append(f'<div class="{cls_attr}" style="left:{x*100:.2f}%; top:{y*100:.2f}%;"><div class="num">{idx}</div></div>')
        parts.append('</div>')
        parts.append('<pre>')

        if "texts" in r:
            for idx, _, _, user, assistant, cls in markers:
                if cls == "half-left":
                    continue
                parts.append(f'user: {_html.escape(user)}')
                parts.append(f'assistant: {_html.escape(assistant)} [{idx}]')
                if idx != len(markers):
                    parts.append('')
        elif "gt_bbox" in r:
            parts.append(f'instruction: {_html.escape(r.get("instruction", ""))}')
            parts.append(f'gt_bbox: {r.get("gt_bbox", [])}')
            parts.append(f'gt_action: {_html.escape(r.get("gt_action", ""))}')
            parts.append(f'task_type: {_html.escape(r.get("task_type", ""))}')
            parts.append(f'os_type: {_html.escape(r.get("os_type", ""))}')
            parts.append(f'resolution: {_html.escape(r.get("resolution", ""))}')

        parts.append('</pre>')
        parts.append('</div>')

    parts.append("</body></html>")
    index_path.write_text("\n".join(parts), encoding="utf-8")
    return index_path


def _write_preview_gif(
    rows: List[Dict[str, Any]],
    base_dir: Path,
    *,
    limit: int = 50,
    fps: int = 1,
    size: Tuple[int, int] = (640, 360),
) -> Path:
    """Create a single GIF preview of the first N tasks."""
    from PIL import Image as _PIL_Image
    from PIL import ImageDraw as _PIL_Draw
    from PIL import ImageFont as _PIL_Font

    preview_dir = base_dir / "previews"
    preview_dir.mkdir(parents=True, exist_ok=True)
    gif_path = preview_dir / "preview.gif"

    W, H = size
    sidebar_w = 200
    content_w = W - sidebar_w
    content_h = H
    box_w = content_w
    box_h = content_h
    box_x = 0
    box_y = 0

    xy_re = re.compile(r"x=([0-9.]+)\s*,\s*y=([0-9.]+)")
    drag_re = re.compile(r"drag\(.*?from_coord=\[\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\]\s*,\s*to_coord=\[\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\].*?\)")

    frames: List[_PIL_Image.Image] = []
    try:
        font = _PIL_Font.load_default()
    except Exception:
        font = None

    for i, r in enumerate(rows[:limit]):
        img_list = r.get("images", [])
        if not img_list:
            continue
        img_obj = img_list[0]
        if hasattr(img_obj, 'filename') and img_obj.filename:
            img_path = img_obj.filename
        else:
            continue

        frame = _PIL_Image.new("RGB", (W, H), (255, 255, 255))
        draw = _PIL_Draw.Draw(frame)

        try:
            shot = _PIL_Image.open(img_path).convert("RGB")
        except Exception:
            shot = _PIL_Image.new("RGB", (box_w, box_h), (240, 240, 240))
        sW, sH = shot.size
        scale = min(box_w / sW, box_h / sH)
        new_w = max(1, int(round(sW * scale)))
        new_h = max(1, int(round(sH * scale)))
        shot_resized = shot.resize((new_w, new_h), _PIL_Image.BICUBIC)
        paste_x = box_x + (box_w - new_w) // 2
        paste_y = box_y + (box_h - new_h) // 2
        frame.paste(shot_resized, (paste_x, paste_y))

        def norm_to_px(nx: float, ny: float) -> Tuple[int, int]:
            nx = min(max(nx, 0.0), 1.0)
            ny = min(max(ny, 0.0), 1.0)
            return paste_x + int(round(nx * (new_w - 1))), paste_y + int(round(ny * (new_h - 1)))

        markers: List[Tuple[int, float, float, str, str, str]] = []
        _W, _H = (sW, sH)

        if "texts" in r:
            texts = r.get("texts", [])
            for idx, pair in enumerate(texts, start=1):
                user = str(pair.get("user", ""))
                assistant = str(pair.get("assistant", ""))
                md = drag_re.search(assistant)
                if md:
                    fx = float(md.group(1)); fy = float(md.group(2)); tx = float(md.group(3)); ty = float(md.group(4))
                    def _n(v, d):
                        return v / d if v > 1.0 else v
                    nfx, nfy = _n(fx, _W), _n(fy, _H)
                    ntx, nty = _n(tx, _W), _n(ty, _H)
                    nfx = min(max(nfx, 0.0), 1.0); nfy = min(max(nfy, 0.0), 1.0)
                    ntx = min(max(ntx, 0.0), 1.0); nty = min(max(nty, 0.0), 1.0)
                    markers.append((idx, nfx, nfy, user, assistant, "half-left"))
                    markers.append((idx, ntx, nty, user, assistant, "half-right"))
                else:
                    m = xy_re.search(assistant)
                    if m:
                        x = float(m.group(1)); y = float(m.group(2))
                        x = x / _W if x > 1.0 else x
                        y = y / _H if y > 1.0 else y
                    else:
                        continue
                    x = min(max(x, 0.0), 1.0); y = min(max(y, 0.0), 1.0)
                    markers.append((idx, x, y, user, assistant, ""))
        elif "gt_bbox" in r:
            gt_bbox = r.get("gt_bbox", [])
            instruction = r.get("instruction", "")
            if len(gt_bbox) == 4:
                x1, y1, x2, y2 = gt_bbox
                cx = (x1 + x2) / 2.0
                cy = (y1 + y2) / 2.0
                markers.append((1, cx, cy, instruction, "click", ""))

        def draw_cross(px: int, py: int, idx: int, cls: str = "") -> None:
            cross_size = 16
            color = (220, 30, 30)
            draw.line((px, py - cross_size, px, py + cross_size), fill=color, width=2)
            if cls == "half-left":
                draw.line((px - cross_size, py, px, py), fill=color, width=2)
            elif cls == "half-right":
                draw.line((px, py, px + cross_size, py), fill=color, width=2)
            else:
                draw.line((px - cross_size, py, px + cross_size, py), fill=color, width=2)
            badge_text = str(idx)
            bx, by = px + 8, py - 20
            draw.rectangle((bx - 2, by - 10, bx + 12, by + 4), fill=color)
            if font:
                draw.text((bx, by - 8), badge_text, fill=(255, 255, 255), font=font)

        for idx, nx, ny, user, assistant, cls in markers:
            px, py = norm_to_px(nx, ny)
            draw_cross(px, py, idx, cls)

        sidebar_x0 = content_w
        draw.rectangle((sidebar_x0, 0, W, H), fill=(245, 245, 245))
        pad = 12
        tx = sidebar_x0 + pad
        ty = pad
        if font:
            draw.text((tx, ty), f"Task {i+1}", fill=(0, 0, 0), font=font)
        ty += 18

        listed = set()
        for idx, _, _, user, assistant, cls in markers:
            if cls == "half-left":
                continue
            if idx in listed:
                continue
            listed.add(idx)
            u_lines = _textwrap.wrap(f"user: {user}", width=46)
            a_lines = _textwrap.wrap(f"assistant: {assistant} [{idx}]", width=46)
            for line in u_lines:
                if font:
                    draw.text((tx, ty), line, fill=(20, 20, 20), font=font)
                ty += 14
            for line in a_lines:
                if font:
                    draw.text((tx, ty), line, fill=(20, 20, 20), font=font)
                ty += 16
            ty += 6

        frames.append(frame)

    if not frames:
        return gif_path

    duration_ms = int(round(1000 / max(1, fps)))
    try:
        frames[0].save(
            gif_path,
            save_all=True,
            append_images=frames[1:],
            duration=duration_ms,
            loop=0,
            optimize=False,
            disposal=2,
        )
    except Exception:
        frames[0].save(
            gif_path,
            save_all=True,
            append_images=frames[1:],
            duration=duration_ms,
            loop=0,
        )
    return gif_path
