"""Synthetic full-page screenshot + accessibility-tree rendering.

This module renders one *whole-page* screenshot per CuaTask (a handful of
elements laid out on a single canvas) -- at the scale a benchmark dataset
needs (thousands of pages, not millions of crops), plain PIL on CPU renders
this fast enough that GPU-batching would add complexity without a measurable
speed win, and it never touches a GPU so it needs no GPU lock and can run
fully concurrently with any model's training/eval job.

Renderer contracts (see tests/test_render.py). This module is deliberately
structured so a real, previously-observed failure class -- a live
GPU-rendered screenshot silently going stale relative to real element
positions after a reflow -- is structurally impossible here:

- **Layout is a pure function of row INDEX, never of content length.**
  Every row's vertical position is `TITLE_H + MARGIN + i * ROW_H` -- fixed
  height per row, computed before any text is drawn. A field's value (even a
  much-longer-than-typical one) can never push rows below it up or down,
  because nothing about a row's geometry is derived from another row's
  content. This is an intentional, documented simplification versus a real
  browser (where a long value can genuinely reflow a page): it holds because
  (a) the task generator -- not attacker-controlled input -- controls value
  length and can keep it bounded, and (b) this schema has no multi-line
  field (no "Edit" role wraps text across lines); every value is drawn on
  the single fixed-height line its row already reserves. If a future task
  family adds a real multi-line/textarea role, this fixed-row-height
  assumption must be revisited for that role specifically -- don't silently
  extend it.
- **Frames are computed once, from the same geometry that is drawn, and
  returned verbatim** -- `render_page` never draws from one set of
  coordinates and reports another. There is no separate "detector" pass and
  no caching of stale coordinates across calls.
- **Long text is truncated (with an ellipsis) to fit inside the box/frame it
  is drawn in**, rather than left to silently overflow past its own frame's
  right edge or, for full-width rows, off the canvas. This keeps the "frame
  bbox bounds the visible content" contract exact even for pathological
  (very long) values -- see the reflow/long-value tests.
- **Rendering is fully deterministic**: no randomness, no wall-clock/host
  state, no dict/set iteration whose order isn't already list-order. Same
  `(title, rows)` in -> byte-identical PNG and identical `elements` out,
  every call, every process.
"""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

PAGE_W = 760
MARGIN = 24
ROW_H = 56
TITLE_H = 48
LABEL_DY = 4
BOX_H = 32
CHECKBOX_SIZE = 18
BUTTON_W, BUTTON_H = 160, 36

_FONT_CANDIDATES = ("C:/Windows/Fonts/segoeui.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
_FONT_BOLD_CANDIDATES = ("C:/Windows/Fonts/segoeuib.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf")


def _font(candidates: tuple[str, ...], size: int) -> ImageFont.ImageFont:
    for p in candidates:
        if Path(p).exists():
            return ImageFont.truetype(p, size)
    return ImageFont.load_default()


FONT_TITLE = _font(_FONT_BOLD_CANDIDATES, 20)
FONT_LABEL = _font(_FONT_CANDIDATES, 14)
FONT_VALUE = _font(_FONT_CANDIDATES, 14)

ELLIPSIS = "…"


def _fit_text(draw: ImageDraw.ImageDraw, text: str, font: ImageFont.ImageFont, max_width: float) -> str:
    """Truncate `text` (adding an ellipsis) so it renders within `max_width`
    px, so a very long value/label can never visually overflow the frame or
    row it was drawn in -- see the module docstring's "long text is
    truncated" contract. No-op if it already fits or max_width is too small
    to hold even the ellipsis."""
    if max_width <= 0 or draw.textlength(text, font=font) <= max_width:
        return text
    if draw.textlength(ELLIPSIS, font=font) > max_width:
        return ""
    lo, hi = 0, len(text)
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if draw.textlength(text[:mid] + ELLIPSIS, font=font) <= max_width:
            lo = mid
        else:
            hi = mid - 1
    return text[:lo] + ELLIPSIS


def render_page(title: str, rows: list[dict]) -> tuple[Image.Image, list[dict]]:
    """rows: [{"id","role","label","value","checked"}, ...] in display order.
    Returns (PIL image, elements) where elements carries pixel `frame` boxes
    (x1,y1,x2,y2) matching the rendered layout -- these become CuaTask.elements
    and are what a real visual element detector would also need to recover
    for a real screenshot, so keeping this layout simple/deterministic is
    deliberate.
    """
    height = TITLE_H + MARGIN * 2 + len(rows) * ROW_H
    img = Image.new("RGB", (PAGE_W, height), "white")
    draw = ImageDraw.Draw(img)
    draw.rectangle((0, 0, PAGE_W, TITLE_H), fill=(245, 245, 248))
    draw.text((MARGIN, 12), title, font=FONT_TITLE, fill=(20, 20, 20))

    elements = []
    y = TITLE_H + MARGIN
    for row in rows:
        role, label = row["role"], row["label"]
        if role == "Edit" or role == "Select":
            label_fit = _fit_text(draw, label, FONT_LABEL, PAGE_W - 2 * MARGIN)
            draw.text((MARGIN, y), label_fit, font=FONT_LABEL, fill=(60, 60, 60))
            box = (MARGIN, y + LABEL_DY + 18, PAGE_W - MARGIN, y + LABEL_DY + 18 + BOX_H)
            draw.rectangle(box, outline=(120, 120, 120), width=1)
            value = row.get("value", "")
            if role == "Select":
                value = value or "Select..."
            value_fit = _fit_text(draw, value, FONT_VALUE, (box[2] - box[0]) - 16)
            draw.text((box[0] + 8, box[1] + 7), value_fit, font=FONT_VALUE, fill=(10, 10, 10))
            frame = box
        elif role == "CheckBox":
            box = (MARGIN, y + (ROW_H - CHECKBOX_SIZE) // 2, MARGIN + CHECKBOX_SIZE, y + (ROW_H - CHECKBOX_SIZE) // 2 + CHECKBOX_SIZE)
            fill = (0, 103, 192) if row.get("checked") else None
            draw.rectangle(box, outline=(64, 64, 64), width=2, fill=fill)
            if row.get("checked"):
                draw.line((box[0] + 3, box[1] + 9, box[0] + 7, box[3] - 3), fill="white", width=2)
                draw.line((box[0] + 7, box[3] - 3, box[2] - 2, box[1] + 2), fill="white", width=2)
            label_fit = _fit_text(draw, label, FONT_LABEL, (PAGE_W - MARGIN) - (box[2] + 8))
            draw.text((box[2] + 8, y + (ROW_H - 16) // 2), label_fit, font=FONT_LABEL, fill=(30, 30, 30))
            frame = (box[0], box[1], PAGE_W - MARGIN, box[3])
        elif role == "Button":
            x1 = MARGIN
            box = (x1, y + (ROW_H - BUTTON_H) // 2, x1 + BUTTON_W, y + (ROW_H - BUTTON_H) // 2 + BUTTON_H)
            draw.rectangle(box, outline=(90, 90, 90), width=1, fill=(230, 230, 232))
            label_fit = _fit_text(draw, label, FONT_LABEL, BUTTON_W - 16)
            tw = draw.textlength(label_fit, font=FONT_LABEL)
            draw.text((box[0] + (BUTTON_W - tw) / 2, box[1] + 9), label_fit, font=FONT_LABEL, fill=(10, 10, 10))
            frame = box
        else:
            frame = (MARGIN, y, PAGE_W - MARGIN, y + ROW_H)
        elements.append({"id": row["id"], "role": role, "label": label, "frame": list(frame)})
        y += ROW_H
    return img, elements


def render_ax_tree(title: str, rows: list[dict]) -> str:
    """A markdown-style synthetic accessibility tree -- same information a real
    ax-tree capture would carry (role, name, value/checked state)."""
    lines = [f"# {title}", ""]
    for row in rows:
        role, label = row["role"], row["label"]
        if role in ("Edit", "Select"):
            lines.append(f"- [{row['id']}] {role} \"{label}\" value=\"{row.get('value', '')}\"")
        elif role == "CheckBox":
            lines.append(f"- [{row['id']}] {role} \"{label}\" checked={row.get('checked', False)}")
        else:
            lines.append(f"- [{row['id']}] {role} \"{label}\"")
    return "\n".join(lines)
