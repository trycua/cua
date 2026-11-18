"""AndroidControl (Google Research, Apache-2.0) -> CuaTask converter.

Source: https://github.com/google-research/google-research/tree/master/android_control
Canonical data: gs://gresearch/android_control/ (TFRecord/GZIP). This module
reads a community mirror of the SAME canonical TFRecords re-uploaded to the
Hugging Face Hub (`leosltl/Android-Control`) -- verified against the official
README's documented schema (field names, action-type vocabulary, license)
before use. A second mirror (`smolagents/android-control`, parquet) was
inspected and rejected for this integration because it drops the
`accessibility_trees` field entirely (screenshots + actions only) -- this
converter's whole reason for wanting AndroidControl is that it has BOTH
modalities per step, so a mirror missing one defeats the point.

What this module does, end to end:
  1. Reads raw TFRecord episodes (`tfrecord` package, gzip-compressed).
  2. Decodes each step's `accessibility_trees[i]` -- a serialized
     `android_env.AndroidAccessibilityForest` proto -- with `_pbwire`'s
     generic wire-format reader (see that module's docstring for why: no
     `protoc` toolchain dependency, and the official path requires installing
     the `android_env` package's generated bindings). Field numbers below are
     copied verbatim from the public .proto files at
     github.com/google-deepmind/android_env/tree/main/android_env/proto/a11y.
  3. Flattens the forest into this benchmark's flat `elements` format,
     keeping only nodes that are visible AND (clickable OR checkable OR
     editable) -- i.e., nodes a GUI-action model could plausibly act on.
  4. Maps AndroidControl's one-gold-action-per-step JSON onto this
     benchmark's action vocabulary (fill/check/click/select/scroll/skip),
     resolving the action's screen coordinates (or focus state, for
     input_text) against the elements list to find which element the gold
     action targets.
  5. Builds a small option set per converted step: every element gets a
     `skip` option; the resolved gold element additionally gets the mapped
     gold action (+ entity pointer, for fill) -- mirrors the synthetic
     generator's own "small candidate set" design in `generator.py`, not a
     combinatorial cross-join.
  6. Assigns one of this benchmark's task families by keyword/action-type
     heuristic (documented per-rule below).

Action-mapping decisions (the parts that do NOT map cleanly):
  - `input_text` -> `fill`. AndroidControl's `input_text` has no target
    coordinates (it types into whatever is currently focused). We resolve
    the target element as the unique `is_editable` node with `is_focused =
    true` in the BEFORE-action tree; if none is focused (happens when the
    focus flag wasn't set by the on-device recorder) but exactly one
    editable node exists on screen, we fall back to that. Steps where
    neither holds are skipped (not fabricated) -- see `_resolve_input_text`.
  - `click` -> `check` if the resolved element `is_checkable`, else `select`
    if its class name suggests a spinner/dropdown, else `click`. This is a
    heuristic: AndroidControl has no distinct "select" action type at all
    (choosing a dropdown item is just two `click`s in the raw data), so this
    benchmark's `select` action is under-represented in the converted set by
    construction -- documented, not invented data.
  - `long_press` -> treated the same as `click` (this benchmark has no
    long-press action; this is a lossy but harmless collapse since both
    require choosing the same target element).
  - `scroll` -> `scroll`, targeting the largest `is_scrollable` node on
    screen (AndroidControl's scroll action carries a direction but no
    target element; this benchmark's schema needs one).
  - `open_app`, `navigate_home`, `navigate_back`, `wait` -> NOT converted.
    These are episode/system-level actions with no on-screen target element
    at all, so they cannot be expressed as a (element, action) pair under
    this benchmark's schema. Steps with these action types are dropped, not
    forced into a fabricated mapping -- a real, honest gap, not silently
    absorbed.
  - A step is skipped entirely (contributes zero tasks) if the target
    element can't be resolved with reasonable confidence (no node contains
    the click point and none is within `_CLICK_SNAP_PX` of it; no focused-or-
    unique editable node for input_text; no scrollable node for scroll).
"""
from __future__ import annotations

import json
import random
from dataclasses import dataclass
from pathlib import Path

from ..task import ACTIONS, CuaTask, OptionSpec
from . import _pbwire as pb

# How close (px, in screenshot coordinate space) a click must be to a node's
# bounding box to "snap" to it when no node's bounds directly contain the
# click point (device coordinate rounding / recorder jitter).
_CLICK_SNAP_PX = 24

_SPINNER_HINTS = ("spinner", "dropdown", "combobox")

_LOGIN_WORDS = ("log in", "login", "sign in", "signin", "password", "username", "sign-in")
_SEARCH_WORDS = ("search", "filter", "find ", "look up", "browse")
_SUBMIT_WORDS = ("submit", "confirm", "save", "done", "continue", "next", "finish", "checkout", "pay")


# ---------------------------------------------------------------------------
# Step 1: raw TFRecord episode iteration
# ---------------------------------------------------------------------------

def iter_episodes(tfrecord_path: str | Path, limit: int | None = None):
    """Yields raw episode dicts straight out of the TFRecord file (numpy
    arrays / bytes, AndroidControl's native shapes) -- no conversion yet."""
    from tfrecord.reader import tfrecord_loader
    n = 0
    for record in tfrecord_loader(str(tfrecord_path), None, compression_type="gzip"):
        yield record
        n += 1
        if limit is not None and n >= limit:
            return


# ---------------------------------------------------------------------------
# Step 2-3: accessibility forest -> flat element list
# ---------------------------------------------------------------------------

@dataclass
class A11yNode:
    unique_id: int
    left: int
    top: int
    right: int
    bottom: int
    class_name: str
    content_description: str
    hint_text: str
    text: str
    view_id_resource_name: str
    is_checkable: bool
    is_checked: bool
    is_clickable: bool
    is_editable: bool
    is_enabled: bool
    is_focused: bool
    is_scrollable: bool
    is_visible_to_user: bool

    @property
    def w(self) -> int:
        return max(0, self.right - self.left)

    @property
    def h(self) -> int:
        return max(0, self.bottom - self.top)

    @property
    def area(self) -> int:
        return self.w * self.h

    def contains(self, x: float, y: float) -> bool:
        return self.left <= x <= self.right and self.top <= y <= self.bottom

    def label(self) -> str:
        for cand in (self.text, self.content_description, self.hint_text):
            if cand and cand.strip():
                return cand.strip()
        if self.view_id_resource_name:
            return self.view_id_resource_name.split("/")[-1].replace("_", " ")
        return self.class_name.rsplit(".", 1)[-1]

    def role(self) -> str | None:
        if self.is_editable:
            return "Edit"
        if self.is_checkable:
            return "CheckBox"
        if any(h in self.class_name.lower() for h in _SPINNER_HINTS):
            return "Select"
        if self.is_clickable:
            return "Button"
        return None


def _parse_rect(raw: bytes | None) -> tuple[int, int, int, int]:
    if raw is None:
        return (0, 0, 0, 0)
    f = pb.parse_message(raw)
    return (pb.get_int(f, 1), pb.get_int(f, 2), pb.get_int(f, 3), pb.get_int(f, 4))


def _parse_node(raw: bytes) -> A11yNode:
    f = pb.parse_message(raw)
    left, top, right, bottom = _parse_rect(pb.get_msg(f, 2))
    return A11yNode(
        unique_id=pb.get_int(f, 1),
        left=left, top=top, right=right, bottom=bottom,
        class_name=pb.get_str(f, 3),
        content_description=pb.get_str(f, 4),
        hint_text=pb.get_str(f, 5),
        text=pb.get_str(f, 7),
        view_id_resource_name=pb.get_str(f, 10),
        is_checkable=pb.get_bool(f, 12),
        is_checked=pb.get_bool(f, 13),
        is_clickable=pb.get_bool(f, 14),
        is_editable=pb.get_bool(f, 15),
        is_enabled=pb.get_bool(f, 16),
        is_focused=pb.get_bool(f, 18),
        is_scrollable=pb.get_bool(f, 21),
        is_visible_to_user=pb.get_bool(f, 23),
    )


def parse_forest(raw: bytes) -> list[A11yNode]:
    """AndroidAccessibilityForest -> flat list of every node across every
    window's tree (field numbers per the upstream .proto, see module docstring)."""
    forest_fields = pb.parse_message(raw)
    nodes: list[A11yNode] = []
    for window_raw in pb.get_repeated_msg(forest_fields, 1):  # Forest.windows = 1
        window_fields = pb.parse_message(window_raw)
        tree_raw = pb.get_msg(window_fields, 11)  # Window.tree = 11
        if tree_raw is None:
            continue
        tree_fields = pb.parse_message(tree_raw)
        for node_raw in pb.get_repeated_msg(tree_fields, 1):  # Tree.nodes = 1
            try:
                nodes.append(_parse_node(node_raw))
            except (IndexError, ValueError):
                # Observed in practice: the very last node of the very last
                # window in a small fraction of episodes has its final field
                # cut a byte or two short of a complete tag+length+value --
                # consistent with an upstream capture-size cap on the forest
                # byte string rather than a bug in this reader. Dropped, not
                # guessed at: an element that's missing is a smaller elements
                # list, not a corrupted one.
                continue
    return nodes


def interactable_elements(nodes: list[A11yNode]) -> list[A11yNode]:
    return [n for n in nodes
            if n.is_visible_to_user and n.is_enabled and n.role() is not None
            and n.w > 0 and n.h > 0]


# ---------------------------------------------------------------------------
# Step 4: gold action -> (target element, action)
# ---------------------------------------------------------------------------

def _nearest(nodes: list[A11yNode], x: float, y: float, max_dist: float) -> A11yNode | None:
    best, best_d = None, max_dist
    for n in nodes:
        cx, cy = (n.left + n.right) / 2, (n.top + n.bottom) / 2
        d = ((cx - x) ** 2 + (cy - y) ** 2) ** 0.5
        if d < best_d:
            best, best_d = n, d
    return best


def _resolve_click_target(elements: list[A11yNode], x: float, y: float) -> A11yNode | None:
    containing = [n for n in elements if n.contains(x, y)]
    if containing:
        return min(containing, key=lambda n: n.area)
    return _nearest(elements, x, y, _CLICK_SNAP_PX)


def _resolve_input_text_target(elements: list[A11yNode]) -> A11yNode | None:
    edits = [n for n in elements if n.role() == "Edit"]
    focused = [n for n in edits if n.is_focused]
    if len(focused) == 1:
        return focused[0]
    if len(edits) == 1:
        return edits[0]
    return None


def _resolve_scroll_target(elements: list[A11yNode]) -> A11yNode | None:
    scrollable = [n for n in elements if n.is_scrollable]
    if not scrollable:
        return None
    return max(scrollable, key=lambda n: n.area)


class UnmappedStep(Exception):
    """Raised (and caught by the caller) when a step's action_type has no
    equivalent action in this benchmark, or its target element can't be
    resolved."""


def map_action(action: dict, elements: list[A11yNode]) -> tuple[A11yNode, str, str | None]:
    """Returns (target_element, action, gold_text_for_fill_or_None)."""
    at = action.get("action_type")
    if at in ("click", "long_press"):
        x, y = action.get("x"), action.get("y")
        if x is None or y is None:
            raise UnmappedStep(f"{at} missing coordinates")
        target = _resolve_click_target(elements, x, y)
        if target is None:
            raise UnmappedStep(f"no element resolves {at} at ({x},{y})")
        if target.role() == "CheckBox":
            return target, "check", None
        if target.role() == "Select":
            return target, "select", None
        return target, "click", None
    if at == "input_text":
        target = _resolve_input_text_target(elements)
        if target is None:
            raise UnmappedStep("input_text: no focused/unique editable element")
        return target, "fill", action.get("text") or ""
    if at == "scroll":
        target = _resolve_scroll_target(elements)
        if target is None:
            raise UnmappedStep("scroll: no scrollable element on screen")
        return target, "scroll", None
    raise UnmappedStep(f"action_type {at!r} has no on-screen target (episode-level action)")


# ---------------------------------------------------------------------------
# Step 6: family assignment (heuristic, documented)
# ---------------------------------------------------------------------------

def assign_family(cua_action: str, goal: str, step_instruction: str, label: str) -> str:
    text = f"{goal} {step_instruction} {label}".lower()
    if cua_action == "fill":
        return "form_filling"
    if cua_action == "check":
        return "consent_checkbox"
    if cua_action == "scroll":
        return "pagination"
    if cua_action == "select":
        return "search_filter"
    # cua_action == "click": disambiguate by keyword.
    if any(w in text for w in _LOGIN_WORDS):
        return "login_auth"
    if any(w in text for w in _SEARCH_WORDS):
        return "search_filter"
    if any(w in text for w in _SUBMIT_WORDS):
        return "multi_step_submit"
    # Fallback bucket for generic navigation clicks that don't cleanly match
    # any family: a real fraction of AndroidControl `click` steps are just
    # "tap the next screen element in a multi-app flow" with no consent/
    # search/login/submit semantics at all. Bucketed into multi_step_submit
    # as the closest existing fit (both are "one click among several,
    # mid-flow") rather than inventing a new family unilaterally.
    return "multi_step_submit"


# ---------------------------------------------------------------------------
# Step 3+5: element list + option set -> CuaTask
# ---------------------------------------------------------------------------

_MAX_ELEMENTS = 16


def _build_elements_and_options(
    rng: random.Random,
    all_elements: list[A11yNode],
    target: A11yNode,
    cua_action: str,
    gold_text: str | None,
    hard_distractor: bool = False,
) -> tuple[list[dict], list[OptionSpec], dict[str, str], list[dict]]:
    others = [n for n in all_elements if n.unique_id != target.unique_id]
    rng.shuffle(others)
    kept_others = others[: max(0, _MAX_ELEMENTS - 1)]
    ordered = [target] + kept_others
    rng.shuffle(ordered)  # gold position shouldn't be positionally predictable

    elements: list[dict] = []
    options: list[OptionSpec] = []
    expected: dict[str, str] = {}
    entities: list[dict] = []
    entity_id = None
    if cua_action == "fill":
        entity_id = "ent_0"
        entities.append({"id": entity_id, "label": target.label(), "value": gold_text or ""})

    # Hard-distractor follow-up (see generator.py's `_add_hard_distractor` for
    # the full design rationale, applied here to real AndroidControl screens):
    # among the SAME-role "others", pick whichever one has the most lexically
    # similar label to the gold `target` (the element a naive instruction/
    # label text-matcher would most likely confuse with the real one) and
    # give IT a real, plausible non-skip option too (same `cua_action`, same
    # `entity_id` when filling) -- gold stays "skip" on it. This makes the
    # option set carry a genuine near-miss on real app screens, not just a
    # manufactured never-right decoy.
    hard_distractor_id = None
    if hard_distractor and len(ordered) > 1:
        same_role = [n for n in ordered if n.unique_id != target.unique_id and n.role() == target.role()]
        pool = same_role or [n for n in ordered if n.unique_id != target.unique_id]
        if pool:
            import difflib
            best = max(pool, key=lambda n: difflib.SequenceMatcher(None, n.label().lower(), target.label().lower()).ratio())
            hard_distractor_id = best.unique_id

    for i, node in enumerate(ordered):
        eid = f"el_{i}"
        elements.append({
            "id": eid, "role": node.role(), "label": node.label(),
            "frame": [node.left, node.top, node.right, node.bottom],
        })
        options.append(OptionSpec(element_id=eid, role=node.role(), label=node.label(), action="skip"))
        if node.unique_id == target.unique_id:
            options.append(OptionSpec(element_id=eid, role=node.role(), label=node.label(),
                                       action=cua_action, entity_id=entity_id))
            expected[eid] = cua_action
        elif node.unique_id == hard_distractor_id:
            options.append(OptionSpec(element_id=eid, role=node.role(), label=node.label(),
                                       action=cua_action, entity_id=entity_id))
            expected[eid] = "skip"
        else:
            expected[eid] = "skip"
    return elements, options, expected, entities


def convert_step(
    *,
    episode_id: int,
    step_idx: int,
    goal: str,
    step_instruction: str,
    screenshot_png: bytes,
    forest_raw: bytes,
    action: dict,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("text", "multimodal"),
    seed: int = 0,
    hard_distractor: bool = False,
) -> CuaTask:
    """Raises UnmappedStep if this step can't be converted (see module docstring).
    `hard_distractor`: see `_build_elements_and_options`'s note -- adds one
    plausible same-role/near-label decoy option per task, gold=skip."""
    nodes = parse_forest(forest_raw)
    elements = interactable_elements(nodes)
    if not elements:
        raise UnmappedStep("no interactable elements in accessibility tree")
    target, cua_action, gold_text = map_action(action, elements)
    if cua_action not in ACTIONS:
        raise UnmappedStep(f"mapped action {cua_action!r} not in this benchmark's ACTIONS")

    rng = random.Random(seed)
    el_list, options, expected, entities = _build_elements_and_options(
        rng, elements, target, cua_action, gold_text, hard_distractor=hard_distractor)

    task_id = f"androidcontrol_{episode_id}_{step_idx}"
    screenshot_rel = None
    ax_tree_text = None
    if "multimodal" in modality_available:
        out_dir.mkdir(parents=True, exist_ok=True)
        shot_path = out_dir / f"{task_id}.png"
        shot_path.write_bytes(screenshot_png)
        screenshot_rel = str(shot_path)
    if "text" in modality_available:
        lines = [f"- {e['role']} \"{e['label']}\" @ {e['frame']}" for e in el_list]
        ax_tree_text = "\n".join(lines)

    family = assign_family(cua_action, goal, step_instruction, target.label())

    return CuaTask(
        id=task_id,
        family=family,
        app=f"androidcontrol_ep{episode_id}",
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
            "source": "AndroidControl",
            "source_url": "gs://gresearch/android_control/",
            "mirror": "leosltl/Android-Control (HF)",
            "license": "Apache-2.0",
            "episode_id": episode_id,
            "step_idx": step_idx,
            "goal": goal,
            "step_instruction": step_instruction,
            "raw_action": json.dumps(action),
        },
    )


def convert_episode(
    episode: dict,
    *,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("text", "multimodal"),
    max_steps_per_episode: int = 3,
    hard_distractor: bool = False,
) -> list[CuaTask]:
    """Converts up to `max_steps_per_episode` resolvable steps of one raw
    AndroidControl episode dict (as yielded by `iter_episodes`) into
    CuaTasks. Steps that don't map cleanly are silently skipped (see
    UnmappedStep call sites) -- this function returns however many it could
    honestly convert, which may be zero."""
    def _as_list(v):
        # tfrecord/numpy squeezes length-1 repeated fields to bare scalars;
        # normalize everything to a plain list so indexing is uniform.
        if isinstance(v, (bytes, str)):
            return [v]
        return list(v)

    episode_id = int(episode["episode_id"][0]) if hasattr(episode["episode_id"], "__len__") else int(episode["episode_id"])
    goal = episode["goal"] if isinstance(episode["goal"], str) else episode["goal"].decode("utf-8", "replace")
    screenshots = _as_list(episode["screenshots"])
    trees = _as_list(episode["accessibility_trees"])
    actions = _as_list(episode["actions"])
    step_instructions = _as_list(episode["step_instructions"])

    tasks: list[CuaTask] = []
    for i in range(len(actions)):
        if len(tasks) >= max_steps_per_episode:
            break
        try:
            action = json.loads(actions[i])
            step_instr = step_instructions[i]
            step_instr = step_instr if isinstance(step_instr, str) else step_instr.decode("utf-8", "replace")
            task = convert_step(
                episode_id=episode_id, step_idx=i, goal=goal, step_instruction=step_instr,
                screenshot_png=bytes(screenshots[i]), forest_raw=bytes(trees[i]), action=action,
                out_dir=out_dir, modality_available=modality_available, seed=episode_id * 100 + i,
                hard_distractor=hard_distractor,
            )
            tasks.append(task)
        except UnmappedStep:
            continue
    return tasks
