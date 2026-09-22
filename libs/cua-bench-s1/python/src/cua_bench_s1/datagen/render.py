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
  extend it. The optional source-record panel (`_source_lines`) shifts every
  form row down by one constant offset computed *before* any drawing, from the
  wrapped source-line COUNT alone, so this contract holds with the panel too:
  no row's geometry ever depends on another row's content.
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


SOURCE_LINE_H = 20
SOURCE_PAD = 12


def _source_lines(goal: str | None, entities: list[dict] | None) -> list[str]:
    """The human-visible "what am I working from" panel: the user's goal plus the
    source record the form is to be filled from.

    Why this exists: `entities` otherwise live only inside the task JSON, and the
    candidate options refer to them as opaque pointers (`fill (with entity
    'ent_2')`), so nothing a model can actually see says what `ent_2` is. A solver
    can then tell that some field has a fillable value but not *which* value, which
    makes choosing between two plausible fill targets a guess rather than a
    decision. A real user always has the source (an email, a PDF, a record on
    another screen) in front of them; rendering it is what makes the fill decision
    determinable. It does NOT make the task easier in the shortcut sense: the
    hard-distractor decoys point at the SAME entity as the genuine field, so seeing
    the value still does not tell you which slot it belongs in.
    """
    lines: list[str] = []
    if goal:
        # Turn semantics. Without this line the task is genuinely ill-posed: nothing
        # says whether an element's label means "what should eventually happen here"
        # or "what do you do right now", so a solver that fills the form AND clicks
        # Submit in the same pass is being scored against an unstated convention.
        # Stating the interaction policy costs no real difficulty: which entity
        # belongs in which slot, which checkbox is actually required, and which
        # buttons are unsafe are all untouched by it.
        lines.append(
            "This is ONE turn. Judge every element against the screen's CURRENT state as shown "
            "below, NOT against the state it would be in after your other choices this turn. "
            "So: do not submit or advance while ANY field on this screen is still empty and has a "
            "value available in the source record, or a required box is still unticked -- even if "
            "you are also choosing to fill or tick it in this same turn; advancing comes on a later "
            "turn. An empty field the record has no value for is not fillable and never blocks "
            "advancing. Only fill a field when the record holds a value that genuinely belongs in "
            "THAT field: never repurpose a value that belongs to a different field, a different "
            "person, or a different point in time.")
        lines.append(f"Goal: {goal}")
    if entities:
        # A screen with nothing to fill (a pure pager) gets no source record: a
        # "Source record: State: MA" header above two paging buttons is noise no real
        # app would show, and the caller passes entities=None for that case.
        lines.append("Source record:")
        for e in entities:
            lines.append(f"  [{e['id']}] {e['label']}: {e['value']}")
    return lines


_MEASURE = ImageDraw.Draw(Image.new("RGB", (1, 1)))


def _wrap_lines(lines: list[str], max_width: float) -> list[str]:
    """Word-wrap each source-panel line to `max_width` px, preserving its leading
    indent on continuation lines. Deterministic and content-only (no layout state)."""
    out: list[str] = []
    for line in lines:
        indent = line[:len(line) - len(line.lstrip())]
        words = line.split()
        if not words:
            out.append(line)
            continue
        cur = indent + words[0]
        for w in words[1:]:
            cand = f"{cur} {w}"
            if _MEASURE.textlength(cand, font=FONT_LABEL) <= max_width:
                cur = cand
            else:
                out.append(cur)
                cur = indent + "  " + w
        out.append(cur)
    return out


def render_page(title: str, rows: list[dict], entities: list[dict] | None = None,
                goal: str | None = None) -> tuple[Image.Image, list[dict]]:
    """rows: [{"id","role","label","value","checked"}, ...] in display order.
    Returns (PIL image, elements) where elements carries pixel `frame` boxes
    (x1,y1,x2,y2) matching the rendered layout -- these become CuaTask.elements
    and are what a real visual element detector would also need to recover
    for a real screenshot, so keeping this layout simple/deterministic is
    deliberate.

    `goal`/`entities`, when given, draw a source-record panel between the title
    bar and the first form row (see `_source_lines`). The panel's height is a
    pure function of the NUMBER of (wrapped) source lines -- computed before
    anything is drawn -- so the module's "layout never depends on content
    length" contract still holds exactly: form-row frames shift by a constant,
    known offset.
    """
    src = _source_lines(goal, entities)
    # Wrap, never truncate: the goal and the turn-semantics line are long, and
    # ellipsizing them would silently cut the instruction off mid-sentence in the
    # SCREENSHOT while the text modality saw it in full -- a modality-specific
    # context-insufficiency asymmetry that would make the multimodal split harder
    # for a reason that has nothing to do with perception. Wrapping happens before
    # any drawing, so the fixed-row-height layout contract still holds (the panel's
    # height is a pure function of the wrapped line COUNT, known up front).
    src = _wrap_lines(src, PAGE_W - 2 * MARGIN)
    src_h = (SOURCE_PAD * 2 + len(src) * SOURCE_LINE_H) if src else 0
    height = TITLE_H + src_h + MARGIN * 2 + len(rows) * ROW_H
    img = Image.new("RGB", (PAGE_W, height), "white")
    draw = ImageDraw.Draw(img)
    draw.rectangle((0, 0, PAGE_W, TITLE_H), fill=(245, 245, 248))
    draw.text((MARGIN, 12), title, font=FONT_TITLE, fill=(20, 20, 20))

    if src:
        draw.rectangle((0, TITLE_H, PAGE_W, TITLE_H + src_h), fill=(252, 250, 235))
        draw.line((0, TITLE_H + src_h, PAGE_W, TITLE_H + src_h), fill=(220, 216, 190), width=1)
        sy = TITLE_H + SOURCE_PAD
        for line in src:
            draw.text((MARGIN, sy), _fit_text(draw, line, FONT_LABEL, PAGE_W - 2 * MARGIN),
                      font=FONT_LABEL, fill=(70, 62, 30))
            sy += SOURCE_LINE_H

    elements = []
    y = TITLE_H + src_h + MARGIN
    for row in rows:
        role, label = row["role"], row["label"]
        if role == "Edit" or role == "Select":
            label = f"{label} *" if row.get("required") else f"{label} (optional)"
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
            label = f"{label} *" if row.get("required") else label
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


def render_ax_tree(title: str, rows: list[dict], entities: list[dict] | None = None,
                   goal: str | None = None) -> str:
    """A markdown-style synthetic accessibility tree -- same information a real
    ax-tree capture would carry (role, name, value/checked state).

    `goal`/`entities` prepend the user goal and source record (see
    `_source_lines`) so the `fill (with entity 'ent_N')` options in the candidate
    set are actually resolvable from the context a model is shown."""
    lines = [f"# {title}", ""]
    src = _source_lines(goal, entities)
    if src:
        lines += src + [""]
    for row in rows:
        role, label = row["role"], row["label"]
        if role in ("Edit", "Select"):
            lines.append(f"- [{row['id']}] {role} \"{label}\" value=\"{row.get('value', '')}\""
                         f" required={bool(row.get('required', False))}")
        elif role == "CheckBox":
            lines.append(f"- [{row['id']}] {role} \"{label}\" checked={row.get('checked', False)}"
                         f" required={bool(row.get('required', False))}")
        else:
            lines.append(f"- [{row['id']}] {role} \"{label}\"")
    return "\n".join(lines)
