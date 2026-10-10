"""Shared plumbing for BenchLab probes: PRNG, seed derivation, log loading, scoring.

The PRNG and the per-mode derivations below must stay identical to the Swift
implementation in swift/BenchLab.swift (see swift/README.md for the spec). The
app never writes expected answers; evaluators recompute them from the seed here.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Callable

MASK64 = (1 << 64) - 1


# --------------------------------------------------------------------------- PRNG


class SplitMix64:
    """splitmix64: state += 0x9E3779B97F4A7C15, then two xor-shift-multiply rounds."""

    def __init__(self, seed: int) -> None:
        self.state = seed & MASK64

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        return z ^ (z >> 31)

    def index(self, n: int) -> int:
        """Uniform-ish index in [0, n) as next() % n."""
        return self.next() % n

    def randint(self, lo: int, hi: int) -> int:
        """Integer in [lo, hi] inclusive as lo + next() % (hi - lo + 1)."""
        return lo + self.next() % (hi - lo + 1)

    def shuffled(self, items: list[Any]) -> list[Any]:
        """Fisher-Yates from the end: for i = n-1 down to 1, swap i with next() % (i+1)."""
        out = list(items)
        for i in range(len(out) - 1, 0, -1):
            j = self.next() % (i + 1)
            out[i], out[j] = out[j], out[i]
        return out


# ------------------------------------------------------------------ data tables

NAMES = [
    "Ava Thompson", "Liam Okafor", "Noor Haddad", "Mateo Rivera", "Priya Raman",
    "Jonas Lindqvist", "Sofia Bianchi", "Kenji Watanabe", "Amara Nwosu", "Lucas Moreau",
    "Hana Kobayashi", "Diego Alvarez", "Freya Johansson", "Omar Farouk", "Chloe Dubois",
    "Rafael Costa", "Ingrid Larsen", "Tariq Mansour", "Elena Petrova", "Marcus Whitfield",
]  # fmt: skip
CATEGORIES = [
    "Hardware", "Software", "Services", "Travel",
    "Training", "Marketing", "Office Supplies", "Utilities",
]  # fmt: skip
PRIORITIES = ["Low", "Medium", "High"]
NOTES = [
    "Deliver to the loading dock before noon",
    "Customer requested a revised quote",
    "Net 30 terms apply to this order",
    "Fragile items so handle with care",
    "Call reception on arrival",
    "Split the shipment into two parts",
    "Include a copy of the signed contract",
    "Reference purchase order PO-7741",
    "Reminder to confirm the delivery window",
    "Archive after the quarterly review",
]
HOVER_NAMES = ["Archive", "Duplicate", "Export", "Pin", "Share", "Rename"]
COLORS = ["red", "green", "blue"]

TABLE_ROWS = 400
CANVAS_W, CANVAS_H = 700, 420
ZONE_SIZE, TILE_SIZE = 96, 56
COLUMN_W = 233
CLICK_COLS, CLICK_ROWS, CLICK_COUNT = 6, 4, 24
CLICK_CELL_W, CLICK_CELL_H = 116, 105
CIRCLE_RADIUS = 14

# Initial control values in the forms mode (fixed, not seed dependent).
FORMS_DEFAULTS: dict[str, Any] = {
    "customer_name": "",
    "invoice_amount": "",
    "category": 0,
    "priority": None,
    "notify": False,
    "quantity": 1,
    "notes": "",
}


# ----------------------------------------------------------------- derivations


def derive_forms(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    name = NAMES[r.index(20)]
    dollars = r.randint(200, 9800)
    cents = (0, 25, 50, 75)[r.index(4)]
    category = r.randint(1, 7)
    priority = PRIORITIES[r.index(3)]
    notify = r.index(2) == 1
    quantity = r.randint(2, 99)
    notes = NOTES[r.index(10)]
    return {
        "customer_name": name,
        "invoice_amount": f"{dollars}.{cents:02d}",
        "category_index": category,
        "category_title": CATEGORIES[category],
        "priority": priority,
        "notify": notify,
        "quantity": quantity,
        "notes": notes,
    }


def derive_table(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    rows = (r.randint(30, 130), r.randint(131, 260), r.randint(261, 400))
    return {"codes": [f"K-{n:04d}" for n in rows], "row_count": TABLE_ROWS}


def derive_canvas(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    zone_cols = r.shuffled([0, 1, 2])
    zones = []
    for i, color in enumerate(COLORS):
        x = zone_cols[i] * COLUMN_W + r.randint(8, 129)
        y = r.randint(16, 150)
        zones.append({"name": color, "x": x, "y": y, "w": ZONE_SIZE, "h": ZONE_SIZE})
    tile_cols = r.shuffled([0, 1, 2])
    tiles = []
    for i, color in enumerate(COLORS):
        x = tile_cols[i] * COLUMN_W + r.randint(8, 169)
        y = r.randint(310, 356)
        tiles.append({"name": color, "x": x, "y": y, "size": TILE_SIZE})
    return {"zones": zones, "tiles": tiles}


def derive_hover(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    names = r.shuffled(HOVER_NAMES)[:4]
    target_index = r.randint(0, 3)
    return {"names": names, "target_index": target_index, "target": names[target_index]}


def derive_clipboard(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    a = r.randint(120, 989)
    b = r.randint(11, 97)
    c = r.randint(1000, 99999)
    return {"a": a, "b": b, "c": c, "result": a * b + c}


def derive_canvasclick(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    labels = r.shuffled(list(range(1, CLICK_COUNT + 1)))
    circles = []
    for i in range(CLICK_COUNT):
        jx = r.randint(-30, 30)
        jy = r.randint(-26, 26)
        circles.append(
            {
                "index": i,
                "label": labels[i],
                "cx": (i % CLICK_COLS) * CLICK_CELL_W + CLICK_CELL_W // 2 + jx,
                "cy": (i // CLICK_COLS) * CLICK_CELL_H + 52 + jy,
            }
        )
    perm = r.shuffled(list(range(1, CLICK_COUNT + 1)))
    return {
        "circles": circles,
        "radius": CIRCLE_RADIUS,
        "left_sequence": perm[:12],
        "right_targets": perm[12:16],
    }


# ------------------------------------------------- macOS bench (MB-*) additions
# Keep identical to swift/BenchLabModes.swift (Tables2 and the *Params structs).

MENU_ACTIONS = ["Pin", "Mute", "Archive", "Highlight", "Duplicate"]
ITEM_WORDS = [
    "Anvil", "Basil", "Cedar", "Delta", "Ember", "Falcon", "Garnet", "Harbor",
    "Indigo", "Juniper", "Kestrel", "Lantern", "Maple", "Nimbus", "Orchid", "Pebble",
    "Quartz", "Raven", "Saffron", "Tundra", "Umber", "Violet", "Willow", "Yarrow",
]  # fmt: skip
ITEM_KINDS = ["bracket", "gasket", "sensor", "valve", "widget", "fitting"]
SENTENCES = [
    ("The ferry leaves the harbor at dawn.", "harbor", "marina"),
    ("A quiet storm crossed the valley.", "storm", "breeze"),
    ("Our team shipped the update on Friday.", "Friday", "Thursday"),
    ("The baker sold every loaf before noon.", "loaf", "roll"),
    ("Maria left a green umbrella at the cafe.", "umbrella", "scarf"),
    ("The old clock tower chimes at six.", "chimes", "rings"),
    ("Two cyclists climbed the steep hill.", "steep", "long"),
    ("Rain delayed the concert by an hour.", "concert", "match"),
    ("The library opens its doors at nine.", "library", "museum"),
    ("A small robot watered the garden.", "robot", "drone"),
    ("Snow covered the mountain road overnight.", "Snow", "Frost"),
    ("The pilot checked the engine twice.", "engine", "radio"),
]  # fmt: skip
ORDINALS = ["first", "second", "third"]
DISPLAY_NAMES = [
    "Orbit Studio", "Maple Desk", "North Wing", "Quiet Harbor", "Blue Annex",
    "Copper Lab", "Juniper Room", "Signal House", "Linden Hall", "Atlas Corner",
]  # fmt: skip
THEME_CHOICES = ["Light", "Sepia", "Slate", "Forest"]
THEME_TARGETS = ["Sepia", "Slate", "Forest"]
LIST_ITEMS = [
    "Draft agenda", "Book room", "Send invites", "Print slides", "Test projector",
    "Order lunch", "Share notes", "Archive files", "Collect badges", "Update roster",
]  # fmt: skip
ICONS = ["Truck", "Cube", "Flag", "Clock"]
CODE_LETTERS = "ACEFGHKLMNPRTXZ"


def derive_canvasmenu(seed: int) -> dict[str, Any]:
    base = derive_canvasclick(seed)
    r = SplitMix64(seed ^ 0xC3C3C3C3C3C3C3C3)
    actions = [MENU_ACTIONS[r.index(5)] for _ in range(4)]
    targets = base["right_targets"]
    return {
        "circles": base["circles"],
        "radius": base["radius"],
        "targets": targets,
        "actions": actions,
        "expected": dict(zip(targets, actions)),
    }


def derive_tablesel(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    w = r.index(24)
    k = r.index(6)
    qty = r.randint(101, 499)
    row = r.randint(200, 380)
    name = f"{ITEM_WORDS[w]} {ITEM_KINDS[k]}"
    d1 = r.randint(20, 150)
    q1 = r.randint(101, 499)
    if q1 == qty:
        q1 += 1
    d2 = r.randint(151, 199)
    q2 = r.randint(101, 499)
    if q2 == qty:
        q2 += 1
    d3 = r.randint(381, 400)
    w3 = (w + 1 + r.index(23)) % 24
    return {
        "code": f"K-{row:04d}",
        "row": row,
        "name": name,
        "qty": qty,
        "decoys": [
            {"row": d1, "name": name, "qty": q1},
            {"row": d2, "name": name, "qty": q2},
            {"row": d3, "name": f"{ITEM_WORDS[w3]} {ITEM_KINDS[k]}", "qty": qty},
        ],
    }


def derive_richtext(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    idx = r.shuffled(list(range(12)))[:3]
    bold_index = r.index(3)
    replace_index = (bold_index + 1 + r.index(2)) % 3
    sentences = [SENTENCES[i][0] for i in idx]
    _, old, new = SENTENCES[idx[replace_index]]
    final_sentences = list(sentences)
    final_sentences[replace_index] = sentences[replace_index].replace(old, new, 1)
    start = sum(len(s) + 1 for s in final_sentences[:bold_index])
    bold_sentence = final_sentences[bold_index]
    return {
        "sentences": sentences,
        "bold_index": bold_index,
        "replace_index": replace_index,
        "old_word": old,
        "new_word": new,
        "paragraph": " ".join(sentences),
        "final_text": " ".join(final_sentences),
        "bold_sentence": bold_sentence,
        "bold_range": [start, len(bold_sentence)],
    }


def derive_settings(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    name = DISPLAY_NAMES[r.index(10)]
    theme = THEME_TARGETS[r.index(3)]
    compact_default = r.index(2) == 1
    compact_target = not compact_default
    return {
        "name": name,
        "theme": theme,
        "compact_default": compact_default,
        "compact_target": compact_target,
        "ticket": settings_ticket(seed, name, theme, compact_target),
    }


def fnv1a32(data: str) -> int:
    h = 2166136261
    for b in data.encode("utf-8"):
        h ^= b
        h = (h * 16777619) & 0xFFFFFFFF
    return h


def settings_ticket(seed: int, name: str, theme: str, compact: bool) -> int:
    return 1000 + fnv1a32(f"{seed}|{name}|{theme}|{1 if compact else 0}") % 9000


def derive_listdrag(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    initial = r.shuffled(LIST_ITEMS)[:5]
    target = r.shuffled(initial)
    while sum(1 for a, b in zip(initial, target) if a != b) < 4:
        target = r.shuffled(initial)
    return {"initial": initial, "target": target}


def derive_tooltip(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed)
    order = r.shuffled(ICONS)
    target_index = r.index(4)
    codes = []
    for _ in range(4):
        a = CODE_LETTERS[r.index(15)]
        b = CODE_LETTERS[r.index(15)]
        n = r.randint(1000, 9999)
        codes.append(f"{a}{b}-{n}")
    return {
        "order": order,
        "target_index": target_index,
        "target": order[target_index],
        "codes": codes,
        "target_code": codes[target_index],
    }


def derive_mode_params(seed: int) -> dict[str, Any]:
    return {
        "canvasmenu": derive_canvasmenu(seed),
        "tablesel": derive_tablesel(seed),
        "richtext": derive_richtext(seed),
        "settings": derive_settings(seed),
        "listdrag": derive_listdrag(seed),
        "tooltip": derive_tooltip(seed),
    }


# ------------------------------------------ Bench v2 interruption probes (IR-*)
# Keep identical to swift/BenchLabInterrupts.swift (TablesIR, irSalt and the IR*Params structs).

IR_TEAMS = ["Design", "Finance", "Operations", "Research"]
IR_PERM_RESOURCES = ["Contacts", "Calendars", "Photos"]
IR_NOTE_TITLES = [
    "Budget review", "Client kickoff", "Hiring plan", "Launch checklist",
    "Office move", "Q3 roadmap", "Supplier audit", "Team offsite",
]  # fmt: skip
IR_STATUS_WORDS = ["Approved", "On hold", "Shipped", "Cancelled"]
IR_PLANS = ["Starter", "Team", "Enterprise"]
IR_INITIAL_STATUS = "Draft"
IR_SALT = {
    "irmodal": 0x1A011A011A011A01,
    "irbanner": 0x1A021A021A021A02,
    "irunsaved": 0x1A031A031A031A03,
    "irconsent": 0x1A041A041A041A04,
}


def ir_email(name: str, domain: str) -> str:
    return name.lower().replace(" ", ".") + "@" + domain


def derive_irmodal(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed ^ IR_SALT["irmodal"])
    name = NAMES[r.index(20)]
    return {
        "name": name,
        "email": ir_email(name, "example.com"),
        "team": IR_TEAMS[r.index(4)],
        "seats": r.randint(2, 40),
        "resource": IR_PERM_RESOURCES[r.index(3)],
    }


def derive_irbanner(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed ^ IR_SALT["irbanner"])
    item = f"{ITEM_WORDS[r.index(24)]} {ITEM_KINDS[r.index(6)]}"
    return {"item": item, "quantity": r.randint(2, 60), "priority": PRIORITIES[r.index(3)]}


def derive_irunsaved(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed ^ IR_SALT["irunsaved"])
    titles = r.shuffled(IR_NOTE_TITLES)[:3]
    status_index = r.index(3)
    rename_index = (status_index + 1 + r.index(2)) % 3
    status = IR_STATUS_WORDS[r.index(4)]
    new_title = f"{ITEM_WORDS[r.index(24)]} plan"
    return {
        "titles": titles,
        "status_index": status_index,
        "rename_index": rename_index,
        "status": status,
        "new_title": new_title,
    }


def derive_irconsent(seed: int) -> dict[str, Any]:
    r = SplitMix64(seed ^ IR_SALT["irconsent"])
    name = NAMES[r.index(20)]
    return {
        "name": name,
        "email": ir_email(name, "example.org"),
        "plan": IR_PLANS[r.index(3)],
        "digest": r.index(2) == 1,
    }


def derive_ir_params(seed: int) -> dict[str, Any]:
    return {
        "irmodal": derive_irmodal(seed),
        "irbanner": derive_irbanner(seed),
        "irunsaved": derive_irunsaved(seed),
        "irconsent": derive_irconsent(seed),
    }


def derive_all(seed: int) -> dict[str, Any]:
    return {
        "prng_head": [str(v) for v in _head(seed, 4)],
        "forms": derive_forms(seed),
        "table": derive_table(seed),
        "canvas": derive_canvas(seed),
        "canvasclick": derive_canvasclick(seed),
        "hover": derive_hover(seed),
        "clipboard": derive_clipboard(seed),
    }


def _head(seed: int, n: int) -> list[int]:
    r = SplitMix64(seed)
    return [r.next() for _ in range(n)]


# ------------------------------------------------------------------ brief render


def render_template(path: Path, values: dict[str, Any]) -> str:
    """Fill {{key}} placeholders; an unknown or unused key is an error."""
    text = path.read_text("utf-8")
    used: set[str] = set()

    def sub(match: re.Match[str]) -> str:
        key = match.group(1).strip()
        if key not in values:
            raise KeyError(f"brief placeholder {key!r} has no value")
        used.add(key)
        return str(values[key])

    out = re.sub(r"\{\{\s*([A-Za-z0-9_]+)\s*\}\}", sub, text)
    unused = set(values) - used
    if unused:
        raise KeyError(f"brief values not used by template: {sorted(unused)}")
    return out


# ----------------------------------------------------------------- input loading


class LoadError(Exception):
    pass


def load_state(path: str | os.PathLike[str]) -> dict[str, Any]:
    try:
        raw = Path(path).read_text("utf-8")
    except OSError as error:
        raise LoadError(f"state file unreadable: {error}") from error
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as error:
        raise LoadError(f"state file is not valid JSON: {error}") from error
    if not isinstance(data, dict):
        raise LoadError("state file is not a JSON object")
    return data


def load_events(path: str | os.PathLike[str]) -> tuple[list[dict[str, Any]], int]:
    """Return (events, malformed_line_count)."""
    try:
        raw = Path(path).read_text("utf-8")
    except OSError as error:
        raise LoadError(f"events file unreadable: {error}") from error
    events: list[dict[str, Any]] = []
    bad = 0
    for line in raw.splitlines():
        if not line.strip():
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            bad += 1
            continue
        if isinstance(obj, dict):
            events.append(obj)
        else:
            bad += 1
    return events, bad


# ---------------------------------------------------------------- check plumbing


class Checks:
    """Required checks decide `passed` and `score`; diagnostics are recorded and never decide."""

    def __init__(self, extra: dict[str, Any] | None = None) -> None:
        self.items: dict[str, dict[str, Any]] = {}
        self.diagnostics: dict[str, dict[str, Any]] = {}
        self.extra: dict[str, Any] = extra or {}

    def add(self, name: str, passed: bool, weight: float, detail: str = "") -> bool:
        self.items[name] = {"passed": bool(passed), "weight": float(weight), "detail": detail}
        return bool(passed)

    def diag(self, name: str, value: Any, detail: str = "") -> None:
        self.diagnostics[name] = {"value": value, "detail": detail}

    def result(self) -> dict[str, Any]:
        total = sum(c["weight"] for c in self.items.values())
        earned = sum(c["weight"] for c in self.items.values() if c["passed"])
        score = round(earned / total, 4) if total > 0 else 0.0
        passed = bool(self.items) and all(c["passed"] for c in self.items.values())
        out: dict[str, Any] = {"passed": passed, "score": score, "checks": self.items}
        if self.diagnostics:
            out["diagnostics"] = self.diagnostics
        return out


def is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def is_num(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def events_of(events: list[dict[str, Any]], type_: str) -> list[dict[str, Any]]:
    return [e for e in events if e.get("type") == type_]


def details(event: dict[str, Any]) -> dict[str, Any]:
    d = event.get("details")
    return d if isinstance(d, dict) else {}


def integrity_problems(
    mode: str,
    seed: int,
    state: dict[str, Any],
    events: list[dict[str, Any]],
    bad_lines: int,
) -> list[str]:
    """Check that state and events belong together and look app-written."""
    problems: list[str] = []
    if bad_lines:
        problems.append(f"{bad_lines} malformed event line(s)")
    if state.get("mode") != mode:
        problems.append(f"state mode is {state.get('mode')!r}, expected {mode!r}")
    if state.get("seed") != seed or not is_int(state.get("seed")):
        problems.append(f"state seed is {state.get('seed')!r}, expected {seed}")
    if not is_int(state.get("seq")):
        problems.append("state has no integer seq")
    if not events:
        problems.append("events log is empty")
        return problems
    for index, event in enumerate(events):
        if event.get("seq") != index + 1 or not is_int(event.get("seq")):
            problems.append(f"event {index + 1} has seq {event.get('seq')!r}")
            break
        if not is_num(event.get("t")) or not isinstance(event.get("type"), str):
            problems.append(f"event {index + 1} lacks numeric t or string type")
            break
        if not isinstance(event.get("details"), dict):
            problems.append(f"event {index + 1} lacks details object")
            break
    first = events[0]
    first_details = details(first)
    if first.get("type") != "app_start":
        problems.append("first event is not app_start")
    elif first_details.get("mode") != mode or first_details.get("seed") != seed:
        problems.append("app_start mode or seed does not match")
    if is_int(state.get("seq")) and is_int(events[-1].get("seq")):
        if abs(state["seq"] - events[-1]["seq"]) > 1:
            problems.append(
                f"state seq {state['seq']} does not match last event seq {events[-1]['seq']}"
            )
    return problems


def evaluate_inputs(
    mode: str,
    seed: int,
    state: dict[str, Any],
    events: list[dict[str, Any]],
    bad_lines: int,
    evaluate: Callable[[int, dict[str, Any], list[dict[str, Any]], Checks], None],
    integrity_weight: float = 0.05,
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    checks = Checks(extra)
    problems = integrity_problems(mode, seed, state, events, bad_lines)
    checks.add(
        "integrity",
        not problems,
        integrity_weight,
        "; ".join(problems) or "state and events are consistent",
    )
    try:
        evaluate(seed, state, events, checks)
    except Exception as error:  # noqa: BLE001 - malformed content must fail closed, not crash
        checks.add("evaluator_error", False, 1.0, f"{type(error).__name__}: {error}")
    return checks.result()


def failed_result(name: str, detail: str) -> dict[str, Any]:
    checks = Checks()
    checks.add(name, False, 1.0, detail)
    return checks.result()


def write_result(path: str | os.PathLike[str], result: dict[str, Any]) -> None:
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=target.parent, prefix=".result-", suffix=".json")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2, sort_keys=True)
            handle.write("\n")
        os.replace(tmp, target)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def load_sentinel(path: str) -> dict[str, Any] | None:
    """Sentinel summary from a summary .json, or from a raw sentinel .jsonl log (summarized here)."""
    try:
        if path.endswith(".jsonl"):
            sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "swift"))
            import summarize_sentinel  # type: ignore

            return summarize_sentinel.summarize(path)
        return json.loads(Path(path).read_text("utf-8"))
    except Exception:  # noqa: BLE001 - a missing or broken sentinel input fails the checks, not the evaluator
        return None


def run_cli(
    mode: str,
    evaluate: Callable[[int, dict[str, Any], list[dict[str, Any]], Checks], None],
    argv: list[str] | None = None,
) -> int:
    """CLI entry: always writes a result file; exit 0 once it is written.

    `--sentinel PATH` is optional: the JSON written by summarize_sentinel for the same trial. Only
    evaluators that score disturbance (MB-12) read it, from `checks.extra["sentinel"]`.
    """
    parser = argparse.ArgumentParser(description=f"Evaluate a BenchLab {mode} run.")
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--state", required=True)
    parser.add_argument("--events", required=True)
    parser.add_argument("--result", required=True)
    parser.add_argument("--sentinel", default=None)
    args = parser.parse_args(argv)
    try:
        state = load_state(args.state)
        events, bad = load_events(args.events)
        extra: dict[str, Any] = {}
        if args.sentinel:
            extra["sentinel"] = load_sentinel(args.sentinel)
        result = evaluate_inputs(mode, args.seed, state, events, bad, evaluate, extra=extra)
    except LoadError as error:
        result = failed_result("inputs_loadable", str(error))
    except Exception as error:  # noqa: BLE001
        result = failed_result("evaluator_error", f"{type(error).__name__}: {error}")
    write_result(args.result, result)
    print(json.dumps({"passed": result["passed"], "score": result["score"]}))
    return 0


# --------------------------------------------------------------- value parsing


def normalize_text(value: Any) -> str | None:
    return value.strip() if isinstance(value, str) else None


def parse_amount(value: Any) -> Decimal | None:
    """Parse '1,284.50' or '$1284.5'; None when not a plain decimal number."""
    if not isinstance(value, str):
        return None
    text = re.sub(r"[\s\u00a0\u202f,$]", "", value)
    if not re.fullmatch(r"-?\d+(\.\d+)?", text):
        return None
    try:
        return Decimal(text)
    except InvalidOperation:
        return None


def parse_integer_result(value: Any) -> int | None:
    """Parse an integer with optional thousands commas and whitespace."""
    if not isinstance(value, str):
        return None
    text = re.sub(r"[\s\u00a0\u202f]", "", value)
    if re.fullmatch(r"-?\d{1,3}(,\d{3})+", text) or re.fullmatch(r"-?\d+", text):
        return int(text.replace(",", ""))
    return None


def main_guard(mode: str, evaluate: Callable[..., None]) -> None:
    sys.exit(run_cli(mode, evaluate))
