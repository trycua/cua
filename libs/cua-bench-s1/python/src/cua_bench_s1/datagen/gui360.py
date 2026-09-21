"""GUI-360 (Microsoft Research / community authors, MIT) -> CuaTask converter.

Source: https://huggingface.co/datasets/vyokky/GUI-360
Paper: "GUI-360: A Comprehensive Dataset And Benchmark For Computer-Using
Agents" (arXiv 2511.04307), code at github.com/2020-qqtcg/GUI-360.

Why this dataset (chosen over OSWorld/WindowsAgentArena): OSWorld and
WindowsAgentArena are both primarily *live-VM execution* benchmarks -- what
they publish is task configs (goal text + VM snapshot + a checker script),
not recorded (state, action) trajectories. Actually driving them requires
running an agent against a live VM to generate trajectories yourself, which
is out of scope here and would make any "real" claim about per-step ground
truth false. GUI-360, by contrast, ships real recorded per-step data: a
screenshot, a live Windows UI Automation (UIA) control tree, and a single
structured gold action, for both successful and failed human/agent-collected
trajectories over native Word/Excel/PowerPoint desktop apps (not a browser)
-- exactly the (state, action) pair shape this converter needs, with no VM
replay step required.

What this module does, end to end:
  1. `iter_episode_steps` reads one raw per-episode JSONL file (GUI-360 ships
     one small JSONL per episode, e.g.
     `test/data/excel/in_app/success/excel_1_2.jsonl`) and yields its steps
     in `step_id` order.
  2. Each step's `control_infos.uia_controls_info` list -- real UIA control
     records captured live from the running Office app (control_type,
     on-screen control_rect, control_text, and a small integer `label` used
     by GUI-360's own on-screen annotation overlay) -- is flattened into this
     benchmark's flat `elements` format via `interactable_elements`.
  3. Each step's single gold `action` dict (function: click/type/
     wheel_mouse_input/select_text/...) is resolved to one of those elements
     by exact match on GUI-360's own `control_label` field against each
     element's `label` -- unlike AndroidControl (which had no such id and
     needed coordinate-snapping), GUI-360 already carries an explicit,
     unambiguous element pointer per action, so resolution here is exact,
     not nearest-match.
  4. The action's `function` is mapped onto this benchmark's action
     vocabulary (fill/check/click/select/scroll/skip) -- see `map_action` for
     the exact per-function rules and what's dropped.
  5. A small option set is built per step, mirroring androidcontrol.py's
     "every element gets skip; the resolved gold element also gets the
     mapped gold action" design -- not a combinatorial cross-join.
  6. Family assignment reuses `androidcontrol.assign_family`'s exact
     heuristic (same family taxonomy, same keyword rules) so both real
     integrations feed the same downstream family buckets.

Action-mapping decisions (the parts that do NOT map cleanly):
  - `click` -> `check` if the resolved control's `control_type` is
    `CheckBox`; `select` if it's `RadioButton`/`ComboBox`/`Spinner`
    (choosing among alternatives); else `click`.
  - `type` -> `fill`, with `args.text` as the gold entity value.
  - `select_text` / `select_paragraph` / `select_table_range` /
    `select_table` -> `select`. GUI-360 has no single action type matching
    this benchmark's `select` cleanly; these four "select a range of
    existing content" functions are the closest real fit (all are "choose
    among/within what's already on screen" rather than a fresh click or a
    typed value) -- documented, not invented data.
  - `wheel_mouse_input` -> `scroll`, targeting the resolved control_label's
    element directly (GUI-360's mouse-wheel action already carries a
    concrete on-screen target control, unlike AndroidControl's scroll which
    had none).
  - `''` (empty function, `action_type: "API"`, GUI-360's `OVERALL_FINISH`
    terminal marker), `drag`, and `set_focus` -> NOT converted. The first is
    an episode-level completion marker with no on-screen target at all;
    `drag` has no equivalent action in this benchmark; `set_focus` has no
    associated `control_label` in the observed data. Steps with these
    functions are dropped, not forced into a fabricated mapping.
  - A step is skipped entirely if `control_label` doesn't resolve to any
    element in that step's own `uia_controls_info` list (should not happen
    in clean data, but defensively not fabricated if it does).
"""
from __future__ import annotations

import json
import random
from dataclasses import dataclass
from pathlib import Path

from ..task import ACTIONS, CuaTask, OptionSpec
from .androidcontrol import assign_family

# Functions with no equivalent action in this benchmark, or no reliable
# on-screen target -- see module docstring's "Action-mapping decisions".
_UNMAPPED_FUNCTIONS = ("", "drag", "set_focus")

_SELECT_RANGE_FUNCTIONS = ("select_text", "select_paragraph", "select_table_range", "select_table")

_SELECT_CONTROL_TYPES = ("RadioButton", "ComboBox", "Spinner")

_MAX_ELEMENTS = 16


@dataclass
class UiaControl:
    label: int
    control_type: str
    control_text: str
    rect: tuple[int, int, int, int]  # left, top, right, bottom

    @property
    def w(self) -> int:
        return max(0, self.rect[2] - self.rect[0])

    @property
    def h(self) -> int:
        return max(0, self.rect[3] - self.rect[1])

    def role(self) -> str:
        if self.control_type == "Edit":
            return "Edit"
        if self.control_type == "CheckBox":
            return "CheckBox"
        if self.control_type in _SELECT_CONTROL_TYPES:
            return "Select"
        return "Button"

    def label_text(self) -> str:
        return (self.control_text or self.control_type or "").strip() or self.control_type


class UnmappedStep(Exception):
    """Raised when a step's action `function` has no equivalent action in
    this benchmark, or its `control_label` can't be resolved against that
    step's own uia_controls_info list."""


# ---------------------------------------------------------------------------
# Step 1: raw per-episode JSONL iteration
# ---------------------------------------------------------------------------

def iter_episode_steps(jsonl_path: str | Path) -> list[dict]:
    """Reads one GUI-360 per-episode JSONL file and returns its raw step
    records (as dicts, straight off the wire) sorted by `step_id`."""
    records = []
    with Path(jsonl_path).open(encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                records.append(json.loads(line))
    records.sort(key=lambda r: r.get("step_id", 0))
    return records


# ---------------------------------------------------------------------------
# Step 2: uia_controls_info -> flat element list
# ---------------------------------------------------------------------------

def parse_uia_controls(uia_controls_info: list[dict]) -> list[UiaControl]:
    out = []
    for c in uia_controls_info:
        rect = c.get("control_rect") or [0, 0, 0, 0]
        if len(rect) != 4:
            continue
        try:
            label = int(c.get("label"))
        except (TypeError, ValueError):
            continue
        out.append(UiaControl(
            label=label,
            control_type=c.get("control_type") or "",
            control_text=c.get("control_text") or "",
            rect=tuple(int(v) for v in rect),
        ))
    return out


def interactable_elements(controls: list[UiaControl]) -> list[UiaControl]:
    return [c for c in controls if c.w > 0 and c.h > 0]


# ---------------------------------------------------------------------------
# Step 3-4: gold action -> (target control, action)
# ---------------------------------------------------------------------------

def map_action(action: dict, controls: list[UiaControl]) -> tuple[UiaControl, str, str | None]:
    """Returns (target_control, action, gold_text_for_fill_or_None)."""
    fn = action.get("function") or ""
    if fn in _UNMAPPED_FUNCTIONS:
        raise UnmappedStep(f"function {fn!r} has no on-screen mappable target")

    raw_label = action.get("control_label")
    try:
        label = int(raw_label)
    except (TypeError, ValueError):
        raise UnmappedStep(f"function {fn!r}: no resolvable control_label ({raw_label!r})")

    by_label = {c.label: c for c in controls}
    target = by_label.get(label)
    if target is None:
        raise UnmappedStep(f"control_label {label} not found among this step's uia_controls_info")

    if fn == "click":
        if target.control_type == "CheckBox":
            return target, "check", None
        if target.control_type in _SELECT_CONTROL_TYPES:
            return target, "select", None
        return target, "click", None
    if fn == "type":
        text = action.get("args", {}).get("text")
        if text is None:
            raise UnmappedStep("type: no args.text")
        return target, "fill", text
    if fn in _SELECT_RANGE_FUNCTIONS:
        return target, "select", None
    if fn == "wheel_mouse_input":
        return target, "scroll", None
    raise UnmappedStep(f"function {fn!r} not handled")


# ---------------------------------------------------------------------------
# Step 5: element list + option set -> CuaTask
# ---------------------------------------------------------------------------

def _build_elements_and_options(
    rng: random.Random,
    all_controls: list[UiaControl],
    target: UiaControl,
    cua_action: str,
    gold_text: str | None,
    hard_distractor: bool = False,
) -> tuple[list[dict], list[OptionSpec], dict[str, str], list[dict]]:
    others = [c for c in all_controls if c.label != target.label]
    rng.shuffle(others)
    kept_others = others[: max(0, _MAX_ELEMENTS - 1)]
    ordered = [target] + kept_others
    rng.shuffle(ordered)

    elements: list[dict] = []
    options: list[OptionSpec] = []
    expected: dict[str, str] = {}
    entities: list[dict] = []
    entity_id = None
    if cua_action == "fill":
        entity_id = "ent_0"
        entities.append({"id": entity_id, "label": target.label_text(), "value": gold_text or ""})

    # Hard-distractor follow-up -- see androidcontrol.py's identical note and
    # generator.py's `_add_hard_distractor` for the design rationale: on real
    # GUI-360 desktop screens, ribbons/toolbars are full of same-role controls
    # with near-identical labels (e.g. "Cut"/"Copy"/"Paste"), so a genuine
    # lexical near-miss decoy is picked from `ordered` and given the SAME real
    # non-skip option as the gold target, gold still "skip" on it.
    hard_distractor_label = None
    if hard_distractor and len(ordered) > 1:
        same_role = [c for c in ordered if c.label != target.label and c.role() == target.role()]
        pool = same_role or [c for c in ordered if c.label != target.label]
        if pool:
            import difflib
            best = max(pool, key=lambda c: difflib.SequenceMatcher(None, c.label_text().lower(), target.label_text().lower()).ratio())
            hard_distractor_label = best.label

    for i, ctrl in enumerate(ordered):
        eid = f"el_{i}"
        elements.append({
            "id": eid, "role": ctrl.role(), "label": ctrl.label_text(),
            "frame": list(ctrl.rect),
        })
        options.append(OptionSpec(element_id=eid, role=ctrl.role(), label=ctrl.label_text(), action="skip"))
        if ctrl.label == target.label:
            options.append(OptionSpec(element_id=eid, role=ctrl.role(), label=ctrl.label_text(),
                                       action=cua_action, entity_id=entity_id))
            expected[eid] = cua_action
        elif ctrl.label == hard_distractor_label:
            options.append(OptionSpec(element_id=eid, role=ctrl.role(), label=ctrl.label_text(),
                                       action=cua_action, entity_id=entity_id))
            expected[eid] = "skip"
        else:
            expected[eid] = "skip"
    return elements, options, expected, entities


def convert_step(
    *,
    episode_id: str,
    step_idx: int,
    request: str,
    subtask: str,
    screenshot_path: Path | None,
    uia_controls_info: list[dict],
    action: dict,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("text", "multimodal"),
    seed: int = 0,
    app_domain: str = "",
    hard_distractor: bool = False,
) -> CuaTask:
    """Raises UnmappedStep if this step can't be converted (see module docstring)."""
    controls = interactable_elements(parse_uia_controls(uia_controls_info))
    if not controls:
        raise UnmappedStep("no interactable UIA controls on this step")
    target, cua_action, gold_text = map_action(action, controls)
    if cua_action not in ACTIONS:
        raise UnmappedStep(f"mapped action {cua_action!r} not in this benchmark's ACTIONS")

    use_multimodal = "multimodal" in modality_available and screenshot_path is not None and screenshot_path.exists()
    if "multimodal" in modality_available and not use_multimodal:
        raise UnmappedStep("multimodal requested but no real screenshot file available")

    rng = random.Random(seed)
    el_list, options, expected, entities = _build_elements_and_options(
        rng, controls, target, cua_action, gold_text, hard_distractor=hard_distractor)

    task_id = f"gui360_{episode_id}_{step_idx}"
    screenshot_rel = None
    ax_tree_text = None
    if use_multimodal:
        out_dir.mkdir(parents=True, exist_ok=True)
        dest = out_dir / f"{task_id}.png"
        dest.write_bytes(screenshot_path.read_bytes())
        screenshot_rel = str(dest)
    if "text" in modality_available:
        lines = [f"- {e['role']} \"{e['label']}\" @ {e['frame']}" for e in el_list]
        ax_tree_text = "\n".join(lines)

    family = assign_family(cua_action, request, subtask, target.label_text())

    return CuaTask(
        id=task_id,
        family=family,
        app=f"gui360_{app_domain}",
        modality_available=list(modality_available),
        screenshot=screenshot_rel,
        ax_tree=ax_tree_text,
        ax_tree_source="real",
        elements=el_list,
        elements_source="accessibility_api",
        entities=entities,
        options=options,
        expected=expected,
        split="public",
        group=None,
        provenance={
            "source": "GUI-360",
            "source_url": "https://huggingface.co/datasets/vyokky/GUI-360",
            "license": "MIT",
            "episode_id": episode_id,
            "step_idx": step_idx,
            "app_domain": app_domain,
            "request": request,
            "subtask": subtask,
            "raw_action": json.dumps(action),
        },
    )


def convert_episode(
    jsonl_path: str | Path,
    *,
    images_root: Path,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("text", "multimodal"),
    max_steps_per_episode: int = 5,
    hard_distractor: bool = False,
) -> list[CuaTask]:
    """Converts up to `max_steps_per_episode` resolvable steps of one raw
    GUI-360 per-episode JSONL file into CuaTasks. Steps that don't map
    cleanly are silently skipped (see UnmappedStep call sites) -- returns
    however many it could honestly convert, which may be zero.

    `images_root` is the directory extracted screenshots were placed under,
    containing paths of the shape
    `<app_domain>/in_app/success/<execution_id>/action_step<N>.png` -- the
    same relative layout GUI-360's own `screenshot_clean` field encodes
    (minus its `success/` prefix, which this integration folds into the
    per-episode subdirectory already implied by `execution_id`).
    """
    steps = iter_episode_steps(jsonl_path)
    if not steps:
        return []
    episode_id = steps[0].get("execution_id", Path(jsonl_path).stem)
    app_domain = steps[0].get("app_domain", "")
    request = steps[0].get("request", "")

    tasks: list[CuaTask] = []
    for rec in steps:
        if len(tasks) >= max_steps_per_episode:
            break
        step = rec.get("step", {})
        action = step.get("action", {})
        control_infos = step.get("control_infos", {})
        uia_controls_info = control_infos.get("uia_controls_info", [])
        subtask = step.get("subtask", "")
        screenshot_clean = step.get("screenshot_clean")  # e.g. "success/excel_1_2/action_step1.png"
        screenshot_path = None
        if screenshot_clean:
            rel = screenshot_clean.split("/", 1)[-1] if screenshot_clean.startswith("success/") else screenshot_clean
            screenshot_path = images_root / app_domain / "in_app" / "success" / rel
        try:
            task = convert_step(
                episode_id=episode_id, step_idx=rec.get("step_id", 0), request=request, subtask=subtask,
                screenshot_path=screenshot_path, uia_controls_info=uia_controls_info, action=action,
                out_dir=out_dir, modality_available=modality_available,
                seed=hash((episode_id, rec.get("step_id", 0))) & 0xFFFFFFFF, app_domain=app_domain,
                hard_distractor=hard_distractor,
            )
            tasks.append(task)
        except UnmappedStep:
            continue
    return tasks
