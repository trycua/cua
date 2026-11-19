#!/usr/bin/env python3
"""Outcome-reward RL for a `cua-s1-4b` adapter, on the real `cua-bench-basic`
environment, on top of a supervised adapter.

Supervised training (`train_4b_v2.py`) is cross-entropy against gold labels on
static bounded decisions. This stage is reinforcement learning from real,
verifiable task outcomes: the policy acts in a live multi-step environment,
the environment decides whether the task was actually accomplished, and that
single terminal signal drives the update. No gold action labels exist here --
`cua-bench-basic` never says what the right action was, only whether the final
state satisfies the task.

The behaviour this addresses is one supervised training cannot reach: knowing
WHEN TO STOP. A per-step label can say which control to act on, but "the goal
is already achieved, do nothing further" depends on the outcome of the whole
episode. On a click-type environment a repeated action is harmless; on a
toggle environment an even number of toggles returns the widget to its
original state and the reward is lost.

THE FORMULATION
===============

Policy. Softmax over the option-letter logits at the final sequence position,
over the closed option set built from the live page (`build_task` below).
Identical to the readout used at eval time, so there is no train/serve gap.

Reward. `r = 1` if the environment's own terminal reward is >= 0.5, else 0.
That is `cua-bench`'s own verdict on the real final state.

Estimator: REINFORCE with a leave-one-out baseline (RLOO). For each task
instance, K episodes are rolled out on-policy; for episode k the advantage is
`A_k = r_k - mean(r_j, j != k)` and the loss is
`-(1/K) * sum_k A_k * sum_t log pi(a_t | s_t)`. Unbiased, needs no learned
critic, and the leave-one-out baseline is what makes it usable at small K.
PPO would add an off-policy ratio to clip, which buys nothing here: with K
episodes per instance and one gradient step per batch, the policy that
generated the data is the policy being updated.

Calibration. A policy gradient makes the policy more often right; it does not
make its stated probabilities mean anything. Per episode, the policy's
confidence is the geometric mean of the probabilities it assigned to the
actions it took, and the calibration loss is the Brier score of that
confidence against the realized outcome, `mean_k (conf_k - r_k)^2`. Brier is
a strict proper scoring rule, so its minimizer is the true P(success).

KL anchor. `KL(pi_theta || pi_ref)` averaged over visited states, against the
frozen supervised policy, so the run cannot collapse onto a degenerate
high-reward mode and destroy its initialization.

    total = L_pg + cal_weight * L_cal + kl_weight * L_kl

MODALITIES
==========
`--modality text` grounds each option in the live element's own label.
`--modality multimodal` instead gives the model set-of-mark grounding:
numbered boxes are drawn over the candidate elements on the live screenshot
and the options name marks by NUMBER only, never by the element's text, so
the model has to visually locate the mark. The mark boxes are placed from the
page's own element rectangles rather than by a visual detector -- the same
simplification `cua_bench_s1.task`'s `elements_source` documents -- so what
the model must do visually is decide which marked region satisfies the
instruction, not find the candidates.

MEMORY
======
LoRA only. The KL reference is a SECOND LoRA ADAPTER on the same base model,
switched with `set_adapter()`, rather than a duplicated frozen copy of the
model. (`disable_adapter()` would anchor to the raw base model, not to the
supervised policy this run starts from.) Rollouts use one forward pass per
step and never call `generate()`, so no KV cache accumulates. The rollout
buffer holds tasks (whose screenshot is a path) and scalars, not hidden
states, and gradients are accumulated one step at a time.

Usage:
    python libs/cua-s1/training/train_4b_rl.py --sft-adapter runs/cua4b_v2_lora \
        --out runs/cua4b_rl --modality text
"""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import random
import re
import sys
import tempfile
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

# See train_4b.py's note: `cua-bench-s1` is a sibling package declared as a
# dependency of the `four-b-train`/`four-b-rl` extras, with a source-tree
# fallback for an environment that installed only cua-s1's own dependencies.
try:
    from cua_bench_s1.task import CuaTask, OptionSpec
except ImportError:
    _BENCH_S1_SRC = Path(__file__).resolve().parents[2] / "cua-bench-s1" / "python" / "src"
    sys.path.insert(0, str(_BENCH_S1_SRC))
    from cua_bench_s1.task import CuaTask, OptionSpec  # noqa: E402

from cua_s1.four_b import DEFAULT_BASE_MODEL, Option, assign_letters, build_prompt

# Environments that are rewardable under the `simulated` provider AND expose
# page elements an element-grounded policy can act on.
TRAIN_ENVS = (
    "click-button",
    "click-icon",
    "color-picker",
    "spreadsheet-cell",
    "toggle-switch",
    "typing-input",
)

# The readout assigns one single-token letter per OPTION, so options -- not
# elements -- are the hard budget. `MAX_ELEMENTS` is an upper bound applied
# before the option budget, since an element can contribute several options.
MAX_OPTIONS = 26
MAX_ELEMENTS = 11

_TEXT_TAGS = ("INPUT", "TEXTAREA")
_STOPWORDS = {
    "the",
    "a",
    "an",
    "into",
    "on",
    "in",
    "to",
    "and",
    "click",
    "type",
    "select",
    "enter",
    "turn",
    "press",
    "page",
    "icon",
    "button",
    "field",
    "cell",
    "color",
}


# ---------------------------------------------------------------------------
# Live step -> bounded decision, and chosen option -> real environment action.
# ---------------------------------------------------------------------------
# `cua-bench-basic`'s action space is coordinate-based (`ClickAction(x, y)`,
# `TypeAction(text)`) and its observation is a screenshot plus an instruction.
# The helpers below turn one live step from
# `cua_bench_s1.agentic.CuaBenchBasicEnv` into the closed (element, action)
# option set this model family already scores, and map the chosen option back
# to real environment actions -- so the RL policy is the same policy, prompt
# builder and readout the static benchmark scores.


def instruction_tokens(text: str) -> set[str]:
    return {t for t in re.findall(r"[a-z0-9]+", (text or "").lower()) if t not in _STOPWORDS}


def quoted_values(instruction: str, metadata: dict | None) -> list[str]:
    """The concrete strings this task might require typing: the instruction's
    quoted spans plus the environment's own `metadata` values. Both are part
    of the observation the environment hands the agent, not labels."""
    out: list[str] = []
    for quoted in re.findall(r'"([^"]+)"', instruction or ""):
        out.append(quoted)
    for key in ("text", "value", "input_text", "name"):
        v = (metadata or {}).get(key)
        if isinstance(v, str) and v and v not in out:
            out.append(v)
    return out[:3]


def prune_elements(elements: list[dict], instruction: str, limit: int = MAX_ELEMENTS) -> list[dict]:
    """Keep at most `limit` candidate elements, ranked by lexical overlap
    between the element's label and the instruction, ties broken by page
    order.

    Ranking uses the instruction with its QUOTED SPANS REMOVED. A quoted span
    is the VALUE to enter (`quoted_values` consumes it for exactly that), never
    the target to act on: `Enter "=SUM(A1:A10)" into cell C3.` must rank cell
    C3, not the cell A1 the formula happens to mention.

    The result is returned in RANK order, not page order, because the option
    builder consumes it in order and stops when the letter budget is
    exhausted -- page order would let truncation drop the best candidate.

    This pruning is part of the POLICY, not the environment, and it uses the
    instruction, so on element-rich pages it does part of the work the model
    would otherwise do. On pages with few elements it is a no-op.
    """
    if len(elements) <= limit:
        return list(elements)
    toks = instruction_tokens(re.sub(r'"[^"]*"', " ", instruction))

    def score(e: dict) -> int:
        return len(instruction_tokens(e.get("label") or "") & toks)

    ranked = sorted(range(len(elements)), key=lambda i: (-score(elements[i]), i))
    return [elements[i] for i in ranked[:limit]]


# Reads the CURRENT state of each interactive element: what a text field
# contains, whether a switch or checkbox is on, what a select has chosen. Run
# through the same public `session.execute_javascript` API the environment's
# own `elements()` uses; read-only.
LIVE_VALUES_JS = """
(() => {
  const out = [];
  document.querySelectorAll(
    'input, textarea, select, button, [role="switch"], [role="checkbox"]'
  ).forEach((el) => {
    const tag = el.tagName;
    let value = null, checked = null;
    if (tag === 'INPUT' || tag === 'TEXTAREA') {
      if (el.type === 'checkbox' || el.type === 'radio') { checked = !!el.checked; }
      else { value = el.value === undefined ? null : String(el.value); }
    } else if (tag === 'SELECT') {
      value = el.value === undefined ? null : String(el.value);
    }
    const aria = el.getAttribute('aria-checked') || el.getAttribute('aria-pressed');
    if (aria !== null && checked === null) { checked = (aria === 'true'); }
    if (el.id || value !== null || checked !== null) {
      out.push({dom_id: el.id || null, tag: tag, value: value, checked: checked});
    }
  });
  return out;
})()
"""


async def live_values(env) -> dict[str, dict]:
    """`dom_id -> {value, checked}` for the live page; `{}` if unavailable.

    `CuaBenchBasicEnv.elements()` reports each element's identity and box but
    not what a text field currently contains or whether a switch is on. Those
    are what make "is this task already done?" decidable from the observation:
    without them the state text is identical before and after the policy acts,
    and an agent that re-types into a field that already holds the value
    appends to it and loses the reward.

    Best-effort by design: any failure returns `{}` and the caller falls back
    to the value-free state text, so a provider that cannot run JavaScript
    degrades rather than crashing a training run.
    """
    inner = getattr(env, "_env", None)
    session = getattr(inner, "session", None)
    if session is None:
        return {}
    try:
        from cua_bench_s1.agentic.cua_bench_basic_env import _first_window_pid

        pid = _first_window_pid(session)
        if pid is None:
            return {}
        raw = await session.execute_javascript(pid, LIVE_VALUES_JS)
    except Exception:  # noqa: BLE001 - observability is best-effort, never fatal
        return {}
    out: dict[str, dict] = {}
    for rec in raw or []:
        dom_id = rec.get("dom_id")
        if dom_id:
            out[dom_id] = {"value": rec.get("value"), "checked": rec.get("checked")}
    return out


def state_suffix(element: dict, values: dict[str, dict]) -> str:
    """The live-state annotation for one element's state-description line."""
    rec = values.get(element.get("dom_id") or "")
    if not rec:
        return ""
    if rec.get("checked") is not None:
        return f" checked={str(rec['checked']).lower()}"
    value = rec.get("value")
    if value is not None:
        return f' value="{value}"'
    return ""


def _option_group(
    element: dict, eid: str, role: str, label: str, values: list[str], values_state: dict[str, dict]
) -> list[OptionSpec]:
    """Every option one element offers this turn.

    A `fill` whose value the field already holds is not offered: typing
    APPENDS in this environment, so repeating it corrupts the field and
    destroys the reward. That rule only applies when the live value is
    observable; with no state available the option is still offered.
    """
    is_text = (element.get("tag") or "").upper() in _TEXT_TAGS
    current = (values_state.get(element.get("dom_id") or "") or {}).get("value")
    group: list[OptionSpec] = []
    if is_text and values:
        for vi, value in enumerate(values):
            if current is not None and current == value:
                continue
            group.append(OptionSpec(eid, role, label, "fill", f"val_{vi}"))
    group.append(OptionSpec(eid, role, label, "click"))
    group.append(OptionSpec(eid, role, label, "skip"))
    return group


def build_options(
    elements: list[dict],
    instruction: str,
    metadata: dict | None,
    max_options: int = MAX_OPTIONS,
    values_state: dict[str, dict] | None = None,
) -> list[OptionSpec]:
    """The closed option set for one live step, bounded by the letter budget.

    Elements are consumed in the order given (already ranked by
    `prune_elements`) and an element is admitted only if its whole option
    group fits -- a partially admitted element would offer `click` but not
    `fill`, silently removing the correct action. One slot is always reserved
    for the terminal `done` option, which is how the policy stops.
    """
    values = quoted_values(instruction, metadata)
    state = values_state or {}
    opts: list[OptionSpec] = []
    for element in elements:
        eid = element["id"]
        group = _option_group(
            element,
            eid,
            element.get("role") or "Button",
            element.get("label") or eid,
            values,
            state,
        )
        if len(opts) + len(group) > max_options - 1:
            break
        opts.extend(group)
    opts.append(OptionSpec("__episode__", "Button", "Finish - task complete", "done"))
    return opts


def build_task(
    step_result,
    elements: list[dict],
    episode_id: str,
    step: int,
    values_state: dict[str, dict] | None = None,
) -> CuaTask:
    """One live step as a text-modality `CuaTask`, so the existing prompt
    builder and readout apply unchanged.

    `expected` is a placeholder: nothing on this path reads it, because
    success comes from the environment's own reward rather than from a label.
    Only elements that made it into the option set are described, so the state
    text never advertises a control the model has no option to act on.
    """
    instruction = step_result.instruction or ""
    metadata = step_result.metadata or {}
    kept = prune_elements(elements, instruction)
    options = build_options(kept, instruction, metadata, values_state=values_state)
    admitted = {o.element_id for o in options}
    kept = [e for e in kept if e["id"] in admitted]
    values = quoted_values(instruction, metadata)
    vs = values_state or {}
    ax_lines = [
        f'- {e.get("role") or "Button"} "{e.get("label")}"{state_suffix(e, vs)} @ {e.get("frame")}'
        for e in kept
    ]
    return CuaTask(
        id=f"{episode_id}-s{step}",
        family="multi_step_submit",
        app="cua_bench_basic",
        modality_available=["text"],
        screenshot=None,
        ax_tree="\n".join(ax_lines) or "- (no interactive elements detected)",
        ax_tree_source="real",
        elements=[
            {"id": e["id"], "role": e.get("role"), "label": e.get("label"), "frame": e.get("frame")}
            for e in kept
        ],
        elements_source="accessibility_api",
        entities=[
            {"id": f"val_{i}", "label": "text to type", "value": v} for i, v in enumerate(values)
        ],
        options=options,
        expected={o.element_id: "skip" for o in options},
        provenance={"synthetic_goal": instruction, "agentic_step": step},
    )


MARK_COLORS = (
    (255, 59, 48),
    (0, 122, 255),
    (52, 199, 89),
    (255, 149, 0),
    (175, 82, 222),
    (255, 45, 85),
    (90, 200, 250),
    (162, 132, 94),
)


def render_marked_screenshot(png_bytes: bytes, elements: list[dict], out_path) -> dict[int, dict]:
    """Draw a numbered box per candidate element on the live screenshot and
    return the `mark number -> element` map."""
    import io

    from PIL import Image, ImageDraw

    img = Image.open(io.BytesIO(png_bytes)).convert("RGB")
    draw = ImageDraw.Draw(img)
    marks: dict[int, dict] = {}
    for i, element in enumerate(elements, start=1):
        frame = element.get("frame")
        if not frame:
            continue
        x0, y0, x1, y1 = (int(v) for v in frame)
        color = MARK_COLORS[(i - 1) % len(MARK_COLORS)]
        draw.rectangle((x0, y0, x1, y1), outline=color, width=3)
        tw, th = 18, 16
        tx = max(0, min(x0, img.width - tw))
        ty = max(0, min(y0 - th, img.height - th))
        draw.rectangle((tx, ty, tx + tw, ty + th), fill=color)
        draw.text((tx + 4, ty + 2), str(i), fill=(255, 255, 255))
        marks[i] = element
    img.save(out_path)
    return marks


def build_options_multimodal(
    marks: dict[int, dict],
    instruction: str,
    metadata: dict | None,
    values_state: dict[str, dict] | None = None,
    max_options: int = MAX_OPTIONS,
) -> list[OptionSpec]:
    """Option set whose labels carry ONLY the mark number -- no page text, so
    the model has to read the marked region rather than the option list."""
    values = quoted_values(instruction, metadata)
    state = values_state or {}
    opts: list[OptionSpec] = []
    for n, element in marks.items():
        group = _option_group(element, element["id"], "Mark", f"mark {n}", values, state)
        if len(opts) + len(group) > max_options - 1:
            break
        opts.extend(group)
    opts.append(OptionSpec("__episode__", "Mark", "Finish - task complete", "done"))
    return opts


def build_task_multimodal(
    step_result,
    marks: dict[int, dict],
    screenshot_path: str,
    episode_id: str,
    step: int,
    values_state: dict[str, dict] | None = None,
) -> CuaTask:
    """One live step as a multimodal `CuaTask`: the marked screenshot is the
    whole state. `ax_tree` is None so neither the prompt builder nor the
    readout can fall back to text, which is what keeps this an honest
    multimodal measurement."""
    instruction = step_result.instruction or ""
    metadata = step_result.metadata or {}
    options = build_options_multimodal(marks, instruction, metadata, values_state)
    admitted = {o.element_id for o in options}
    values = quoted_values(instruction, metadata)
    return CuaTask(
        id=f"{episode_id}-s{step}",
        family="multi_step_submit",
        app="cua_bench_basic",
        modality_available=["multimodal"],
        screenshot=str(screenshot_path),
        ax_tree=None,
        ax_tree_source=None,
        elements=[
            {"id": e["id"], "role": "Mark", "label": f"mark {n}", "frame": e.get("frame")}
            for n, e in marks.items()
            if e["id"] in admitted
        ],
        elements_source="cua_som",
        entities=[
            {"id": f"val_{i}", "label": "text to type", "value": v} for i, v in enumerate(values)
        ],
        options=options,
        expected={o.element_id: "skip" for o in options},
        provenance={"synthetic_goal": instruction, "agentic_step": step},
    )


@dataclass
class ActionPlan:
    """The real `cua_bench` action(s) one chosen option expands into."""

    actions: list
    done: bool = False
    note: str = ""


def option_to_actions(option: OptionSpec, elements: list[dict], values: list[str]) -> ActionPlan:
    """Map a chosen option back to real environment actions.

    `fill` expands into a focus click followed by a type, which is two real
    environment steps against the step cap -- the environment has no
    single "set this field" action.
    """
    from cua_bench.types import ClickAction, DoneAction, TypeAction

    if option.action == "done":
        return ActionPlan([DoneAction()], done=True, note="agent declared done")
    element = next((e for e in elements if e["id"] == option.element_id), None)
    if element is None or not element.get("frame"):
        return ActionPlan([], note="no-op: option had no locatable element")
    x0, y0, x1, y1 = element["frame"]
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    if option.action == "click":
        return ActionPlan([ClickAction(x=cx, y=cy)], note=f"click {element.get('label')!r}")
    if option.action == "fill":
        try:
            text = values[int(str(option.entity_id).split("_")[1])]
        except (IndexError, ValueError):
            return ActionPlan([], note="no-op: unresolved fill value")
        return ActionPlan(
            [ClickAction(x=cx, y=cy), TypeAction(text=text)],
            note=f"fill {element.get('label')!r} with {text!r}",
        )
    return ActionPlan([], note=f"no-op: {option.action}")


# ---------------------------------------------------------------------------
# Policy, rollout, objective.
# ---------------------------------------------------------------------------


def _gpu_mb() -> int:
    try:
        import torch

        if torch.cuda.is_available():
            return torch.cuda.memory_allocated() // (1024 * 1024)
    except Exception:  # noqa: BLE001 - a memory readout must never fail a run
        pass
    return -1


def task_options(task: CuaTask) -> list[Option]:
    """`OptionSpec` -> `cua_s1.four_b.Option`, order preserved (letter
    assignment is order-sensitive)."""
    return [
        Option(
            element_id=o.element_id,
            role=o.role,
            label=o.label,
            action=o.action,
            entity_id=o.entity_id,
        )
        for o in task.options
    ]


class Policy:
    """The frozen base model plus two LoRA adapters: a trainable `policy` and
    a frozen `ref` used only as the KL anchor. Both are loaded from the same
    supervised adapter, so the run starts anchored to where it started."""

    def __init__(
        self, base_model: str, sft_adapter: str, device: str = "cuda", modality: str = "text"
    ):
        import torch
        from peft import PeftModel
        from transformers import (
            AutoModelForCausalLM,
            AutoModelForImageTextToText,
            AutoProcessor,
            AutoTokenizer,
        )

        self.modality = modality
        self.tokenizer = AutoTokenizer.from_pretrained(base_model)
        if self.tokenizer.pad_token is None:
            self.tokenizer.pad_token = self.tokenizer.eos_token
        # The model class must follow the modality: this checkpoint's vision
        # tower is only wired up under AutoModelForImageTextToText, and the
        # causal-LM class drops every `model.visual.*` weight.
        self.processor = (
            AutoProcessor.from_pretrained(base_model) if modality == "multimodal" else None
        )
        model_cls = (
            AutoModelForImageTextToText if modality == "multimodal" else AutoModelForCausalLM
        )
        base = model_cls.from_pretrained(base_model, torch_dtype=torch.bfloat16, device_map=device)
        self.model = PeftModel.from_pretrained(
            base, sft_adapter, adapter_name="policy", is_trainable=True
        )
        self.model.load_adapter(sft_adapter, adapter_name="ref", is_trainable=False)
        self.model.set_adapter("policy")
        self.torch = torch

    def letter_ids(self, assignment) -> list[int]:
        ids = []
        for letter in assignment.letters:
            toks = self.tokenizer.encode(letter, add_special_tokens=False)
            if len(toks) != 1:
                raise ValueError(f"letter {letter!r} is not a single token for this tokenizer")
            ids.append(toks[0])
        return ids

    def encode(self, task: CuaTask):
        """Tokenize one task. Called lazily during rollout and again during
        the update rather than cached: caching would hold every visited step's
        decoded `pixel_values` resident across K episodes. A `CuaTask` carries
        only an image PATH."""
        assignment = assign_letters(task_options(task))
        messages = build_prompt(
            assignment,
            app=task.app,
            task_family=task.family,
            ax_tree=task.ax_tree,
            screenshot=task.screenshot,
            modality=self.modality,
            goal=task.goal,
        )
        if self.modality == "multimodal":
            from PIL import Image

            image = Image.open(task.screenshot).convert("RGB")
            chat = self.processor.apply_chat_template(
                messages, tokenize=False, add_generation_prompt=True
            )
            inputs = dict(self.processor(text=[chat], images=[image], return_tensors="pt"))
        else:
            text = self.tokenizer.apply_chat_template(
                messages, tokenize=False, add_generation_prompt=True
            )
            inputs = dict(self.tokenizer(text, return_tensors="pt"))
        return assignment, inputs, self.letter_ids(assignment)

    def log_probs(
        self, inputs: dict, letter_ids: list[int], *, adapter: str = "policy", grad: bool = False
    ):
        torch = self.torch
        from torch.nn import functional as F

        self.model.set_adapter(adapter)
        ctx = torch.enable_grad() if grad else torch.no_grad()
        fwd = {}
        for k, v in inputs.items():
            v = v.to(self.model.device)
            if k == "pixel_values":
                v = v.to(dtype=self.model.dtype)
            fwd[k] = v
        with ctx:
            out = self.model(**fwd)
            final = out.logits[0, -1, :]
            selected = final[torch.tensor(letter_ids, device=final.device)].float()
            return F.log_softmax(selected, dim=-1)


async def rollout_once(policy: Policy, env_name: str, task_index: int, args, *, sample: bool):
    """One real episode. Returns `(reward, steps)`, where each step is
    `(task, letter_ids, chosen_index)` -- the task, not its tensors, so the
    buffer stays small and multimodal states stay on disk."""
    from cua_bench.types import WaitAction
    from cua_bench_s1.agentic import CuaBenchBasicEnv

    torch = policy.torch
    steps: list[tuple] = []
    tmpdir = tempfile.mkdtemp(prefix="cua_s1_rl_")
    async with CuaBenchBasicEnv(
        env_name,
        dataset_dir=args.dataset_dir,
        task_index=task_index,
        provider=args.provider,
        max_steps=args.max_steps,
    ) as env:
        obs = await env.reset()
        while not obs.done:
            elements = await env.elements()
            values_state = await live_values(env)
            kept = prune_elements(elements, obs.instruction or "")
            if args.modality == "multimodal":
                shot = f"{tmpdir}/{env_name}_{task_index}_{len(steps)}.png"
                marks = render_marked_screenshot(obs.screenshot, kept, shot)
                if not marks:
                    break
                task = build_task_multimodal(
                    obs,
                    marks,
                    shot,
                    f"{env_name}-{task_index}",
                    obs.step_count,
                    values_state=values_state,
                )
            else:
                task = build_task(
                    obs,
                    elements,
                    f"{env_name}-{task_index}",
                    obs.step_count,
                    values_state=values_state,
                )
            assignment, inputs, letter_ids = policy.encode(task)
            log_probs = policy.log_probs(inputs, letter_ids, adapter="policy", grad=False)
            if sample:
                probs = (log_probs / args.sample_temperature).exp()
                probs = probs / probs.sum()
                choice = int(torch.multinomial(probs, 1).item())
            else:
                choice = int(log_probs.argmax().item())
            steps.append((task, letter_ids, choice))

            plan = option_to_actions(
                assignment.options[choice], kept, quoted_values(obs.instruction or "", obs.metadata)
            )
            if not plan.actions:
                obs = await env.step(WaitAction())
                continue
            for action in plan.actions:
                obs = await env.step(action)
                if obs.done:
                    break
        reward = obs.reward if obs.reward is not None else await env.evaluate()
    return (1.0 if (reward is not None and reward >= 0.5) else 0.0), steps


def rloo_advantage(rewards: list[float], k: int) -> float:
    """Leave-one-out advantage for episode `k`: its reward minus the mean of
    the other episodes' rewards. Zero-mean across the group by construction,
    and needs no learned critic."""
    if len(rewards) < 2:
        return rewards[k]
    baseline = (sum(rewards) - rewards[k]) / (len(rewards) - 1)
    return rewards[k] - baseline


def brier_step_coefficient(confidence: float, reward: float, n_steps: int) -> float:
    """`d (conf - r)^2 / d log pi_t` with `conf = exp(mean_t log pi_t)` held
    detached: `2 (conf - r) * conf / T`.

    Taking the coefficient from a detached confidence makes each step's
    contribution linear in `log pi_t`, so the Brier term accumulates step by
    step and is mathematically identical to backwarding the coupled
    expression -- without keeping every step's activations alive at once.
    """
    return 2.0 * (confidence - reward) * confidence / n_steps


def accumulate_batch_grads(policy: Policy, group: list[tuple], args) -> tuple[bool, dict]:
    """RLOO + Brier calibration + KL over one instance's K episodes, with the
    gradient accumulated ONE STEP AT A TIME.

    Building an episode's loss as a single expression and backwarding it once
    keeps every visited step's forward activations alive simultaneously (K
    episodes x up to `--max-steps` steps of ~1-2k-token prompts). Every term's
    gradient decomposes linearly across steps once the per-episode scalars are
    detached -- the RLOO advantage comes from environment rewards and is
    constant in theta, the Brier coefficient is detached (see
    `brier_step_coefficient`), and KL is already a per-step sum -- so each step
    is forwarded, backwarded and freed before the next. A first no-grad pass
    collects the detached confidence the Brier coefficient needs.
    """
    torch = policy.torch
    rewards = [r for r, _ in group]
    k_episodes = len(group)
    mean_reward = sum(rewards) / k_episodes
    stats: dict[str, float] = defaultdict(float)
    n_eps = 0

    for k, (reward, steps) in enumerate(group):
        if not steps:
            continue
        n_steps = len(steps)
        advantage = rloo_advantage(rewards, k)

        total_log_prob = 0.0
        for task, letter_ids, choice in steps:
            _, inputs, _ = policy.encode(task)
            log_probs = policy.log_probs(inputs, letter_ids, adapter="policy", grad=False)
            total_log_prob += float(log_probs[choice].item())
            del inputs
        confidence = float(math.exp(total_log_prob / n_steps))
        brier = (confidence - reward) ** 2
        cal_coefficient = brier_step_coefficient(confidence, reward, n_steps)

        kl_sum = 0.0
        for task, letter_ids, choice in steps:
            _, inputs, _ = policy.encode(task)
            ref = policy.log_probs(inputs, letter_ids, adapter="ref", grad=False).detach()
            log_probs = policy.log_probs(inputs, letter_ids, adapter="policy", grad=True)
            kl_t = (log_probs.exp() * (log_probs - ref.to(log_probs.device))).sum()
            step_loss = ((-advantage / n_steps) + args.cal_weight * cal_coefficient) * log_probs[
                choice
            ] + args.kl_weight * kl_t / n_steps
            (step_loss / max(1, k_episodes)).backward()
            kl_sum += float(kl_t.item())
            del log_probs, ref, kl_t, step_loss, inputs

        stats["conf"] += confidence
        stats["cal"] += brier
        stats["kl"] += kl_sum / n_steps
        n_eps += 1

    if n_eps == 0:
        return False, {}
    return True, {
        "reward": mean_reward,
        # A group whose episodes all succeeded or all failed has zero
        # advantage everywhere and contributes no policy gradient; tracking
        # the informative fraction says how much of the run is doing work.
        "informative": float(0.0 < mean_reward < 1.0),
        "conf": stats["conf"] / n_eps,
        "cal": stats["cal"] / n_eps,
        "kl": stats["kl"] / n_eps,
    }


async def train_async(args: argparse.Namespace) -> None:
    import torch
    from cua_bench_s1.agentic import list_task_variants

    policy = Policy(args.base_model, args.sft_adapter, modality=args.modality)
    trainable = [p for p in policy.model.parameters() if p.requires_grad]
    print(f"[train_4b_rl] trainable tensors: {len(trainable)}; GPU alloc {_gpu_mb()} MB")
    optim = torch.optim.AdamW(trainable, lr=args.lr)

    variants = list_task_variants(dataset_dir=args.dataset_dir, split=args.split)
    instances = [
        (env_name, ti)
        for env_name in (args.envs or TRAIN_ENVS)
        for ti in range(min(variants.get(env_name, 1), args.variants))
    ]
    rng = random.Random(args.seed)
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    history = []

    for epoch in range(args.epochs):
        rng.shuffle(instances)
        agg: dict[str, float] = defaultdict(float)
        n_batches = 0
        for env_name, task_index in instances:
            group = []
            for _ in range(args.samples):
                reward, steps = await rollout_once(policy, env_name, task_index, args, sample=True)
                group.append((reward, steps))
            optim.zero_grad()
            ok, stats = accumulate_batch_grads(policy, group, args)
            if not ok:
                continue
            torch.nn.utils.clip_grad_norm_(trainable, args.max_grad_norm)
            optim.step()
            optim.zero_grad()
            for k, v in stats.items():
                agg[k] += v
            n_batches += 1
            del group
            torch.cuda.empty_cache()
            print(
                f"[train_4b_rl]   {env_name}/{task_index}: reward={stats['reward']:.2f} "
                f"informative={stats['informative']:.0f} conf={stats['conf']:.3f} "
                f"kl={stats['kl']:.4f} | GPU {_gpu_mb()} MB"
            )

        m = max(1, n_batches)
        record = {
            "epoch": epoch + 1,
            "mean_reward": agg["reward"] / m,
            "informative_fraction": agg["informative"] / m,
            "mean_confidence": agg["conf"] / m,
            "mean_brier": agg["cal"] / m,
            "mean_kl": agg["kl"] / m,
            "n_batches": n_batches,
        }
        history.append(record)
        print(
            f"[train_4b_rl] epoch {epoch + 1}/{args.epochs} "
            + " ".join(
                f"{k}={v:.4f}" if isinstance(v, float) else f"{k}={v}" for k, v in record.items()
            )
        )
        # Only the trainable adapter is written; `ref` stays the frozen
        # supervised anchor and is not part of the artifact.
        policy.model.set_adapter("policy")
        policy.model.save_pretrained(str(out_dir), selected_adapters=["policy"])
        policy.tokenizer.save_pretrained(str(out_dir))

    (out_dir / "train_config.json").write_text(
        json.dumps(
            {**vars(args), "stage": "rl_cua_bench_basic", "history": history}, default=str, indent=2
        ),
        encoding="utf-8",
    )
    print(f"[train_4b_rl] done -> {out_dir}")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument(
        "--sft-adapter",
        required=True,
        help="supervised adapter: both the RL initialization and the KL reference",
    )
    p.add_argument("--out", required=True, help="output directory for the RL LoRA adapter")
    p.add_argument("--base-model", default=DEFAULT_BASE_MODEL)
    p.add_argument(
        "--dataset-dir",
        default=None,
        help="cua-bench-basic dataset directory; defaults to the bundled one",
    )
    p.add_argument(
        "--envs",
        nargs="*",
        default=None,
        help=f"environments to train on; default: {' '.join(TRAIN_ENVS)}",
    )
    p.add_argument("--variants", type=int, default=2, help="task parameterizations per environment")
    p.add_argument("--epochs", type=int, default=2)
    p.add_argument(
        "--samples",
        type=int,
        default=4,
        help="K on-policy episodes per instance (the RLOO group size)",
    )
    p.add_argument("--lr", type=float, default=1e-5)
    p.add_argument("--sample-temperature", type=float, default=1.3)
    p.add_argument(
        "--cal-weight", type=float, default=1.0, help="weight on the Brier calibration term"
    )
    p.add_argument("--kl-weight", type=float, default=0.05, help="weight on the KL anchor")
    p.add_argument("--max-grad-norm", type=float, default=1.0)
    p.add_argument("--max-steps", type=int, default=20, help="environment step cap per episode")
    p.add_argument("--modality", choices=["text", "multimodal"], default="text")
    p.add_argument("--provider", default="simulated")
    p.add_argument("--split", default="train")
    p.add_argument("--seed", type=int, default=0)
    return p


def main() -> None:
    asyncio.run(train_async(build_parser().parse_args()))


if __name__ == "__main__":
    main()
