"""Native task specs for jev-use (RFC #4268, Phase 1).

A ``NativeTask`` declares one application task: its goal, parameters (text is
taken only from these), the window it may observe and act on, the allowed
action kinds, opt-in risk categories, whether foreground delivery is allowed,
a step budget, and an app-owned success oracle. It builds each step's closed
candidate set from the native accessibility source, plus visual regions only
under the fallback rule, and composes them in the fixed order page, ax, visual.

The built-in tasks drive the repository's harness applications in task mode:
AppKit (``CUA_APPKIT_TASK_STATE``), WPF (``CUA_WPF_TASK_STATE``), WinUI3
(``CUA_WINUI3_TASK_STATE``), and GTK3 (``CUA_GTK3_TASK_STATE``), under
``libs/cua-driver/tests/fixtures/apps``.
Their oracle is the harness's own JSON state file, which the app rewrites on
every change; it never depends on Driver output.

``canvas-cancel`` drives the cross-platform visual-only canvas fixture
(``tests/fixtures/apps/cross-platform/visual-only-canvas``), a custom-painted
Tk surface with no accessibility tree. Its only executable candidate is a
capture-bound visual click, so it proves the OmniParser fallback. The fixture
publishes its state to a loopback journal; the caller (``verify_native.py``)
writes each published state to the task's state file unchanged except for the
schema name.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Callable, Literal, Mapping

from choose_action import (
    MAX_ELEMENTS,
    MAX_HISTORY,
    MAX_PROGRESS,
    MAX_PROGRESS_COUNT,
    REQUEST_SCHEMA_V2,
)
from native import (
    PARAMETER_NAME_PATTERN,
    NativeControl,
    element_state,
    field_state,
    has_application_elements,
    slug,
)
from native_roles import ACTION_KINDS
from sources import Candidate, NativeAccessibilitySource, TextMethod
from tasks import Outcome, TaskParameter, TaskSources, redact_token

MAX_EXECUTABLE_CANDIDATES = 24
SOURCE_ORDER = ("page", "ax", "visual")
Check = Literal["verified", "refuted", "pending"]

NATIVE_RESERVED = (
    Candidate(
        "reobserve",
        "Take no action and obtain a fresh observation of the window, because the current "
        "observation looks stale, incomplete, or contradicts the goal's progress.",
        None,
        {},
    ),
    Candidate(
        "abstain",
        "Stop without acting if none of the proposed actions is safe or moves toward the goal.",
        None,
        {},
    ),
)

# Words too common to make a control relevant to a goal (#4312).
_RELEVANCE_STOPWORDS = frozenset(
    {"the", "and", "then", "once", "per", "step", "stop", "into", "with", "for", "from",
     "this", "that", "its", "each", "exactly", "starts", "set", "option", "field", "button"}
)


def _relevance_words(text: str) -> frozenset[str]:
    return frozenset(
        word for word in re.findall(r"[a-z0-9]+", text.lower())
        if len(word) >= 3 and word not in _RELEVANCE_STOPWORDS
    )


_VERBS = {
    "press": "pressed",
    "toggle": "toggled",
    "select": "selected",
    "open_menu": "opened",
    "visual_click": "clicked the visual region",
}


@dataclass(frozen=True)
class WindowScope:
    """The one window a task may observe and act on."""

    window_title: str
    bundle_id: str | None = None
    process_name: str | None = None
    query: str | None = None
    max_elements: int | None = None
    max_depth: int | None = None

    def window_state_arguments(self) -> dict[str, Any]:
        arguments: dict[str, Any] = {}
        if self.query is not None:
            arguments["query"] = self.query
        if self.max_elements is not None:
            arguments["max_elements"] = self.max_elements
        if self.max_depth is not None:
            arguments["max_depth"] = self.max_depth
        return arguments


class OracleError(RuntimeError):
    pass


@dataclass(frozen=True)
class AppStateOracle:
    """An ``app_check`` oracle over an app-owned JSON state file.

    The file must carry the expected ``schema`` and, when given, the ``pid`` of
    the exact app process under test, so a stale file from another run never
    counts as evidence.
    """

    path: Path
    schema: str
    expected_pid: int | None = None

    def read(self) -> dict[str, Any]:
        try:
            state = json.loads(Path(self.path).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise OracleError(f"app state is unreadable: {type(error).__name__}") from None
        if not isinstance(state, dict) or state.get("schema") != self.schema:
            raise OracleError("app state has an unexpected schema")
        if self.expected_pid is not None and state.get("pid") != self.expected_pid:
            raise OracleError("app state belongs to a different process")
        return state


@dataclass(frozen=True)
class ComposeStats:
    """Content-free counts logged for every step."""

    sources: Mapping[str, int]
    duplicates: int
    risk_excluded: Mapping[str, int]
    dropped: int

    def to_log(self) -> dict[str, Any]:
        return {
            "sources": dict(self.sources),
            "duplicates": self.duplicates,
            "risk_excluded": dict(self.risk_excluded),
            "dropped": self.dropped,
        }


def compose(
    groups: Mapping[str, list[Candidate]],
    *,
    allowed_risks: frozenset[str],
    reserved: tuple[Candidate, ...] = NATIVE_RESERVED,
    cap: int = MAX_EXECUTABLE_CANDIDATES,
    relevance: Callable[[Candidate], int] | None = None,
) -> tuple[list[Candidate], ComposeStats]:
    """Merge source outputs in the order page, ax, visual.

    Duplicate IDs are dropped deterministically (first source wins). A
    candidate tagged with a risk category the task did not allow is removed.
    At most ``cap`` executable candidates remain; the count dropped is
    reported, never silently truncated. The reserved ``reobserve`` and
    ``abstain`` candidates are always appended.

    Without ``relevance``, the first ``cap`` candidates in source then element
    (depth-first) order are kept. With ``relevance`` (lower is more relevant),
    a set over the cap keeps the ``cap`` candidates with the lowest
    ``(relevance, depth-first position)`` and still presents them in
    depth-first order (#4312), so ranking decides only which candidates
    survive, never where they appear. A set within the cap is unchanged.
    """
    seen: set[str] = {candidate.id for candidate in reserved}
    merged: list[Candidate] = []
    duplicates = 0
    risk_excluded: dict[str, int] = {}
    for source in SOURCE_ORDER:
        for candidate in groups.get(source, []):
            if candidate.id in seen:
                duplicates += 1
                continue
            seen.add(candidate.id)
            blocked = sorted(candidate.risk - allowed_risks)
            if blocked:
                for category in blocked:
                    risk_excluded[category] = risk_excluded.get(category, 0) + 1
                continue
            merged.append(candidate)
    if relevance is None or len(merged) <= cap:
        kept = merged[:cap]
    else:
        ranked = sorted(range(len(merged)), key=lambda index: (relevance(merged[index]), index))
        kept = [merged[index] for index in sorted(ranked[:cap])]
    counts = {source: 0 for source in SOURCE_ORDER}
    for candidate in kept:
        if candidate.source in counts:
            counts[candidate.source] += 1
    stats = ComposeStats(counts, duplicates, risk_excluded, len(merged) - len(kept))
    return kept + list(reserved), stats


@dataclass(frozen=True)
class TaskStep:
    """One step a task requires, counted from the runner's own performed actions.

    ``candidate_id`` names the candidate that performs the step; its
    ``:foreground`` variant counts too. The description is task-authored and
    value-free: it may name a parameter, never its value. A step with
    ``after_previous`` must wait for every earlier step to be done.
    """

    description: str
    candidate_id: str
    times: int = 1
    after_previous: bool = True


def performed_counts(history: list[Mapping[str, Any]]) -> dict[str, int]:
    """Count the actions this run dispatched successfully, by candidate ID.

    Only history entries marked ``performed`` count: a refused, stale, or
    reobserve step did nothing. Nothing here is read from the application.
    """
    counts: dict[str, int] = {}
    for item in history:
        if item.get("performed") is True:
            base = str(item["selected_id"]).removesuffix(":foreground")
            counts[base] = counts.get(base, 0) + 1
    return counts


@dataclass(frozen=True)
class NativeStep:
    """One step's closed candidate set and its model-safe context."""

    candidates: list[Candidate]
    stats: ComposeStats
    elements: list[dict[str, str]]
    outcomes: Mapping[str, str]


def _quoted(label: str, limit: int = 60) -> str:
    return json.dumps(label if len(label) <= limit else label[: limit - 1] + "…", ensure_ascii=False)


@dataclass(frozen=True)
class NativeTask:
    """A declarative native task. See the module docstring."""

    id: str
    goal: str
    scope: WindowScope
    allowed_actions: frozenset[str]
    oracle: AppStateOracle
    check: Callable[[Mapping[str, Any]], Check]
    parameters: tuple[TaskParameter, ...] = ()
    allowed_risks: frozenset[str] = frozenset()
    allow_foreground: bool = False
    max_steps: int = 6
    text_method: TextMethod = "set_value"
    visual_targets: tuple[str, ...] = ()
    # OCR confidence a visual target must reach; the default matches the
    # browser path. Exact-text uniqueness still applies at any bar.
    visual_min_confidence: float = 0.8
    mock_preferences: tuple[str, ...] = ()
    # Ordered steps the task requires (#4313). The request reports how often
    # this run has performed each, and a step's candidate names any earlier
    # step that is not done yet, so a model need not infer order or count
    # from history. Empty means the request carries no progress.
    steps: tuple[TaskStep, ...] = ()
    # How a set over the cap is cut (#4312): "relevance" keeps the declared
    # steps' candidates first; "depth_first" keeps the first ``cap`` in element
    # order, as before. Both present the kept candidates in element order.
    cap_order: Literal["relevance", "depth_first"] = "relevance"
    # The oracle is polled after every action, so no candidate is special.
    completion_candidate_ids: frozenset[str] = field(default=frozenset(), init=False)

    def __post_init__(self) -> None:
        unknown = set(self.allowed_actions) - ACTION_KINDS
        if unknown:
            raise ValueError(f"unknown action kinds: {sorted(unknown)}")
        for parameter in self.parameters:
            if not PARAMETER_NAME_PATTERN.fullmatch(parameter.name):
                raise ValueError("parameter names must match [a-z][a-z0-9_]{0,7}")
        if self.cap_order not in ("relevance", "depth_first"):
            raise ValueError("cap_order must be relevance or depth_first")
        if self.max_steps < 1:
            raise ValueError("max_steps must be positive")
        if len(self.steps) > MAX_PROGRESS:
            raise ValueError(f"a task declares at most {MAX_PROGRESS} steps")
        for task_step in self.steps:
            if not 1 <= task_step.times <= MAX_PROGRESS_COUNT:
                raise ValueError(f"step times must be from 1 to {MAX_PROGRESS_COUNT}")

    @property
    def allowed_action_kinds(self) -> frozenset[str]:
        """The Driver tools this task can dispatch."""
        tools = set()
        if self.allowed_actions & {"press", "toggle", "select", "open_menu", "visual_click"}:
            tools.add("click")
        if "set_text" in self.allowed_actions:
            tools.add(self.text_method)
        return frozenset(tools)

    def redact(self, value: Any) -> Any:
        for parameter in self.parameters:
            if parameter.secret:
                value = redact_token(value, parameter.value, parameter.redaction)
        return value

    def redact_text(self, value: str) -> str:
        return self.redact(value)

    # -- candidates -------------------------------------------------------

    def _native_candidates(
        self, ax: NativeAccessibilitySource, foreground_ids: frozenset[str]
    ) -> tuple[list[Candidate], dict[str, str], dict[str, str]]:
        candidates: list[Candidate] = []
        outcomes: dict[str, str] = {}
        labels: dict[str, str] = {}
        for native in ax.controls:
            if native.action not in self.allowed_actions:
                continue
            control = ax.control(native)
            label = _quoted(native.label)
            if native.action == "set_text":
                for parameter in self.parameters:
                    if native.value == parameter.value:
                        continue
                    candidate_id = f"{native.id}:set:{parameter.name}"
                    state = field_state(native.value, parameter.value)
                    candidates.append(
                        ax.type_text(
                            control,
                            parameter.value,
                            candidate_id=candidate_id,
                            description=(
                                f"Set the text field {label} to the task parameter "
                                f"{json.dumps(parameter.name)}, replacing its contents. The field "
                                f"currently reports {state}."
                            ),
                        )
                    )
                    outcomes[candidate_id] = f"set {label} to parameter {parameter.name}"
                    labels[candidate_id] = native.label
                continue
            if native.action == "select" and native.selected:
                continue  # selecting an already selected option is a no-op
            description = self._describe(native, label)
            candidate_id = native.id
            delivery: Literal["background", "foreground"] = "background"
            if native.id in foreground_ids:
                if not self.allow_foreground:
                    continue
                candidate_id = f"{native.id}:foreground"
                delivery = "foreground"
                description += (
                    " Use foreground delivery, which activates the window, because Driver "
                    "refused background delivery for this control."
                )
            candidates.append(
                ax.click(control, candidate_id=candidate_id, description=description, delivery=delivery)
            )
            outcomes[candidate_id] = f"{_VERBS[native.action]} {label}"
            labels[candidate_id] = native.label
        return candidates, outcomes, labels

    @staticmethod
    def _describe(native: NativeControl, label: str) -> str:
        kind = native.role_class
        if native.action == "toggle":
            now = "checked" if native.selected else "unchecked"
            after = "unchecked" if native.selected else "checked"
            noun = "switch" if kind == "toggle" else "checkbox"
            return f"Toggle the {noun} {label}. It is currently {now}; afterward it will be {after}."
        if native.action == "select":
            return f"Select the radio option {label}. It is currently not selected."
        if native.action == "open_menu":
            return (
                f"Open the pop-up menu {label}. Its options become candidates on the next "
                "observation."
            )
        noun = {"menu_item": "menu item", "link": "link"}.get(kind, "button")
        return f"Press the {noun} labeled {label}."

    def _visual_candidates(self, sources: TaskSources) -> tuple[list[Candidate], dict[str, str]]:
        visual = sources.visual
        if visual is None or "visual_click" not in self.allowed_actions:
            return [], {}
        candidates: list[Candidate] = []
        outcomes: dict[str, str] = {}
        for target in self.visual_targets:
            control = visual.find("button", target)
            if control is None:
                continue
            candidate_id = f"visual:{slug(target)}"
            description = (
                f"Click the unique validated visual region reading {_quoted(target)}; "
                "no accessibility element covers it."
            )
            source = visual
            if candidate_id in sources.foreground_ids:
                if not self.allow_foreground:
                    continue
                candidate_id = f"{candidate_id}:foreground"
                description += (
                    " Use foreground delivery, which activates the window, because Driver "
                    "refused background delivery for this region."
                )
                source = replace(visual, delivery="foreground")
            candidate = source.click(
                control,
                candidate_id=candidate_id,
                description=description,
            )
            if candidate is not None:
                candidates.append(candidate)
                outcomes[candidate_id] = f"clicked the visual region {_quoted(target)}"
        return candidates, outcomes

    def relevance(self, labels: Mapping[str, str]) -> Callable[[Candidate], int]:
        """Rank candidates for the cap only (#4312); lower is more relevant.

        Tier 0 is a candidate that performs one of the task's declared steps or
        clicks a declared visual target (a ``:foreground`` variant counts).
        Tier 1 is a control whose label shares a word with the goal. Tier 2 is
        everything else. The rank uses only task-authored text and control
        labels, never values, and never changes the presented order.
        """
        declared = {task_step.candidate_id for task_step in self.steps} | {
            f"visual:{slug(target)}" for target in self.visual_targets
        }
        goal_words = _relevance_words(self.goal)

        def tier(candidate: Candidate) -> int:
            if candidate.id.removesuffix(":foreground") in declared:
                return 0
            if _relevance_words(labels.get(candidate.id, "")) & goal_words:
                return 1
            return 2

        return tier

    def plan(self, sources: TaskSources) -> NativeStep:
        """Build the step's closed candidate set, stats, and compact elements."""
        groups: dict[str, list[Candidate]] = {}
        outcomes: dict[str, str] = {}
        labels: dict[str, str] = {}
        elements: list[dict[str, str]] = []
        if sources.ax is not None:
            groups["ax"], native_outcomes, labels = self._native_candidates(
                sources.ax, sources.foreground_ids
            )
            outcomes.update(native_outcomes)
            elements = [
                {
                    "role_class": control.role_class,
                    "label": control.label,
                    "state": element_state(control),
                }
                for control in sources.ax.controls[:MAX_ELEMENTS]
            ]
        groups["visual"], visual_outcomes = self._visual_candidates(sources)
        outcomes.update(visual_outcomes)
        candidates, stats = compose(
            groups,
            allowed_risks=self.allowed_risks,
            relevance=self.relevance(labels) if self.cap_order == "relevance" else None,
        )
        for candidate in candidates:
            if candidate.tool is not None and candidate.tool not in self.allowed_action_kinds:
                raise ValueError(f"task {self.id} does not allow action kind {candidate.tool}")
        return NativeStep(candidates, stats, elements, outcomes)

    def candidates(self, sources: TaskSources) -> list[Candidate]:
        return self.plan(sources).candidates

    def state_summary(self, sources: TaskSources) -> dict[str, str]:
        """Value-free control states, keyed by stable candidate ID."""
        if sources.ax is None:
            return {}
        return {control.id: element_state(control) for control in sources.ax.controls}

    def history_entry(
        self,
        step: int,
        candidate_id: str,
        *,
        refusal: str | None = None,
        outcome: str | None = None,
        stale: bool = False,
    ) -> dict[str, Any]:
        if stale:
            text = "the observation was stale; nothing happened and the window is observed again"
        elif refusal is not None:
            text = f"Driver refused background delivery ({refusal}); nothing happened"
        elif candidate_id == "reobserve":
            text = "took no action and requested a fresh observation"
        else:
            text = outcome or "completed"
        entry: dict[str, Any] = {
            "step": step,
            "selected_id": candidate_id,
            "outcome": self.redact(text)[:128],
        }
        if not stale and refusal is None and candidate_id not in {"reobserve", "abstain"}:
            entry["performed"] = True  # runner-side only; never sent to a provider
        return entry

    # -- progress ---------------------------------------------------------

    def progress(self, history: list[Mapping[str, Any]]) -> list[dict[str, Any]]:
        """Each declared step and how often this run has performed it."""
        counts = performed_counts(history)
        return [
            {
                "step": self.redact(task_step.description),
                "done": min(counts.get(task_step.candidate_id, 0), MAX_PROGRESS_COUNT),
                "required": task_step.times,
            }
            for task_step in self.steps
        ]

    def step_note(self, candidate_id: str, history: list[Mapping[str, Any]]) -> str:
        """A sentence stating a candidate's place in the task's declared steps.

        A step that is already done says so; a step with an earlier step not yet
        done names that step as its precondition; a step that is due says how
        many more times the task requires it. Other candidates get nothing.
        """
        base = candidate_id.removesuffix(":foreground")
        counts = performed_counts(history)
        for index, task_step in enumerate(self.steps):
            if task_step.candidate_id != base:
                continue
            if counts.get(base, 0) >= task_step.times:
                return (
                    f" This run already did this the {task_step.times} time(s) the task requires."
                )
            pending = [
                earlier.description
                for earlier in (self.steps[:index] if task_step.after_previous else ())
                if counts.get(earlier.candidate_id, 0) < earlier.times
            ]
            if pending:
                return (
                    " The task requires this only after: "
                    + "; ".join(pending)
                    + " (not done yet)."
                )
            remaining = task_step.times - counts.get(base, 0)
            return f" The task still requires this {remaining} more time(s)."
        return ""

    def expected_next(self, history: list[Mapping[str, Any]]) -> list[str]:
        """The candidate IDs that correctly advance the task now, for measurement.

        A declared step is due when this run has performed it fewer times than
        required and, for a step that waits, every earlier step is done. Empty
        when the task declares no steps or every step is done. Counted only
        from the runner's own performed actions, like ``progress``.
        """
        counts = performed_counts(history)
        due = []
        for index, task_step in enumerate(self.steps):
            if counts.get(task_step.candidate_id, 0) >= task_step.times:
                continue
            earlier = self.steps[:index] if task_step.after_previous else ()
            if all(counts.get(step.candidate_id, 0) >= step.times for step in earlier):
                due.append(task_step.candidate_id)
        return due

    def reset(self) -> None:
        """The harness starts fresh for every run; there is nothing to reset."""

    def read_oracle(self) -> Mapping[str, Any]:
        return self.oracle.read()

    def classify(self, oracle: Mapping[str, Any], *, steps: int) -> Outcome:
        result = self.check(oracle)
        if result in {"verified", "refuted"}:
            return result  # type: ignore[return-value]
        if steps >= self.max_steps:
            return "budget_exhausted"
        return "unknown"


def visual_fallback_reason(sources: TaskSources, task: NativeTask, native_count: int) -> str | None:
    """Return why this step may parse visual regions, or ``None``.

    Visual regions are consulted only when the task allows ``visual_click`` and
    declares visual targets, and one of these holds: Driver reported the tree
    empty; the tree is not truncated but has no application elements (only
    window roots and window chrome, as for a custom-painted surface); or a
    complete tree has no native candidate (or no native control labeled like a
    declared target). A truncated tree, or a partial tree that does contain
    application elements, never qualifies.
    """
    ax = sources.ax
    if ax is None or "visual_click" not in task.allowed_actions or not task.visual_targets:
        return None
    observation = ax.observation
    if observation.capture_id is None:
        return None
    if observation.tree_empty:
        return "tree_empty"
    if observation.truncated:
        return None
    if not observation.complete:
        # A partial tree qualifies only when it holds nothing but window roots
        # and window chrome, as for a custom-painted surface.
        if has_application_elements(observation, ax.platform):
            return None
        return "no_application_elements"
    if native_count == 0:
        return "no_native_candidates"
    labels = {control.label.lower() for control in ax.controls}
    if any(target.lower() not in labels for target in task.visual_targets):
        return "target_without_element"
    return None


def visual_regions_wire(sources: TaskSources, task: NativeTask) -> list[dict[str, Any]]:
    if sources.visual is None:
        return []
    return [
        task.redact(
            {
                "id": region.id,
                "kind": region.kind,
                "bounds": {
                    "x": region.x,
                    "y": region.y,
                    "width": region.width,
                    "height": region.height,
                },
                "text": region.text,
                "label": region.label,
                "confidence": region.confidence,
                "interactive": region.interactive,
            }
        )
        for region in sources.visual.observation.regions
    ]


def native_choice_request(
    task: NativeTask,
    sources: TaskSources,
    step: NativeStep,
    history: list[Mapping[str, Any]],
) -> dict[str, Any]:
    """Build the ``cua.jev_choice_request_v2`` the provider receives.

    It carries candidate IDs, descriptions and sources, compact value-free
    elements, compact history, and, for a task that declares steps, the
    progress counted from this run's performed actions. Element tokens,
    values, and pixels stay in the runner.
    """
    if sources.ax is None or sources.ax.observation.capture_id is None:
        raise ValueError("a native request needs an observation with a capture_id")
    observation = sources.ax.observation
    candidates = []
    for candidate in step.candidates:
        description = candidate.description + task.step_note(candidate.id, history)
        item: dict[str, Any] = {"id": candidate.id, "description": task.redact(description)}
        if candidate.source is not None:
            item["source"] = candidate.source
        candidates.append(item)
    request: dict[str, Any] = {
        "schema": REQUEST_SCHEMA_V2,
        "goal": task.redact(task.goal),
        "capture_id": observation.capture_id,
        "snapshot_id": observation.snapshot_id,
        "regions": visual_regions_wire(sources, task),
        "elements": [task.redact(item) for item in step.elements],
        "history": [
            {"selected_id": item["selected_id"], "outcome": item["outcome"]}
            for item in history[-MAX_HISTORY:]
        ],
        "candidates": candidates,
    }
    if task.steps:
        request["progress"] = task.progress(history)
    return request


# -- Harness tasks ------------------------------------------------------------
#
# The same three tasks run on every repository harness that has a task mode:
# AppKit (macOS AX), WPF and WinUI3 (Windows UIA), and GTK3 (Linux AT-SPI). Each harness
# shows the same labeled controls in task mode (Increment, Reset, I agree,
# Small/Medium/Large, Note, Save note, Exit) and rewrites the same app-owned
# JSON state file, so the task semantics, candidate IDs, and mock choices are
# identical across platforms. Only the window, the state schema, and the
# platform role table differ.


@dataclass(frozen=True)
class HarnessSpec:
    """A repository harness application that has a jev-use task mode."""

    name: str
    platform: str
    window_title: str
    state_schema: str
    state_env: str
    bundle_id: str | None = None
    process_name: str | None = None


HARNESSES: Mapping[str, HarnessSpec] = {
    "appkit": HarnessSpec(
        "appkit", "macos", "CuaTestHarness AppKit", "cua.appkit_task_state_v1",
        "CUA_APPKIT_TASK_STATE", bundle_id="com.trycua.harness.appkit",
    ),
    # WPF, WinUI3, and GTK3 show a dedicated task window in task mode: their
    # ordinary main windows scroll, so most controls would be off screen (and
    # excluded).
    "wpf": HarnessSpec(
        "wpf", "windows", "CuaTestHarness WPF Tasks", "cua.wpf_task_state_v1",
        "CUA_WPF_TASK_STATE", process_name="CuaTestHarness.Wpf",
    ),
    "winui3": HarnessSpec(
        "winui3", "windows", "CuaTestHarness WinUI3 Tasks", "cua.winui3_task_state_v1",
        "CUA_WINUI3_TASK_STATE", process_name="CuaTestHarness.WinUI3",
    ),
    "gtk3": HarnessSpec(
        "gtk3", "linux", "CuaTestHarness GTK3 Tasks", "cua.gtk3_task_state_v1",
        "CUA_GTK3_TASK_STATE", process_name="python3",
    ),
}

APPKIT_WINDOW_TITLE = HARNESSES["appkit"].window_title
APPKIT_BUNDLE_ID = HARNESSES["appkit"].bundle_id
APPKIT_STATE_SCHEMA = HARNESSES["appkit"].state_schema
APPKIT_STATE_ENV = HARNESSES["appkit"].state_env
COUNTER_TARGET = 3
DEFAULT_NOTE_TEXT = "jev-use native note"
TARGET_SIZE = "large"
TASK_KINDS = ("counter", "save-note", "choose-size")


def _counter_check(state: Mapping[str, Any]) -> Check:
    counter = state.get("counter")
    if counter == COUNTER_TARGET:
        return "verified"
    if isinstance(counter, int) and counter > COUNTER_TARGET:
        return "refuted"  # Reset is a destructive control this task never offers
    return "pending"


def _note_check(note: str) -> Callable[[Mapping[str, Any]], Check]:
    def check(state: Mapping[str, Any]) -> Check:
        saved = state.get("note_saved")
        if saved == note:
            return "verified"
        if saved is not None:
            return "refuted"
        return "pending"

    return check


def _size_check(state: Mapping[str, Any]) -> Check:
    if state.get("size") == TARGET_SIZE and state.get("agreed") is True:
        return "verified"
    return "pending"


def _canvas_check(state: Mapping[str, Any]) -> Check:
    selected = state.get("selected")
    if selected == slug(CANVAS_TARGET) and state.get("action_count") == 1:
        return "verified"
    if selected is not None:
        return "refuted"  # another card, or Save clicked more than once
    return "pending"


def harness_task_ids(harness: str) -> tuple[str, ...]:
    return tuple(f"{harness}-{kind}" for kind in TASK_KINDS)


# The cross-platform visual-only canvas has no accessibility tree, so it is not
# a form harness: it has one task, reached only through the visual fallback.
CANVAS = HarnessSpec(
    "canvas", "any", "Cua Visual-Only Canvas Fixture", "cua.visual_canvas_task_state_v1",
    "CUA_CANVAS_TASK_STATE",
)
CANVAS_TASK_ID = "canvas-cancel"
# Cancel, not Save: OmniParser reads the 32 px "Save" label inconsistently on a
# 1x macOS capture ("Save", then "Saye"), and the exact text match (correctly)
# withholds the candidate. "Send" is a risk phrase. "Cancel" reads reliably.
CANVAS_TARGET = "Cancel"
# A painted surface has no deeper tree. Walking only the window's top level
# keeps macOS from spending the walk budget in the application menu bar (which
# would truncate the tree and forbid the fallback). Any content container still
# appears at depth 1 and blocks the fallback.
CANVAS_MAX_DEPTH = 1
CANVAS_MIN_CONFIDENCE = 0.8

APPKIT_TASK_IDS = harness_task_ids("appkit")
NATIVE_TASK_IDS = (
    *(task for name in HARNESSES for task in harness_task_ids(name)),
    CANVAS_TASK_ID,
)


def split_task_id(task_id: str) -> tuple[HarnessSpec, str]:
    """Return the harness and task kind of a built-in task ID."""
    if task_id == CANVAS_TASK_ID:
        return CANVAS, "cancel"
    harness, _, kind = task_id.partition("-")
    if harness not in HARNESSES or kind not in TASK_KINDS:
        raise ValueError(f"unknown native task: {task_id}")
    return HARNESSES[harness], kind


def native_task(
    task_id: str,
    state_path: Path,
    *,
    pid: int | None = None,
    note_text: str = DEFAULT_NOTE_TEXT,
    allow_foreground: bool = False,
) -> NativeTask:
    """Build one harness task bound to its app-owned state file."""
    harness, kind = split_task_id(task_id)
    oracle = AppStateOracle(Path(state_path), harness.state_schema, pid)
    scope = WindowScope(
        harness.window_title, bundle_id=harness.bundle_id, process_name=harness.process_name
    )
    if kind == "cancel":
        return NativeTask(
            id=task_id,
            goal=(
                "The window is a painted canvas with Save, Send, and Cancel cards. "
                f"Click the {CANVAS_TARGET} card once, then stop."
            ),
            scope=replace(scope, max_depth=CANVAS_MAX_DEPTH),
            allowed_actions=frozenset({"visual_click"}),
            oracle=oracle,
            check=_canvas_check,
            allow_foreground=allow_foreground,
            max_steps=4,
            visual_targets=(CANVAS_TARGET,),
            visual_min_confidence=CANVAS_MIN_CONFIDENCE,
            mock_preferences=(f"visual:{slug(CANVAS_TARGET)}",),
        )
    if kind == "counter":
        return NativeTask(
            id=task_id,
            goal=(
                f"The counter starts at 0. Set it to exactly {COUNTER_TARGET} by pressing "
                "Increment once per step, then stop."
            ),
            scope=scope,
            allowed_actions=frozenset({"press"}),
            oracle=oracle,
            check=_counter_check,
            allow_foreground=allow_foreground,
            max_steps=COUNTER_TARGET + 3,
            mock_preferences=("ax:button:increment",),
            steps=(
                TaskStep('Press the button labeled "Increment"', "ax:button:increment", COUNTER_TARGET),
            ),
        )
    if kind == "save-note":
        return NativeTask(
            id=task_id,
            goal="Enter the note text into the Note field, then save the note.",
            scope=scope,
            allowed_actions=frozenset({"press", "set_text"}),
            oracle=oracle,
            check=_note_check(note_text),
            parameters=(TaskParameter("note", note_text, secret=True, redaction="[note text]"),),
            allow_foreground=allow_foreground,
            max_steps=5,
            mock_preferences=("ax:text_input:note:set:note", "ax:button:save-note"),
            steps=(
                TaskStep(
                    'Set the text field "Note" to the task parameter "note"',
                    "ax:text_input:note:set:note",
                ),
                TaskStep('Press the button labeled "Save note"', "ax:button:save-note"),
            ),
        )
    return NativeTask(
        id=task_id,
        goal="Choose the Large size option and check the I agree checkbox.",
        scope=scope,
        allowed_actions=frozenset({"select", "toggle"}),
        oracle=oracle,
        check=_size_check,
        allow_foreground=allow_foreground,
        max_steps=5,
        mock_preferences=("ax:radio:large", "ax:checkbox:i-agree"),
        steps=(
            TaskStep('Select the radio option "Large"', "ax:radio:large"),
            # The oracle accepts either order, so neither step waits for the other.
            TaskStep('Toggle the checkbox "I agree"', "ax:checkbox:i-agree", after_previous=False),
        ),
    )


def appkit_task(
    task_id: str,
    state_path: Path,
    *,
    pid: int | None = None,
    note_text: str = DEFAULT_NOTE_TEXT,
    allow_foreground: bool = False,
) -> NativeTask:
    """Build one AppKit harness task (kept for Phase 1 callers)."""
    if task_id not in APPKIT_TASK_IDS:
        raise ValueError(f"unknown AppKit task: {task_id}")
    return native_task(
        task_id, state_path, pid=pid, note_text=note_text, allow_foreground=allow_foreground
    )
