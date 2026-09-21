"""Task schema for cua-bench-s1: a "typed bounded-decision" pattern (state +
fixed option set + expected answer) applied to real GUI/computer-use actions.

A CuaTask never asks a model to generate text. It presents:
  - `state`: a screenshot (path) and/or accessibility-tree element list for one
    viewport of one app/page at one moment.
  - `options`: the fixed, enumerable set of (element, candidate_action) pairs a
    model must score in one pass -- e.g. (Edit "Email", fill:<entity>), (CheckBox
    "I consent...", check), (Button "Submit", click), or skip for any element.
  - `expected`: the gold action (and, for fill, gold entity pointer) per element,
    used only for scoring -- never shown to a model under test.

The dataset is pre-registered: `dataset_hash` is a SHA-256 over the sorted
canonical JSON of every task, computed before any model sees the data, so a
later change to the eval set is visible as a hash change rather than silently
absorbed into a leaderboard number.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, dataclass, field
from pathlib import Path

# Action taxonomy. Kept small and closed on purpose -- see docs/TASK_FAMILIES.md.
ACTIONS = ("fill", "check", "click", "select", "scroll", "skip")

# Task families. See docs/TASK_FAMILIES.md for a fuller description of each,
# and the top-level README for which families are trained-on vs. held-out-only.
FAMILIES = (
    "form_filling",        # fill fields from a source document/entity list
    "login_auth",          # username/password entry + submit
    "consent_checkbox",    # a required checkbox before submit (own family: a distinct failure mode)
    "multi_step_submit",   # a form spanning more than one viewport/screen before its final submit
    "pagination",          # repeated "next" navigation through a list/result set
    "search_filter",       # entering structured constraints (route/date/qty) then searching
    "safety_gate",         # a superficially-actionable element whose correct action is "skip" for a
                            # safety reason (destructive/irreversible, financial commitment, credential
                            # exposure, or scope creep) -- see docs/TASK_FAMILIES.md for the taxonomy.
    "game_control",         # score candidate discrete-action "buttons" from a live game frame -- same
                             # one-pass scoring shape as the GUI families, applied to a real game loop
                             # (ViZDoom) instead of a static form; held out only, never trained on.
    "chess",                 # score candidate legal moves from a real chess position (rendered board
                              # image for multimodal, FEN-derived text for text modality) against a real
                              # Stockfish gold move; held out only, never trained on.
    "general_decision",       # a text-only typed bounded decision imported from an external benchmark
                               # (see datagen/external_bench_import.py); held out only.
)


@dataclass
class OptionSpec:
    """One candidate (element, action) pair a model scores. `entity_id` is set
    only for `fill` options and points into the task's own entity list (never
    a raw string value), which is what makes this a classification task
    rather than a generation task."""
    element_id: str        # stable id within this task's element list
    role: str               # "Edit" | "CheckBox" | "Button" | "Select"
    label: str
    action: str              # one of ACTIONS
    entity_id: str | None = None


@dataclass
class CuaTask:
    """Carries BOTH representations of the same underlying state when available
    (screenshot AND accessibility tree) -- modality is a property of an EVAL
    RUN, not of the task. `cua_bench_s1.eval.runner` strips whichever artifact
    a given modality split must not see before handing the task to a model, so
    a multimodal run never leaks ax-tree text and a text run never sees pixels.
    This lets the exact same task (same elements, same gold labels) anchor a
    same-difficulty comparison across modalities, which a modality-specific
    task set could not give you."""
    id: str
    family: str                       # one of FAMILIES
    app: str                          # which target app/page this came from
    modality_available: list[str]      # subset of ("text", "multimodal") this task can be run under
    screenshot: str | None             # relative path to the state's screenshot (PNG); required if "multimodal" in modality_available
    ax_tree: str | None                 # markdown-style accessibility tree text; required if "text" in modality_available
    ax_tree_source: str | None          # "real" (captured from a live app) | "synthetic" (generated, not from a live a11y API) | None
    elements: list[dict]               # real element list: [{"id","role","label","frame"}, ...] -- used for cropping/actuation, shown to neither modality directly
    elements_source: str                # "accessibility_api" | "cua_som" | "synthetic_spec" -- how `elements` was detected.
    entities: list[dict]                # [{"id","label","value"}, ...] source-of-truth values, if any
    options: list[OptionSpec]           # every (element, action) pair to score
    expected: dict[str, str]            # element_id -> gold action; fill also implies gold entity_id
    split: str = "public"               # "public" | "private"
    group: str | None = None            # paraphrase/variant group id, for consistency scoring
    provenance: dict = field(default_factory=dict)

    def __post_init__(self) -> None:
        if "multimodal" in self.modality_available and not self.screenshot:
            raise ValueError(f"task {self.id}: modality_available includes 'multimodal' but screenshot is None")
        if "text" in self.modality_available and not self.ax_tree:
            raise ValueError(f"task {self.id}: modality_available includes 'text' but ax_tree is None")

    def to_json(self) -> dict:
        d = asdict(self)
        return d

    @staticmethod
    def from_json(d: dict) -> "CuaTask":
        d = dict(d)
        d["options"] = [OptionSpec(**o) for o in d["options"]]
        return CuaTask(**d)


def dataset_hash(tasks: list[CuaTask]) -> str:
    """SHA-256 over the sorted canonical JSON of every task: a pre-registration
    mechanism -- freeze a dataset before any model sees it, so a later
    "improvement" to the eval set is visible as a hash change, not silently
    absorbed into a leaderboard number."""
    canon = json.dumps([t.to_json() for t in sorted(tasks, key=lambda t: t.id)],
                       sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canon.encode("utf-8")).hexdigest()


def load_jsonl(path: str | Path) -> list[CuaTask]:
    tasks = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if line.strip():
            tasks.append(CuaTask.from_json(json.loads(line)))
    return tasks


def save_jsonl(tasks: list[CuaTask], path: str | Path) -> None:
    with Path(path).open("w", encoding="utf-8") as f:
        for t in tasks:
            f.write(json.dumps(t.to_json(), ensure_ascii=False) + "\n")
