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
absorbed into a leaderboard number. That requires the hash to move only when
the content moves -- see `dataset_hash` and `stable_digest` for why a
`uuid.uuid4()` task id or a `hash()`-derived seed makes it move on every
regeneration instead, and so carry no information at all.
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
    "desktop_command_nav",  # pick the one desktop command (ribbon/menu item) whose FUNCTION achieves
                            # a goal stated as an outcome, against same-vocabulary distractors -- see
                            # datagen/desktop_nav.py; trained-on.
    "game_control",         # score candidate discrete-action "buttons" from a live game frame -- same
                             # one-pass scoring shape as the GUI families, applied to a real game loop
                             # (ViZDoom) instead of a static form; held out only, never trained on.
    "chess",                 # score candidate legal moves from a real chess position (rendered board
                              # image for multimodal, FEN-derived text for text modality) against a real
                              # Stockfish gold move; held out only, never trained on.
    "general_decision",       # a text-only typed bounded decision imported from an external benchmark
                               # (see datagen/external_bench_import.py); held out only.
    "cua_bench_basic",         # one real step of a real episode in a real, live `cua-bench`
                                # single-widget GUI env (datasets/cua-bench-basic), with the
                                # env's own reference solution as gold and real, grounded
                                # alternative actions as distractors (see
                                # datagen/cua_bench_basic.py); trained-on.
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

    @property
    def goal(self) -> str | None:
        """The user's stated goal for this task -- part of the observation, not
        a label. See `goal_text` for where it is read from and why it is
        derived rather than stored.

        An adapter that ignores this is answering an underspecified question:
        on a screen of sixteen lookalike cells whose gold action is a click on
        exactly one of them, the goal is the only thing that says which."""
        return goal_text(self)

    def to_json(self) -> dict:
        d = asdict(self)
        return d

    @staticmethod
    def from_json(d: dict) -> "CuaTask":
        d = dict(d)
        d["options"] = [OptionSpec(**o) for o in d["options"]]
        # `goal` is derived (a property), never stored. A serialized task that
        # carries one -- e.g. written by a tool that read `to_json` output back
        # through a schema that materialized the property -- is accepted and
        # dropped rather than crashing the load.
        d.pop("goal", None)
        return CuaTask(**d)


# Provenance keys that carry the USER'S ACTUAL GOAL, as
# (episode_level_key, step_level_key) pairs, one pair per data source.
#
# Every generator and converter RECORDS the goal into `provenance`, and
# `goal_text` is the single place that reads it. The real-capture converters
# already stored it -- GUI-360's `request`/`subtask`, AndroidControl's
# `goal`/`step_instruction` -- but nothing ever surfaced it, so a step whose
# accessibility tree is sixteen lookalike spreadsheet cells and whose gold is a
# click on exactly one of them was shown with nothing saying which. Tasks like
# that are not hard, they are UNDERSPECIFIED: not solvable above chance by
# anyone, so training on them teaches a positional prior and evaluating on them
# measures that prior.
#
# Why the goal is read from `provenance` rather than rendered into `ax_tree` at
# generation time: a converted real capture is frozen on disk and cannot be
# regenerated, and its `ax_tree_source` is "real" -- a claim that the text is
# what the live accessibility API reported. Injecting a synthesized instruction
# into that field would both miss every already-generated split and falsify
# that claim. Reading a key the converter already wrote reaches the frozen
# splits and changes no bytes, so `dataset_hash` is preserved.
_GOAL_KEY_PAIRS = (
    ("request", "subtask"),                # GUI-360
    ("goal", "step_instruction"),           # AndroidControl
    ("synthetic_goal", "synthetic_step"),   # datagen/generator.py (synthetic apps)
)


def goal_text(task: CuaTask) -> str | None:
    """The goal a prompt builder still needs to state, or None if there is none
    left to state.

    Returns the episode-level goal and, when it adds information, the
    step-level instruction as well -- both real, human-authored (or, for the
    synthetic apps, hand-written) strings from the task's own record, never a
    derived label. A step instruction that merely repeats the episode goal is
    emitted once, not twice.

    Returns None when `provenance["goal_in_state"]` is set, which the synthetic
    generator sets because it renders the goal into the observation itself (see
    datagen/generator.py). Those tasks already show the goal in their `ax_tree`
    and in their screenshot's pixels, so a prompt builder that also prepended
    this would print it twice. That flag is the single invariant keeping the two
    placements from colliding; use `provenance["synthetic_goal"]` directly if you
    want the raw string for such a task regardless.

    This is deliberately NOT a stored dataclass field: `provenance` is the
    hashed source of truth, and a value derived from it carries no information
    `dataset_hash` does not already cover. Storing it as a hashed field instead
    would change the hash of every existing split; storing it as an unhashed
    field would let a generator change a task's goal without the hash moving,
    which is exactly the blind spot `dataset_hash` exists to avoid.
    """
    prov = task.provenance or {}
    if prov.get("goal_in_state"):
        return None
    for episode_key, step_key in _GOAL_KEY_PAIRS:
        episode = (prov.get(episode_key) or "").strip()
        step = (prov.get(step_key) or "").strip()
        if not episode and not step:
            continue
        if episode and step and step != episode:
            return f"{episode}\nCurrent step: {step}"
        return episode or step
    return None


def resolve_screenshot(task: CuaTask, dataset_root: str | Path) -> Path | None:
    """Where `task.screenshot` actually lives on disk.

    A task's `screenshot` is stored relative to its own dataset root, which is
    what makes a dataset directory relocatable, and nothing in the task says
    where that root is. An adapter that opens the stored string directly
    resolves it against the process CWD instead, so it only works when launched
    from inside the dataset directory. Pass the directory containing the
    split's jsonl.

    The stored string is never rewritten: it is part of the content
    `dataset_hash` covers, so absolutizing it on load would change the hash of
    every screenshot-bearing split.
    """
    if not task.screenshot:
        return None
    p = Path(task.screenshot)
    if p.is_absolute() or p.exists():
        return p
    return Path(dataset_root) / p


# The fields that make two tasks the SAME DECISION: everything a model can see
# plus the gold it is scored against. Deliberately excludes `id`, `screenshot`
# (a path), `provenance` (seeds, flags) and `split`/`group` -- two tasks drawn
# from different seeds that render the same screen and carry the same gold are
# the same task for train/test purposes, however differently they are labelled.
_CONTENT_FIELDS = ("family", "app", "ax_tree", "elements", "entities", "options",
                   "expected", "modality_available")


def content_key(task: CuaTask) -> str:
    """A stable key identifying a task's DECISION CONTENT.

    Use this, not `(app_id, index)` or the task id, whenever tasks are bucketed
    into train/val/test. Bucketing on a generation index guarantees that no
    index crosses splits but says nothing about content: two different indices
    draw different seeds and can still render an identical screen with an
    identical option set and gold, which silently puts the same task in train
    and in test. Measured on this generator before `ContentDeduper` existed,
    8.1% of one test split's tasks also appeared in its train split -- enough
    to inflate any same-distribution or finetuned-on-own-split number.
    """
    d = task.to_json()
    canon = json.dumps({k: d.get(k) for k in _CONTENT_FIELDS},
                       sort_keys=True, separators=(",", ":"), default=str)
    return hashlib.sha256(canon.encode("utf-8")).hexdigest()


class ContentDeduper:
    """Tracks task content across splits and rejects duplicates.

    `accept(task, split)` returns False when this task's `content_key` has
    already been placed -- in this split or any other. Rejecting across splits
    is the point: a duplicate inside one split merely wastes a sample, while a
    duplicate spanning train and test is contamination.

    Generators should treat a rejection as "draw another sample", not as an
    error: the caller owns how many attempts are worth making before giving up
    on a slot (see `datagen.generator.generate_dataset`).
    """

    def __init__(self) -> None:
        self._by_key: dict[str, str] = {}
        self.rejected = 0

    def accept(self, task: CuaTask, split: str = "") -> bool:
        key = content_key(task)
        if key in self._by_key:
            self.rejected += 1
            return False
        self._by_key[key] = split
        return True

    def placed_split(self, task: CuaTask) -> str | None:
        """Which split this task's content was already placed in, if any."""
        return self._by_key.get(content_key(task))

    def __len__(self) -> int:
        return len(self._by_key)


def stable_digest(*parts: object) -> int:
    """A 64-bit digest of `parts` that is identical in every process and on
    every platform.

    Use this, never the built-in `hash()`, anywhere a value derives a random
    seed or buckets a task into a split. `hash()` is salted per process for
    `str`/`bytes` (that is what `PYTHONHASHSEED` controls), so
    `seed + hash((app_id, i))` and `hash((app_id, i)) % 2` silently produce a
    different dataset -- different task content AND a different train/val/test
    assignment -- on every single run, which makes a generated dataset
    impossible to reproduce or to review a change to. Callers should not have
    to remember to set an environment variable to get determinism.
    """
    blob = "\x1f".join(repr(p) for p in parts).encode("utf-8")
    return int.from_bytes(hashlib.blake2b(blob, digest_size=8).digest(), "big")


def dataset_hash(tasks: list[CuaTask]) -> str:
    """SHA-256 over the sorted canonical JSON of every task: a pre-registration
    mechanism -- freeze a dataset before any model sees it, so a later
    "improvement" to the eval set is visible as a hash change, not silently
    absorbed into a leaderboard number.

    For that to be a real signal the hash has to answer "did the content
    change", and *only* that -- a hash that also moves for reasons unrelated to
    content is indistinguishable from no hash at all, because every
    regeneration then looks like a change and so no regeneration looks like
    one. That requires **deterministic task ids**: a generator minting ids from
    `uuid.uuid4()` re-hashes differently on every run of identical code, so
    re-running a generation could never confirm a dataset was unchanged. The
    synthetic generator derives its ids with `stable_digest`, and the
    real-capture converters key theirs off the source episode/step, so
    regenerating any split from the same inputs reproduces its hash exactly.

    Known remaining sensitivity, deliberately not normalized away: `screenshot`
    is a path, and the same task's image is named differently depending on
    where the dataset directory sits ("<id>.png" as generated,
    "screenshots/<id>.png" once packaged, or an absolute path in a relocated
    copy). Hashing only the basename would be more faithful to "content", but
    it would change the hash of every screenshot-bearing split already recorded
    in this repo's manifests and eval summaries, so it is left alone: compare
    hashes between splits laid out the same way, and treat a relocation as the
    re-registration it currently looks like.
    """
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
