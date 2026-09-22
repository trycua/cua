"""`cua-bench` live GUI environments -> `CuaTask` converter for the trained-on
`cua_bench_basic` family.

## What this bridges

`libs/cua-bench` (package `cua_bench`) is a real, live, steppable
computer-use environment SDK in this same repository. Its bundled
`datasets/cua-bench-basic/` dataset holds 13 self-contained single-widget
task environments (`click-button`, `click-icon`, `color-picker`,
`date-picker`, `drag-drop`, `drag-slider`, `fill-form`, `right-click-menu`,
`select-dropdown`, `spreadsheet-cell`, `toggle-switch`, `typing-input`,
`video-player`), each a synthetic Tailwind-HTML widget UI with its own
`tasks_config` parameterizations, its own scripted reference solution
(`@cb.solve_task`) and its own reward function (`@cb.evaluate_task`).

This module drives those environments **for real** -- `cua_bench.core.make()`
-> `env.reset()` -> `env.solve()` -- and records, at every real step of the
real oracle rollout:

  - the real screenshot of the desktop at that moment,
  - the real interactive-element list of the live DOM (queried out of the
    running page, with real `getBoundingClientRect()` boxes translated into
    the same screen-pixel space the screenshot and the actions use),
  - the real action the reference solution takes next,
  - the real action history so far in the episode.

Each such step becomes one `CuaTask`.

## Option-set design (why one pseudo-element per candidate action)

`CuaTask` scores `(element, action)` pairs over the closed taxonomy
`fill/check/click/select/scroll/skip`, which cannot express cua-bench's real
action space (`TypeAction`, `DragAction`, `RightClickAction`,
`DoubleClickAction`, `KeyAction`, ...). So this converter uses the same shape
`datagen/chess_gym.py` already established for a non-form action space: each
*candidate concrete action* is its own option element, and every element gets
a real `{click, skip}` pair where `click` means "take this action next".
`expected` marks exactly one element `click` (the oracle's real next action)
and every other `skip`. A model must therefore discriminate the single
correct action against a full set of real, present, plausible alternatives --
it cannot win by spotting the one element that has more than one option.

## Where the distractors come from (all real, none fabricated)

Every distractor is a real action that a real agent could really take in that
exact state:

1. **Cross-parameterization oracle actions.** The same env's *other* real
   `tasks_config` parameterizations are rolled out too; the action the oracle
   really takes at the same step index in one of those episodes is used as a
   distractor here. For `color-picker` that is a real click on a different
   real color swatch; for `typing-input` a real `TypeAction` with a different
   real target string; for `drag-slider` a real drag to a different real
   slider position. These are the hardest possible honest distractors: they
   are gold somewhere, just not here. Borrowing is done **within a split
   only**, so a test task never draws a distractor from a train
   parameterization (or vice versa).
2. **Other real interactive elements on the same live page.** Clicks at the
   real centers of the other real interactive elements the live DOM actually
   contains at that step, preferring the same role as the gold target.
3. **Same target, different action type.** e.g. `DoubleClickAction` or
   `RightClickAction` at the gold click's own real coordinates -- a real,
   plausible confusion over the real action space.

No random-noise option is ever emitted.

## Modalities

Both are offered and each is fed only its own real representation:
  - `multimodal`: the real PNG screenshot of the live desktop at that step.
  - `text`: an accessibility-tree-style rendering (`ax_tree`) built from the
    live DOM element query -- real tags/roles/labels/boxes -- plus the real
    task instruction and the real action history so far in the episode (the
    correct next action is history-dependent, so omitting the history would
    make several steps genuinely unanswerable).

`ax_tree_source` is reported as `"synthetic"` and `elements_source` as
`"synthetic_spec"`: the element data is real and live, but it comes from a
DOM query, not from a platform accessibility API, and the schema's existing
enums have no value for "live DOM query". `provenance["ax_tree_method"]`
states exactly what it is.

## Honest limitations: the `simulated` provider

The bundled task envs declare `provider: "native"` (a Docker/QEMU desktop).
For CI-friendly, GPU-free generation this module can override that to
cua-bench's own `simulated` (Playwright) provider -- the same provider
cua-bench's own test suite uses. Two real consequences, both handled and
disclosed rather than papered over:

**1. Window sizing.** That provider renders its window-content iframe about
150px tall regardless of the height the env's own `launch_window(...)` asked
for, clipping most of every one of these pages (all 13 request 256-500px).
`fit_window_layout` (applied by default) resizes the provider's own
container iframe to the requested height; it touches the provider's desktop
chrome only, never the task page. Without it, `click-button`'s own reference
solution passes or fails depending on where a randomized button lands, and
`video-player` yields no usable steps at all. Every task discloses
`provenance["window_layout_fitted"]`. Elements still outside the window's
real visible rect are excluded from the option set, and a step whose own
gold target is off-screen is dropped rather than emitted as an unanswerable
task (see `convert_episode`'s `drop_offscreen_gold`).

**2. Reward reachability.** Even with the layout corrected, only 7 of the 13
envs' reference solutions earn full reward under `simulated`. Measured over
every real parameterization: `click-button`, `click-icon`, `color-picker`,
`right-click-menu`, `spreadsheet-cell`, `toggle-switch` and `typing-input`
score 1.0 on every parameterization; `date-picker`, `drag-drop`,
`fill-form`, `select-dropdown` and `video-player` score 0.0 on every one,
and `drag-slider` on 4 of 5 -- because that provider cannot actuate native
`<select>` popups, HTML5 drag-and-drop, native date inputs or `<video>`
playback. The recorded actions are still the real reference solution's real
actions; only the end-of-episode reward differs. Every episode records its
real `evaluate()` reward, and every task carries
`provenance["oracle_reward"]` and `provenance["oracle_verified"]`. Pass
`require_oracle_success=True` to emit tasks only from episodes whose oracle
really earned reward, or `provider="native"` to generate against the
providers the envs actually declare.

## Usage

```python
import asyncio
from pathlib import Path
from cua_bench_s1.datagen.cua_bench_basic import generate_dataset
from cua_bench_s1.task import save_jsonl

splits = asyncio.run(generate_dataset(
    dataset_dir=Path("libs/cua-bench/datasets/cua-bench-basic"),
    out_dir=Path("out/cua_bench_basic/images"),
))
for name, tasks in splits.items():
    save_jsonl(tasks, f"out/cua_bench_basic/{name}.jsonl")
```

For raw multi-step agentic rollouts (RL) against these same 13 envs -- no
closed option set -- see `cua_bench_s1.agentic.cua_bench_basic_env`.
"""
from __future__ import annotations

import random
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

from ..task import CuaTask, OptionSpec

FAMILY = "cua_bench_basic"

#: The 13 bundled `cua-bench-basic` task environment directory names.
ENV_NAMES = (
    "click-button",
    "click-icon",
    "color-picker",
    "date-picker",
    "drag-drop",
    "drag-slider",
    "fill-form",
    "right-click-menu",
    "select-dropdown",
    "spreadsheet-cell",
    "toggle-switch",
    "typing-input",
    "video-player",
)

#: Envs whose bundled reference solution does NOT earn reward under the
#: `simulated` (Playwright) provider, as really measured over every
#: parameterization -- see the module docstring. Recorded for disclosure only;
#: nothing branches on it (each task's own real `oracle_reward` is what
#: `require_oracle_success` filters on).
SIMULATED_PROVIDER_UNVERIFIED = (
    "date-picker",
    "drag-drop",
    "drag-slider",   # 4 of its 5 parameterizations
    "fill-form",
    "select-dropdown",
    "video-player",
)

#: JS run inside the live task window to enumerate the real interactive
#: elements and their real bounding boxes (window space; translated to screen
#: space by the recorder). Deliberately a plain DOM query -- no heuristics
#: beyond "these tags/roles are interactive".
INTERACTIVE_ELEMENTS_JS = """
(() => {
  const sel = 'button, input, select, textarea, a[href], [role="button"],' +
              '[role="menuitem"], [onclick], [draggable="true"], [contenteditable="true"]';
  const out = [];
  document.querySelectorAll(sel).forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return;
    const style = window.getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none') return;
    const label = (el.getAttribute('aria-label') || el.innerText || el.value ||
                   el.getAttribute('placeholder') || el.getAttribute('title') || '')
                  .toString().trim().slice(0, 80);
    out.push({
      tag: el.tagName,
      type: el.getAttribute('type'),
      dom_id: el.id || null,
      role: el.getAttribute('role'),
      label: label,
      x: Math.round(r.x), y: Math.round(r.y),
      width: Math.round(r.width), height: Math.round(r.height),
    });
  });
  return out;
})()
"""

_ROLE_BY_TAG = {
    "BUTTON": "Button",
    "SELECT": "Select",
    "TEXTAREA": "Edit",
    "A": "Link",
    "INPUT": "Edit",
}
_ROLE_BY_INPUT_TYPE = {
    "checkbox": "CheckBox",
    "radio": "RadioButton",
    "range": "Slider",
    "color": "ColorPicker",
    "date": "DateEdit",
    "submit": "Button",
    "button": "Button",
}


@dataclass
class DomElement:
    """One real interactive element of the live page, in screen-pixel space
    (the same space the screenshot and every cua-bench action use)."""

    id: str
    role: str
    label: str
    frame: list[int]  # [x0, y0, x1, y1], screen space
    tag: str
    dom_id: str | None = None

    @property
    def center(self) -> tuple[int, int]:
        x0, y0, x1, y1 = self.frame
        return (x0 + x1) // 2, (y0 + y1) // 2

    def contains(self, x: float, y: float) -> bool:
        x0, y0, x1, y1 = self.frame
        return x0 <= x <= x1 and y0 <= y <= y1


def role_for(tag: str, input_type: str | None, aria_role: str | None) -> str:
    if aria_role:
        return aria_role.capitalize()
    if tag == "INPUT" and input_type:
        return _ROLE_BY_INPUT_TYPE.get(input_type.lower(), "Edit")
    return _ROLE_BY_TAG.get(tag, "Generic")


def dom_elements_from_query(raw: list[dict], *, offset_x: int, offset_y: int) -> list[DomElement]:
    """Translates the raw `INTERACTIVE_ELEMENTS_JS` result (window-space
    boxes) into screen-space `DomElement`s. `offset_x`/`offset_y` are the real
    measured offset of the task window's content area on the desktop."""
    els: list[DomElement] = []
    for i, r in enumerate(raw):
        x0 = int(r["x"]) + offset_x
        y0 = int(r["y"]) + offset_y
        els.append(
            DomElement(
                id=f"dom_{i}",
                role=role_for(r.get("tag", ""), r.get("type"), r.get("role")),
                label=r.get("label") or (r.get("dom_id") or r.get("tag", "")),
                frame=[x0, y0, x0 + int(r["width"]), y0 + int(r["height"])],
                tag=r.get("tag", ""),
                dom_id=r.get("dom_id"),
            )
        )
    return els


# --------------------------------------------------------------------------
# Actions
# --------------------------------------------------------------------------


@dataclass
class ActionRecord:
    """One real cua-bench action, stored structurally so it can be compared,
    re-instantiated and described without importing `cua_bench`."""

    kind: str                       # "ClickAction" | "TypeAction" | "DragAction" | ...
    params: dict[str, Any] = field(default_factory=dict)

    def key(self) -> str:
        items = ",".join(f"{k}={self.params[k]!r}" for k in sorted(self.params))
        return f"{self.kind}({items})"

    def point(self) -> tuple[float, float] | None:
        if "x" in self.params and "y" in self.params:
            return float(self.params["x"]), float(self.params["y"])
        if "from_x" in self.params:
            return float(self.params["from_x"]), float(self.params["from_y"])
        return None

    def describe(self, elements: list[DomElement]) -> str:
        """Human/model-readable label, grounded in the real element the action
        actually lands on where there is one."""
        target = target_element(self, elements)
        where = f' on {target.role} "{target.label}"' if target is not None else ""
        if self.kind in ("ClickAction", "RightClickAction", "DoubleClickAction", "MoveToAction"):
            verb = {
                "ClickAction": "click",
                "RightClickAction": "right-click",
                "DoubleClickAction": "double-click",
                "MoveToAction": "move to",
            }[self.kind]
            return f"{verb} at ({int(self.params['x'])}, {int(self.params['y'])}){where}"
        if self.kind == "TypeAction":
            return f"type {self.params.get('text','')!r}"
        if self.kind == "KeyAction":
            return f"press key {self.params.get('key','')!r}"
        if self.kind == "HotkeyAction":
            return f"hotkey {'+'.join(self.params.get('keys', []))}"
        if self.kind == "DragAction":
            return (
                f"drag from ({int(self.params['from_x'])}, {int(self.params['from_y'])}) "
                f"to ({int(self.params['to_x'])}, {int(self.params['to_y'])}){where}"
            )
        if self.kind == "ScrollAction":
            return f"scroll {self.params.get('direction','down')} by {self.params.get('amount', 1)}"
        if self.kind == "WaitAction":
            return f"wait {self.params.get('seconds', 1)}s"
        return self.kind.replace("Action", "").lower()


def action_from_obj(action: Any) -> ActionRecord:
    """Converts a live `cua_bench.types.*Action` dataclass instance into an
    `ActionRecord` without importing `cua_bench` (so the pure conversion side
    of this module stays importable with cua-bench absent)."""
    kind = type(action).__name__
    params = {k: v for k, v in vars(action).items() if not k.startswith("_")}
    return ActionRecord(kind=kind, params=params)


def target_element(action: ActionRecord, elements: list[DomElement]) -> DomElement | None:
    """The real element an action's real coordinates land inside. When boxes
    nest (a button inside its container), the smallest containing box wins --
    that is the element a real click would actually hit."""
    pt = action.point()
    if pt is None:
        return None
    hits = [e for e in elements if e.contains(*pt)]
    if not hits:
        return None
    return min(hits, key=lambda e: (e.frame[2] - e.frame[0]) * (e.frame[3] - e.frame[1]))


# --------------------------------------------------------------------------
# Recorded episodes (pure data -- no cua_bench import needed to use these)
# --------------------------------------------------------------------------


@dataclass
class RecordedStep:
    step_index: int
    elements: list[DomElement]
    gold_action: ActionRecord
    screenshot: str | None


@dataclass
class RecordedEpisode:
    env_name: str
    task_index: int
    instruction: str
    metadata: dict
    steps: list[RecordedStep]
    oracle_reward: float | None
    provider: str
    #: The real visible content rect of the task window, in screen space
    #: ([x0,y0,x1,y1]). The bundled pages can overflow their own window, so
    #: some real DOM elements -- occasionally including the reference
    #: solution's own target -- are genuinely not on screen; see
    #: `visible_elements` / `is_visible`.
    viewport: list[int] | None = None
    #: Whether `fit_window_layout` was applied to this episode's session.
    window_layout_fitted: bool = False

    @property
    def oracle_verified(self) -> bool:
        return self.oracle_reward is not None and self.oracle_reward >= 0.5

    def is_visible(self, box: list[int]) -> bool:
        if self.viewport is None:
            return True
        vx0, vy0, vx1, vy1 = self.viewport
        x0, y0, x1, y1 = box
        return x0 < vx1 and x1 > vx0 and y0 < vy1 and y1 > vy0

    def contains_point(self, x: float, y: float) -> bool:
        if self.viewport is None:
            return True
        vx0, vy0, vx1, vy1 = self.viewport
        return vx0 <= x <= vx1 and vy0 <= y <= vy1

    def visible_elements(self, step: RecordedStep) -> list[DomElement]:
        return [e for e in step.elements if self.is_visible(e.frame)]


# --------------------------------------------------------------------------
# Candidate (option) construction
# --------------------------------------------------------------------------


def _same_target_variants(gold: ActionRecord) -> list[ActionRecord]:
    """Real same-target, different-action-type confusions over cua-bench's own
    real action space."""
    if gold.kind == "ClickAction":
        return [
            ActionRecord("DoubleClickAction", dict(gold.params)),
            ActionRecord("RightClickAction", dict(gold.params)),
        ]
    if gold.kind == "RightClickAction":
        return [ActionRecord("ClickAction", dict(gold.params))]
    if gold.kind == "DoubleClickAction":
        return [ActionRecord("ClickAction", dict(gold.params))]
    return []


def _element_click_distractors(
    gold: ActionRecord, elements: list[DomElement], gold_target: DomElement | None
) -> list[ActionRecord]:
    """Clicks at the real centers of the other real interactive elements
    present on the page at this step, same-role first (the hard ones)."""
    kind = gold.kind if gold.kind in ("ClickAction", "RightClickAction", "DoubleClickAction") else "ClickAction"
    same_role, other_role = [], []
    for e in elements:
        if gold_target is not None and e.id == gold_target.id:
            continue
        cx, cy = e.center
        rec = ActionRecord(kind, {"x": cx, "y": cy})
        if gold_target is not None and e.role == gold_target.role:
            same_role.append(rec)
        else:
            other_role.append(rec)
    return same_role + other_role


def _drag_distractors(gold: ActionRecord, elements: list[DomElement]) -> list[ActionRecord]:
    """Real drags from the gold drag's own real source to the real centers of
    the other real elements on the page (real alternative drop targets)."""
    out = []
    fx, fy = gold.params["from_x"], gold.params["from_y"]
    for e in elements:
        cx, cy = e.center
        if abs(cx - float(gold.params["to_x"])) < 4 and abs(cy - float(gold.params["to_y"])) < 4:
            continue
        if abs(cx - float(fx)) < 4 and abs(cy - float(fy)) < 4:
            continue
        out.append(
            ActionRecord(
                "DragAction",
                {"from_x": fx, "from_y": fy, "to_x": cx, "to_y": cy,
                 "duration": gold.params.get("duration", 0.5)},
            )
        )
    return out


def build_candidates(
    *,
    gold: ActionRecord,
    elements: list[DomElement],
    peer_actions: list[ActionRecord],
    max_options: int,
    rng: random.Random,
) -> list[ActionRecord]:
    """The real, closed candidate set for one step: gold plus real, grounded
    distractors, in priority order (cross-parameterization oracle actions
    first -- the hardest -- then same-target type confusions, then real
    alternative on-page targets), deterministically shuffled and capped.

    Returns a list whose first-and-only gold entry is `gold` itself; ordering
    of the returned list is already shuffled, so gold's position carries no
    signal.
    """
    gold_target = target_element(gold, elements)
    seen = {gold.key()}
    ordered: list[ActionRecord] = []

    def add(recs: list[ActionRecord]) -> None:
        for r in recs:
            k = r.key()
            if k not in seen:
                seen.add(k)
                ordered.append(r)

    add([p for p in peer_actions if p.kind == gold.kind])
    add([p for p in peer_actions if p.kind != gold.kind])
    add(_same_target_variants(gold))
    if gold.kind == "DragAction":
        add(_drag_distractors(gold, elements))
    add(_element_click_distractors(gold, elements, gold_target))

    chosen = ordered[: max(0, max_options - 1)]
    out = [gold, *chosen]
    rng.shuffle(out)
    return out


# --------------------------------------------------------------------------
# Text (ax-tree-style) rendering
# --------------------------------------------------------------------------


def render_ax_tree(
    *, instruction: str, step_index: int, prior_actions: list[str], elements: list[DomElement]
) -> str:
    """The real text state: the real task instruction, the real action history
    so far this episode (the correct next action is history-dependent), and an
    accessibility-tree-style listing of the real live-DOM interactive elements
    with their real screen-space boxes. Carries no indication of which
    candidate is gold."""
    lines = [f"Task: {instruction}", f"Step: {step_index}"]
    if prior_actions:
        lines.append("Actions taken so far:")
        lines.extend(f"  {i + 1}. {a}" for i, a in enumerate(prior_actions))
    else:
        lines.append("Actions taken so far: (none -- this is the first step)")
    lines.append("Screen elements:")
    for e in elements:
        x0, y0, x1, y1 = e.frame
        lines.append(f'  - {e.role} "{e.label}" [{x0},{y0},{x1},{y1}]')
    return "\n".join(lines)


# --------------------------------------------------------------------------
# Conversion (pure: no cua_bench import, no live session)
# --------------------------------------------------------------------------


def convert_episode(
    episode: RecordedEpisode,
    *,
    split_name: str,
    peer_episodes: list[RecordedEpisode] | None = None,
    max_options: int = 10,
    modality_available: tuple[str, ...] = ("multimodal", "text"),
    seed: int = 0,
    require_oracle_success: bool = False,
    drop_offscreen_gold: bool = True,
) -> list[CuaTask]:
    """Converts one real recorded episode into one `CuaTask` per real step.

    `peer_episodes` are other real episodes of the SAME env from the SAME
    split; their same-step-index oracle actions become this step's hardest
    real distractors (see the module docstring). Pass none and the distractors
    fall back to the step's own real on-page alternatives.

    Elements outside the window's real visible content rect are excluded (they
    are in the DOM but not on the screenshot, so offering them as options
    would be offering something a multimodal model cannot see). With
    `drop_offscreen_gold` (the default), a step whose own gold target is
    off-screen is skipped entirely rather than emitted as an unanswerable
    task -- the bundled pages really do overflow their windows sometimes.
    Its action still counts in the action history of later steps.
    """
    if require_oracle_success and not episode.oracle_verified:
        return []
    peers = peer_episodes or []
    prior: list[str] = []
    tasks: list[CuaTask] = []

    for step in episode.steps:
        visible = episode.visible_elements(step)
        gold_point = step.gold_action.point()
        gold_onscreen = gold_point is None or episode.contains_point(*gold_point)
        if drop_offscreen_gold and not gold_onscreen:
            prior.append(step.gold_action.describe(step.elements))
            continue
        peer_actions = [
            p.steps[step.step_index].gold_action
            for p in peers
            if p.task_index != episode.task_index and len(p.steps) > step.step_index
        ]
        # A peer episode's own layout may differ, so only keep peer actions
        # that really land on this episode's own visible screen.
        peer_actions = [
            a for a in peer_actions
            if a.point() is None or episode.contains_point(*a.point())
        ]
        rng = random.Random(f"{episode.env_name}|{episode.task_index}|{step.step_index}|{seed}")
        candidates = build_candidates(
            gold=step.gold_action,
            elements=visible,
            peer_actions=peer_actions,
            max_options=max_options,
            rng=rng,
        )
        gold_key = step.gold_action.key()

        elements, options, expected = [], [], {}
        for i, cand in enumerate(candidates):
            eid = f"opt_{i}"
            label = cand.describe(visible)
            tgt = target_element(cand, visible)
            frame = list(tgt.frame) if tgt is not None else _point_frame(cand)
            role = tgt.role if tgt is not None else "Action"
            elements.append({"id": eid, "role": role, "label": label, "frame": frame})
            options.append(OptionSpec(element_id=eid, role=role, label=label, action="click"))
            options.append(OptionSpec(element_id=eid, role=role, label=label, action="skip"))
            expected[eid] = "click" if cand.key() == gold_key else "skip"

        if "text" in modality_available:
            ax_tree = render_ax_tree(
                instruction=episode.instruction,
                step_index=step.step_index,
                prior_actions=list(prior),
                elements=visible,
            )
        else:
            ax_tree = None
        screenshot = step.screenshot if "multimodal" in modality_available else None
        modalities = [m for m in modality_available if m != "multimodal" or screenshot]

        tasks.append(
            CuaTask(
                id=f"cbb_{episode.env_name}_{episode.task_index}_{step.step_index}",
                family=FAMILY,
                app=episode.env_name,
                modality_available=modalities,
                screenshot=screenshot,
                ax_tree=ax_tree,
                ax_tree_source="synthetic" if ax_tree else None,
                elements=elements,
                elements_source="synthetic_spec",
                entities=[],
                options=options,
                expected=expected,
                split="public",
                group=f"{episode.env_name}:{episode.task_index}",
                provenance={
                    "source": "cua-bench live environment (datasets/cua-bench-basic)",
                    "source_url": "https://github.com/trycua/cua/tree/main/libs/cua-bench",
                    "license": "MIT (cua-bench, this repository)",
                    "env_name": episode.env_name,
                    "task_index": episode.task_index,
                    "task_metadata": episode.metadata,
                    "instruction": episode.instruction,
                    "step_index": step.step_index,
                    "n_steps_in_episode": len(episode.steps),
                    "prior_actions": list(prior),
                    "provider": episode.provider,
                    "gold_action": asdict(step.gold_action),
                    "gold_label_method": "cua_bench_reference_solution (@cb.solve_task)",
                    "oracle_reward": episode.oracle_reward,
                    "oracle_verified": episode.oracle_verified,
                    "dataset_split": split_name,
                    "ax_tree_method": "live_dom_query (document.querySelectorAll + getBoundingClientRect), not a platform accessibility API",
                    "n_peer_distractor_actions": len(peer_actions),
                    "viewport": list(episode.viewport) if episode.viewport else None,
                    "window_layout_fitted": episode.window_layout_fitted,
                    "n_offscreen_elements_excluded": len(step.elements) - len(visible),
                    "dom_elements": [asdict(e) for e in visible],
                },
            )
        )
        prior.append(step.gold_action.describe(visible))
    return tasks


def _point_frame(action: ActionRecord, pad: int = 8) -> list[int]:
    pt = action.point()
    if pt is None:
        return [0, 0, 0, 0]
    x, y = int(pt[0]), int(pt[1])
    return [x - pad, y - pad, x + pad, y + pad]


# --------------------------------------------------------------------------
# Split assignment
# --------------------------------------------------------------------------


def assign_splits(n_parameterizations: int) -> dict[str, list[int]]:
    """Deterministic train/val/test partition over an env's own real
    `tasks_config` parameterizations. A clear majority stays in train+val;
    the LAST parameterizations (genuinely fresh parameter values the train
    split never saw) are held out as test -- the same train/test separation
    discipline every other family in this benchmark uses.

    n=1 -> all train (nothing can be held out honestly).
    n=2 -> 1 train, 1 test.
    n>=3 -> 1 val, 1 test (2 test once n>=4), rest train.
    """
    idx = list(range(n_parameterizations))
    if n_parameterizations <= 1:
        return {"train": idx, "val": [], "test": []}
    if n_parameterizations == 2:
        return {"train": idx[:1], "val": [], "test": idx[1:]}
    n_test = 2 if n_parameterizations >= 4 else 1
    test = idx[-n_test:]
    val = idx[-n_test - 1 : -n_test]
    train = idx[: -n_test - 1]
    return {"train": train, "val": val, "test": test}


# --------------------------------------------------------------------------
# Live recording (requires cua_bench + its provider)
# --------------------------------------------------------------------------


async def record_episode(
    *,
    env_dir: str | Path,
    task_index: int,
    out_dir: Path | None = None,
    provider: str | None = "simulated",
    split: str = "train",
    capture_screenshots: bool = True,
    fit_layout: bool = True,
) -> RecordedEpisode:
    """Drives ONE real episode of ONE real `cua-bench` task env and records
    every real oracle action with the real live state it was taken from.

    The recording works by wrapping the live session's `execute_action`: every
    cua-bench action -- whether the reference solution dispatches it directly
    or via a `click_element(selector)` helper -- funnels through that single
    call, so each real action is captured exactly once, with the real
    screenshot and the real DOM state as they were *immediately before* it.

    `provider=None` leaves the env's own declared provider (`native`) alone;
    the default `"simulated"` swaps in cua-bench's own Playwright provider
    (see the module docstring's honest-limitation note). `fit_layout` applies
    `fit_window_layout` after setup, which that provider needs for the pages
    to be rendered at their own requested size at all.
    """
    from cua_bench.core import make

    env_dir = Path(env_dir)
    env = make(str(env_dir), split=split)
    env.headless = True

    tasks = env.tasks_config_fn()
    if provider is not None:
        for t in tasks:
            if getattr(t, "computer", None):
                t.computer = {**t.computer, "provider": provider}
    env.tasks = tasks
    env.current_task = tasks[task_index]
    task_cfg = tasks[task_index]

    steps: list[RecordedStep] = []
    reward: float | None = None
    viewport: list[int] | None = None
    fitted = False
    try:
        await env.reset(task_id=task_index)
        session = env.session
        pid = await _first_window_pid(session)
        if fit_layout:
            fitted = await fit_window_layout(session, pid) is not None
        offset = await _window_offset(session, pid)
        viewport = await _viewport(session, pid)

        original_execute = session.execute_action

        async def recording_execute(action):  # noqa: ANN001 - mirrors provider signature
            idx = len(steps)
            shot_path = None
            if capture_screenshots and out_dir is not None:
                png = await session.screenshot()
                out_dir.mkdir(parents=True, exist_ok=True)
                p = out_dir / f"cbb_{env_dir.name}_{task_index}_{idx}.png"
                p.write_bytes(png)
                shot_path = str(p)
            raw = await session.execute_javascript(pid, INTERACTIVE_ELEMENTS_JS)
            elements = dom_elements_from_query(raw or [], offset_x=offset[0], offset_y=offset[1])
            steps.append(
                RecordedStep(
                    step_index=idx,
                    elements=elements,
                    gold_action=action_from_obj(action),
                    screenshot=shot_path,
                )
            )
            return await original_execute(action)

        session.execute_action = recording_execute
        await env.solve()
        session.execute_action = original_execute
        reward = _scalar_reward(await env.evaluate())
    finally:
        await env.close()

    return RecordedEpisode(
        env_name=env_dir.name,
        task_index=task_index,
        instruction=task_cfg.description,
        metadata=dict(task_cfg.metadata or {}),
        steps=steps,
        oracle_reward=reward,
        provider=provider or (task_cfg.computer or {}).get("provider", "native"),
        viewport=viewport,
        window_layout_fitted=fitted,
    )


def _scalar_reward(result: Any) -> float | None:
    """cua-bench evaluators return `[1.0]`/`[0.0]`, a bare float, or a dict."""
    if isinstance(result, (list, tuple)):
        vals = [v for v in result if isinstance(v, (int, float))]
        return float(sum(vals) / len(vals)) if vals else None
    if isinstance(result, bool):
        return 1.0 if result else 0.0
    if isinstance(result, (int, float)):
        return float(result)
    if isinstance(result, dict):
        for k in ("reward", "score", "success"):
            if k in result:
                return _scalar_reward(result[k])
    return None


async def _first_window_pid(session: Any) -> str:
    """The pid of the real task window the env's setup actually launched."""
    snapshot = await session.get_snapshot()
    for win in getattr(snapshot, "windows", []) or []:
        if str(getattr(win, "pid", "-1")) != "-1":
            return str(win.pid)
    raise RuntimeError("no task window found in the live session snapshot")


async def _window_offset(session: Any, pid: str) -> tuple[int, int]:
    """The real measured offset of the window's content area on the desktop,
    from the provider's own rect API (window space vs. screen space for the
    same real element) -- so DOM boxes land in the same pixel space as the
    screenshot and the actions."""
    win = await session.get_element_rect(pid, "body", space="window")
    scr = await session.get_element_rect(pid, "body", space="screen")
    if not win or not scr:
        return (0, 0)
    return int(scr["x"]) - int(win["x"]), int(scr["y"]) - int(win["y"])


#: Desktop-page JS that resizes the provider's own window-content `<iframe>`
#: to the height the task's own `launch_window(...)` call actually asked for.
_FIT_WINDOW_JS = """
(() => {
  const f = document.querySelector('iframe[pid="%(pid)s"]');
  if (!f) return null;
  f.style.height = %(h)d + 'px';
  f.style.flex = 'none';
  let p = f.parentElement, n = 0;
  while (p && n < 4) { p.style.height = 'auto'; p.style.minHeight = %(h)d + 'px'; p = p.parentElement; n++; }
  const r = f.getBoundingClientRect();
  return {x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height)};
})()
"""


async def fit_window_layout(session: Any, pid: str) -> dict | None:
    """Works around a real rendering defect in cua-bench's own `simulated`
    (Playwright) desktop provider: its window-content `<iframe>` renders about
    150px tall no matter what height the task's `launch_window(...)` asked
    for, while all 13 bundled envs request 256-500px-tall windows. The result
    is that most of each widget UI is clipped -- neither visible in the
    screenshot nor clickable -- which is why `click-button`'s own reference
    solution otherwise passes or fails depending on where its randomized
    target button happens to land.

    This resizes the provider's own container iframe to the height the env
    really requested. It touches only the provider's desktop chrome -- never
    the task page's DOM, content, state or reward function -- so the page
    simply gets the viewport its author asked for.

    Returns the corrected iframe rect, or `None` if the session exposes no
    such chrome (e.g. the `native` provider), in which case nothing is
    changed. Callers disclose whether this ran via
    `provenance["window_layout_fitted"]`.
    """
    page = getattr(session, "page", None)
    state = getattr(session, "_state", None)
    if page is None or not isinstance(state, dict):
        return None
    heights = [int(w["height"]) for w in (state.get("windows") or []) if w.get("height")]
    if not heights:
        return None
    try:
        return await page.evaluate(_FIT_WINDOW_JS % {"pid": pid, "h": max(heights)})
    except Exception:
        # A provider without this desktop chrome is not an error; there is
        # simply nothing to correct.
        return None


async def _viewport(session: Any, pid: str) -> list[int] | None:
    """The real visible content rect of the task window on the desktop, in
    screen space. The bundled pages are laid out for a larger area than the
    window they are launched in, so a real element can genuinely be in the
    DOM but off the screenshot; this is the rect that decides which."""
    rect = await session.get_element_rect(pid, "body", space="screen")
    if not rect:
        return None
    x, y = int(rect["x"]), int(rect["y"])
    return [x, y, x + int(rect["width"]), y + int(rect["height"])]


async def generate_dataset(
    *,
    dataset_dir: str | Path,
    out_dir: Path,
    env_names: tuple[str, ...] = ENV_NAMES,
    provider: str | None = "simulated",
    split: str = "train",
    max_options: int = 10,
    modality_available: tuple[str, ...] = ("multimodal", "text"),
    seed: int = 0,
    require_oracle_success: bool = False,
    capture_screenshots: bool = True,
    fit_layout: bool = True,
) -> dict[str, list[CuaTask]]:
    """Rolls out every real parameterization of every requested real
    `cua-bench-basic` env and returns `{"train": [...], "val": [...],
    "test": [...]}` of `CuaTask`s.

    Parameterizations are partitioned by `assign_splits` BEFORE conversion,
    and cross-parameterization distractors are drawn only from peers within
    the same split, so no test parameterization's gold action ever appears in
    a train task (and no train value is ever used to make a test task easier).
    """
    from cua_bench.core import make  # fail fast, clearly, if cua-bench is absent

    dataset_dir = Path(dataset_dir)
    out: dict[str, list[CuaTask]] = {"train": [], "val": [], "test": []}

    for name in env_names:
        env_dir = dataset_dir / name
        if not (env_dir / "main.py").exists():
            continue
        probe = make(str(env_dir), split=split)
        n_params = len(probe.tasks_config_fn())
        splits = assign_splits(n_params)

        episodes: dict[int, RecordedEpisode] = {}
        for idx in range(n_params):
            episodes[idx] = await record_episode(
                env_dir=env_dir,
                task_index=idx,
                out_dir=out_dir,
                provider=provider,
                split=split,
                capture_screenshots=capture_screenshots,
                fit_layout=fit_layout,
            )

        for split_name, indices in splits.items():
            peers = [episodes[i] for i in indices]
            for i in indices:
                out[split_name].extend(
                    convert_episode(
                        episodes[i],
                        split_name=split_name,
                        peer_episodes=peers,
                        max_options=max_options,
                        modality_available=modality_available,
                        seed=seed,
                        require_oracle_success=require_oracle_success,
                    )
                )
    return out
