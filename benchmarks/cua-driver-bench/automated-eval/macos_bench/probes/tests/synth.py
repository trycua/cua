"""Synthetic BenchLab state/events builders that mimic what the app writes."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

PROBES = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROBES / "_common"))
import benchlab_common as C  # noqa: E402

T0 = 1_760_000_000_000.0


def load_module(probe_id: str, filename: str):
    path = PROBES / probe_id / filename
    spec = importlib.util.spec_from_file_location(f"{probe_id}_{path.stem}".replace("-", "_"), path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def evaluate(
    probe_id: str, mode: str, seed: int, state: dict, events: list[dict], bad_lines: int = 0
) -> dict:
    module = load_module(probe_id, "evaluate.py")
    return C.evaluate_inputs(mode, seed, state, events, bad_lines, module.evaluate)


class Sim:
    """Event recorder with the same seq/t/type/details envelope as the app."""

    def __init__(self, mode: str, seed: int) -> None:
        self.mode, self.seed = mode, seed
        self.seq = 0
        self.t = T0
        self.events: list[dict[str, Any]] = []
        self.emit("app_start", mode=mode, seed=seed)

    def emit(self, type_: str, **details: Any) -> None:
        self.seq += 1
        self.t += 40
        self.events.append({"seq": self.seq, "t": self.t, "type": type_, "details": details})

    def finish(self, state: dict[str, Any]) -> tuple[dict[str, Any], list[dict[str, Any]]]:
        state = dict(state)
        state.update(
            {
                "mode": self.mode,
                "seed": self.seed,
                "seq": self.seq,
                "pid": 4242,
                "updated_ms": self.t,
            }
        )
        return state, self.events


def clone(state: dict, events: list[dict]) -> tuple[dict, list[dict]]:
    return copy.deepcopy(state), copy.deepcopy(events)


# ------------------------------------------------------------------------ forms


def forms_fields(exp: dict[str, Any]) -> dict[str, Any]:
    return {
        "customer_name": exp["customer_name"],
        "invoice_amount": exp["invoice_amount"],
        "category_index": exp["category_index"],
        "category_title": exp["category_title"],
        "priority": exp["priority"],
        "notify": exp["notify"],
        "quantity": exp["quantity"],
        "quantity_text": str(exp["quantity"]),
        "notes": exp["notes"],
    }


def forms_run(seed: int, fields: dict[str, Any] | None = None, submits: int = 1):
    """A genuine forms run; `fields` overrides what the user ends up submitting."""
    exp = C.derive_forms(seed)
    snap = forms_fields(exp)
    if fields:
        snap.update(fields)
    sim = Sim("forms", seed)
    sim.emit("field_edit", field="customer_name", value=snap["customer_name"][:3], via="typing")
    sim.emit("field_edit", field="customer_name", value=snap["customer_name"], via="typing")
    sim.emit("field_edit", field="invoice_amount", value=snap["invoice_amount"], via="typing")
    sim.emit(
        "field_edit",
        field="category",
        value=snap["category_index"],
        title=snap["category_title"],
        via="action",
    )
    sim.emit("field_edit", field="priority", value=snap["priority"], via="action")
    if snap["notify"]:
        sim.emit("field_edit", field="notify", value=True, via="action")
    sim.emit(
        "field_edit",
        field="quantity",
        value=snap["quantity"],
        text=snap["quantity_text"],
        via="stepper",
    )
    sim.emit("field_edit", field="notes", value=snap["notes"], via="typing")
    for count in range(1, submits + 1):
        sim.emit("submit", fields=snap, count=count)
    state = {
        "fields": snap,
        "submitted": True,
        "submit_count": submits,
        "submission": {"fields": snap},
        "status": "Submitted",
    }
    return sim.finish(state)


# ------------------------------------------------------------------------ table


def table_run(seed: int, flag: list[str] | None = None, save: bool = True):
    codes = flag if flag is not None else C.derive_table(seed)["codes"]
    sim = Sim("table", seed)
    sim.emit("scroll", first_visible_row=40)
    flagged: list[str] = []
    for code in codes:
        sim.emit("flag_toggle", code=code, row=int(code[2:]) - 1, flagged=True)
        flagged.append(code)
    saved = None
    if save:
        saved = sorted(flagged)
        sim.emit("save", flagged=saved, count=1)
    state = {
        "flagged": sorted(flagged),
        "flagged_count": len(flagged),
        "saved": save,
        "save_count": 1 if save else 0,
        "saved_flagged": saved,
        "first_visible_row": 40,
        "status": "Saved" if save else "",
    }
    return sim.finish(state)


# ----------------------------------------------------------------------- canvas


def _tile_center(t: dict[str, int]) -> tuple[float, float]:
    return t["x"] + t["size"] / 2, t["y"] + t["size"] / 2


def canvas_run(
    seed: int, targets: dict[str, tuple[float, float]] | None = None, press_done: bool = True
):
    """Drag each tile to `targets` (default: its zone center), then press Done."""
    exp = C.derive_canvas(seed)
    zones = {z["name"]: z for z in exp["zones"]}
    centers = {t["name"]: _tile_center(t) for t in exp["tiles"]}
    sim = Sim("canvas", seed)
    counts = {"mouseDown": 0, "mouseDragged": 0, "mouseUp": 0}
    sequences = 0

    def mouse(phase: str, x: float, y: float, tile: str | None) -> None:
        counts[phase] += 1
        d: dict[str, Any] = {"phase": phase, "x": x, "y": y, "tile": tile}
        if tile:
            d["cx"], d["cy"] = centers[tile]
        sim.emit("mouse", **d)

    for name in C.COLORS:
        zone = zones[name]
        goal = (targets or {}).get(name, (zone["x"] + zone["w"] / 2, zone["y"] + zone["h"] / 2))
        sx, sy = centers[name]
        mouse("mouseDown", sx, sy, name)
        for step in (0.25, 0.5, 0.75, 1.0):
            centers[name] = (sx + (goal[0] - sx) * step, sy + (goal[1] - sy) * step)
            mouse("mouseDragged", centers[name][0], centers[name][1], name)
        mouse("mouseUp", centers[name][0], centers[name][1], name)
        sequences += 1

    snapshot = [{"name": n, "cx": centers[n][0], "cy": centers[n][1]} for n in C.COLORS]
    if press_done:
        sim.emit("done_click", tiles=snapshot)
    state = {
        "tiles": snapshot,
        "zones": [
            {"name": z["name"], "x": float(z["x"]), "y": float(z["y"]), "w": 96.0, "h": 96.0}
            for z in exp["zones"]
        ],
        "done": press_done,
        "done_snapshot": snapshot if press_done else None,
        "drag_sequences": sequences,
        "mouse_events": counts,
    }
    return sim.finish(state)


# ------------------------------------------------------------------ canvasclick


def canvasclick_run(seed: int, actions: list[tuple] | None = None):
    """Replay `actions`: ("left", label), ("right", label), ("miss", button),
    ("down_only", button, label) or ("done",). Default is the full required task."""
    exp = C.derive_canvasclick(seed)
    by_label = {c["label"]: c for c in exp["circles"]}
    if actions is None:
        actions = (
            [("left", n) for n in exp["left_sequence"]]
            + [("right", n) for n in exp["right_targets"]]
            + [("done",)]
        )
    sim = Sim("canvasclick", seed)
    left = [0] * C.CLICK_COUNT
    right = [0] * C.CLICK_COUNT
    log: list[dict] = []
    counts = {
        "mouseDown": 0,
        "mouseUp": 0,
        "rightMouseDown": 0,
        "rightMouseUp": 0,
        "otherMouseDown": 0,
        "otherMouseUp": 0,
    }
    phases = {"left": ("mouseDown", "mouseUp"), "right": ("rightMouseDown", "rightMouseUp")}
    done = 0

    def mouse(phase: str, button: str, x: float, y: float, circle: dict | None) -> None:
        counts[phase] += 1
        sim.emit(
            "mouse",
            phase=phase,
            button=button,
            x=x,
            y=y,
            circle=circle["index"] if circle else None,
            label=circle["label"] if circle else None,
            ctrl=False,
        )

    for action in actions:
        kind = action[0]
        if kind in ("left", "right"):
            c = by_label[action[1]]
            x, y = c["cx"] + 3.5, c["cy"] - 2.0
            mouse(phases[kind][0], kind, x, y, c)
            mouse(phases[kind][1], kind, x, y, c)
            (left if kind == "left" else right)[c["index"]] += 1
            log.append({"button": kind, "circle": c["index"], "label": c["label"], "x": x, "y": y})
        elif kind == "miss":
            button = action[1]
            mouse(phases[button][0], button, 2.0, 2.0, None)
            mouse(phases[button][1], button, 2.0, 2.0, None)
            log.append({"button": button, "circle": None, "label": None, "x": 2.0, "y": 2.0})
        elif kind == "down_only":
            c = by_label[action[2]]
            mouse(phases[action[1]][0], action[1], c["cx"] + 1.0, c["cy"], c)
        elif kind == "done":
            done += 1
            sim.emit("done_click", count=done)
    state = {
        "circles": [
            {**c, "left_clicks": left[c["index"]], "right_clicks": right[c["index"]]}
            for c in exp["circles"]
        ],
        "click_log": log,
        "mouse_events": counts,
        "done": done > 0,
        "done_count": done,
    }
    return sim.finish(state)


# ------------------------------------------------------------------------ hover


def hover_run(
    seed: int, click: str | None = None, overlay_visible: bool = True, with_hover: bool = True
):
    exp = C.derive_hover(seed)
    name = click or exp["target"]
    sim = Sim("hover", seed)
    enters = exits = 0
    if with_hover:
        sim.emit("mouse", phase="mouseEntered", x=40.0, y=20.0, region="hot_zone")
        enters = 1
        sim.emit("overlay_shown", names=exp["names"])
        sim.emit("mouse", phase="mouseMoved", x=44.0, y=22.0, region="hot_zone")
    sim.emit(
        "overlay_click", name=name, overlay_visible=overlay_visible, hot_zone_hovered=with_hover
    )
    if with_hover:
        sim.emit("mouse", phase="mouseExited", x=60.0, y=60.0, region="hot_zone")
        exits = 1
        sim.emit("overlay_hidden")
    click_rec = {
        "name": name,
        "overlay_visible": overlay_visible,
        "hot_zone_hovered": with_hover,
        "t": sim.t,
    }
    state = {
        "overlay_visible": False,
        "overlay_names": exp["names"],
        "hot_zone_hovered": False,
        "hover_enter_count": enters,
        "hover_exit_count": exits,
        "clicks": [click_rec],
        "last_clicked": name,
        "last_click_overlay_visible": overlay_visible,
        "status": f"Last action: {name}",
    }
    return sim.finish(state)


# -------------------------------------------------------------------- clipboard


def clipboard_run(seed: int, typed: str | None = None):
    exp = C.derive_clipboard(seed)
    value = typed if typed is not None else str(exp["result"])
    sim = Sim("clipboard", seed)
    sim.emit("field_edit", field="result", value=value, via="typing")
    sim.emit("save", value=value, count=1)
    state = {
        "result_text": value,
        "saved": True,
        "save_count": 1,
        "saved_value": value,
        "pasteboard_changes": 2,
        "status": "Saved",
    }
    return sim.finish(state)


def write_files(directory: Path, state: dict | None, events: list[dict] | None, torn: bool = False):
    state_path, events_path = directory / "state.json", directory / "events.jsonl"
    if state is not None:
        state_path.write_text(json.dumps(state), "utf-8")
    if events is not None:
        text = "".join(json.dumps(e) + "\n" for e in events)
        events_path.write_text(text + ('{"seq": 99, "t": ' if torn else ""), "utf-8")
    return state_path, events_path
