"""Task specs: what one jev-use run is trying to achieve and how it is checked.

A task spec owns everything that is specific to one application task: the goal
shown to the model, its parameters (secrets are redacted from all model input),
the step budget, the Driver action kinds it may use, the compact state summary
the model sees, the candidate IDs and descriptions it offers, and the success
oracle that verifies the outcome independently of the runner's own events.

Candidate sources (``sources.py``) supply the controls and build the executable
candidates. The runner stays task-agnostic: it observes, asks the task for
candidates and a state summary, lets the model choose one supplied ID, acts,
and asks the task's oracle whether the task is done.

``FixtureFormTask`` is the one built-in task: type a secret verification token
into the loopback fixture's form and submit it, verified through the fixture's
``/state`` endpoint.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Literal, Mapping, Protocol
from urllib.request import Request, urlopen

from sources import BrowserSemanticSource, Candidate, NativeAccessibilitySource, VisualRegionSource

if TYPE_CHECKING:
    from core import VisualDelivery, VisualObservation, VisualRegion

Outcome = Literal["verified", "refuted", "unknown", "abstained", "budget_exhausted"]

REDACTED_TOKEN = "[verification token]"


def redact_token(value: Any, token: str, placeholder: str = REDACTED_TOKEN) -> Any:
    """Replace every occurrence of the token in strings nested in ``value``."""
    if not token:
        return value
    if isinstance(value, str):
        return value.replace(token, placeholder)
    if isinstance(value, Mapping):
        return {key: redact_token(item, token, placeholder) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [redact_token(item, token, placeholder) for item in value]
    return value


@dataclass(frozen=True)
class TaskParameter:
    """A named task input. A ``secret`` value is replaced by ``redaction``
    everywhere in model input; it may still appear in executable arguments."""

    name: str
    value: str
    secret: bool = False
    redaction: str = REDACTED_TOKEN


@dataclass(frozen=True)
class TaskSources:
    """The candidate sources available for one decision step.

    ``visual`` is present only when this step parsed visual regions.
    ``visual_path`` reports whether a capture-bound visual path exists at all,
    so a task can say a control is still pending a visual check.

    ``page`` is present for browser tasks. ``ax`` is present for native tasks
    (RFC #4268); ``foreground_ids`` names the native candidate IDs for which
    Driver refused background delivery on an earlier step.
    """

    page: BrowserSemanticSource | None = None
    visual: VisualRegionSource | None = None
    visual_path: bool = False
    ax: NativeAccessibilitySource | None = None
    foreground_ids: frozenset[str] = frozenset()

    def require_page(self) -> BrowserSemanticSource:
        if self.page is None:
            raise ValueError("this task needs a browser page source")
        return self.page


class Task(Protocol):
    """The interface the runner uses for every task."""

    id: str
    goal: str
    parameters: tuple[TaskParameter, ...]
    max_steps: int
    allowed_action_kinds: frozenset[str]
    # Candidate IDs after which the runner polls the oracle for completion.
    completion_candidate_ids: frozenset[str]

    def redact(self, value: Any) -> Any: ...

    def candidates(self, sources: TaskSources) -> list[Candidate]: ...

    def state_summary(self, sources: TaskSources) -> dict[str, str]: ...

    def history_entry(
        self, step: int, candidate_id: str, *, refusal: str | None = None
    ) -> dict[str, Any]: ...

    def reset(self) -> None: ...

    def read_oracle(self) -> Mapping[str, Any]: ...

    def classify(self, oracle: Mapping[str, Any], *, steps: int) -> Outcome: ...


# The built-in browser fixture task.

FIXTURE_TASK_ID = "browser-fixture-form"
FIXTURE_GOAL = (
    "Enter the required verification token into the verification field, then submit the form."
)
DEFAULT_FIXTURE_URL = "http://127.0.0.1:8765/"
DEFAULT_MAX_STEPS = 4
FIELD_NAME = "verification value"
SUBMIT_NAME = "Submit"
SUBMIT_IDS = frozenset({"submit-form", "submit-form-foreground"})
FIXTURE_ACTION_KINDS = frozenset({"browser_type", "browser_click", "click"})

HISTORY_OUTCOMES = {
    "type-verification-value": "typed the required token into the verification field",
    "submit-form": "clicked Submit; the submission was not yet confirmed",
    "submit-form-foreground": "clicked Submit in the foreground; the submission was not yet confirmed",
    "reobserve": "took no action and requested a fresh observation",
}


def fixture_state(fixture_url: str) -> dict[str, str | None]:
    with urlopen(f"{fixture_url.rstrip('/')}/state", timeout=2) as response:
        return json.loads(response.read())


def reset_fixture(fixture_url: str) -> None:
    request = Request(f"{fixture_url.rstrip('/')}/reset", method="POST", data=b"")
    with urlopen(request, timeout=2) as response:
        if response.status != 204:
            raise RuntimeError(f"fixture reset failed: HTTP {response.status}")


def classify(submitted: str | None, token: str, *, steps: int, max_steps: int) -> Outcome:
    if submitted == token:
        return "verified"
    if submitted is not None:
        return "refuted"
    if steps >= max_steps:
        return "budget_exhausted"
    return "unknown"


def history_entry(
    step: int, candidate_id: str, *, refusal: str | None = None
) -> dict[str, Any]:
    """Build the compact decision-history item shown to the model.

    It records what each step did, not timings or model probabilities, so earlier
    choices do not become a signal to repeat themselves.
    """
    if refusal is not None:
        outcome = (
            f"Driver refused background delivery ({refusal}); no click happened and a "
            "foreground Submit candidate is offered next"
        )
    else:
        outcome = HISTORY_OUTCOMES.get(candidate_id, "completed")
    return {"step": step, "selected_id": candidate_id, "outcome": outcome}


def _reserved_candidates() -> list[Candidate]:
    return [
        Candidate(
            "reobserve",
            "Take no action and obtain a fresh Driver observation, because the current "
            "observation is stale or contradicts the reported form state. A Submit control "
            "that is visual-only, or whose visual check is still pending, is not a reason to "
            "reobserve: the runner parses visual regions for Submit once no page-structure "
            "action remains.",
            None,
            {},
        ),
        Candidate(
            "abstain",
            "Stop without acting if none of the proposed actions is safe for the observed state.",
            None,
            {},
        ),
    ]


@dataclass(frozen=True)
class FixtureFormTask:
    """Type the secret token into the fixture form and submit it.

    The oracle is the fixture's ``/state`` endpoint: the task is verified only
    when the fixture recorded exactly this token as submitted.
    """

    token: str
    fixture_url: str = DEFAULT_FIXTURE_URL
    max_steps: int = DEFAULT_MAX_STEPS
    id: str = field(default=FIXTURE_TASK_ID, init=False)
    goal: str = field(default=FIXTURE_GOAL, init=False)
    allowed_action_kinds: frozenset[str] = field(default=FIXTURE_ACTION_KINDS, init=False)
    completion_candidate_ids: frozenset[str] = field(default=SUBMIT_IDS, init=False)

    @property
    def parameters(self) -> tuple[TaskParameter, ...]:
        return (TaskParameter("verification_token", self.token, secret=True),)

    def redact(self, value: Any) -> Any:
        for parameter in self.parameters:
            if parameter.secret:
                value = redact_token(value, parameter.value, parameter.redaction)
        return value

    def candidates(self, sources: TaskSources) -> list[Candidate]:
        """Build the closed candidate set for one decision.

        Page-structure refs always win. The capture-bound visual Submit is offered
        only when no Submit ref exists. A visual source with ``foreground``
        delivery yields a distinct ``submit-form-foreground`` candidate after
        Driver refused background delivery; the chooser must pick it explicitly.
        """
        page = sources.require_page()
        # Every page candidate addresses the snapshot's target; reject a snapshot
        # without one before offering anything.
        page.require_target()
        token = self.token
        field_control = page.find("textbox", FIELD_NAME)
        button = page.find("button", SUBMIT_NAME)
        candidates: list[Candidate] = []
        if field_control and field_control.value != token:
            candidates.append(
                page.type_text(
                    field_control,
                    token,
                    candidate_id="type-verification-value",
                    description="Type the required verification token into the verification "
                    "field, replacing its current contents.",
                )
            )
        elif field_control and field_control.value == token and button:
            candidates.append(
                page.click(
                    button,
                    candidate_id="submit-form",
                    description="Click the form's Submit button. The observed form state "
                    "reports that the verification field already contains the required "
                    "token, so the form is ready to submit.",
                )
            )
        elif field_control and field_control.value == token and sources.visual is not None:
            visual = sources.visual
            region = visual.find("button", SUBMIT_NAME)
            if region is not None:
                foreground = visual.delivery == "foreground"
                candidate = visual.click(
                    region,
                    candidate_id="submit-form-foreground" if foreground else "submit-form",
                    description=(
                        "Submit the form by clicking the unique validated visual Submit "
                        "region with foreground delivery, which activates the browser "
                        "window, because Driver refused background delivery for the "
                        "previous visual click."
                        if foreground
                        else "Submit the form by clicking the unique validated visual Submit "
                        "region. The observed form state reports that the verification field "
                        "already contains the required token."
                    ),
                )
                if candidate is not None:
                    candidates.append(candidate)
        for candidate in candidates:
            if candidate.tool not in self.allowed_action_kinds:
                raise ValueError(f"task {self.id} does not allow action kind {candidate.tool}")
        return candidates + _reserved_candidates()

    def state_summary(self, sources: TaskSources) -> dict[str, str]:
        """Summarize the form for the decision model without revealing the token.

        The raw field value never leaves the runner; the model receives only whether
        the field is empty, holds the required token, or holds something else.

        ``submit_button`` is ``available`` for a clickable page-structure ref. When
        the page structure has none and the capture-bound visual path is enabled
        (``visual_path``), it is ``visual_only`` if this observation holds a unique
        validated visual Submit region, ``visual_check_pending`` if no visual regions
        were parsed for this step yet (the runner parses them once no page-structure
        action remains), and ``not_found_visually`` otherwise. Without a visual path
        it is ``not_in_page_structure``.
        """
        page = sources.require_page()
        field_control = page.find("textbox", FIELD_NAME)
        button = page.find("button", SUBMIT_NAME)
        if field_control is None:
            field_state = "not_found"
        elif not field_control.value:
            field_state = "empty"
        elif field_control.value == self.token:
            field_state = "contains_required_token"
        else:
            field_state = "contains_other_value"
        if button is not None:
            submit_state = "available"
        elif not sources.visual_path:
            submit_state = "not_in_page_structure"
        elif sources.visual is None:
            submit_state = "visual_check_pending"
        elif sources.visual.find("button", SUBMIT_NAME) is not None:
            submit_state = "visual_only"
        else:
            submit_state = "not_found_visually"
        return {"verification_field": field_state, "submit_button": submit_state}

    def history_entry(
        self, step: int, candidate_id: str, *, refusal: str | None = None
    ) -> dict[str, Any]:
        return history_entry(step, candidate_id, refusal=refusal)

    def reset(self) -> None:
        reset_fixture(self.fixture_url)

    def read_oracle(self) -> Mapping[str, Any]:
        return fixture_state(self.fixture_url)

    def classify(self, oracle: Mapping[str, Any], *, steps: int) -> Outcome:
        return classify(oracle.get("submitted"), self.token, steps=steps, max_steps=self.max_steps)


def fixture_sources(
    snapshot: Mapping[str, Any],
    visual: VisualObservation | None = None,
    *,
    capture_bound_click: bool = False,
    visual_delivery: VisualDelivery = "background",
    visual_path: bool = False,
) -> TaskSources:
    """Build one step's sources from a browser snapshot and optional visual parse."""
    return TaskSources(
        BrowserSemanticSource(snapshot),
        None
        if visual is None
        else VisualRegionSource(visual, visual_delivery, capture_bound_click),
        visual_path,
    )


# Compatibility entry points. They keep the pre-task-spec signatures and
# delegate to the built-in fixture task, so existing callers are unchanged.


def visual_submit_region(visual: VisualObservation | None) -> VisualRegion | None:
    """Return the unique validated visual Submit region, or ``None``."""
    if visual is None:
        return None
    control = VisualRegionSource(visual).find("button", SUBMIT_NAME)
    return control.handle if control is not None else None


def form_state(
    snapshot: Mapping[str, Any],
    token: str,
    visual: VisualObservation | None = None,
    *,
    visual_path: bool = False,
) -> dict[str, str]:
    """Summarize the fixture form; see ``FixtureFormTask.state_summary``."""
    return FixtureFormTask(token).state_summary(
        fixture_sources(snapshot, visual, visual_path=visual_path)
    )


def build_candidates(
    snapshot: Mapping[str, Any],
    token: str,
    visual: VisualObservation | None = None,
    *,
    capture_bound_click: bool = False,
    visual_delivery: VisualDelivery = "background",
) -> list[Candidate]:
    """Build the fixture candidate set; see ``FixtureFormTask.candidates``."""
    return FixtureFormTask(token).candidates(
        fixture_sources(
            snapshot,
            visual,
            capture_bound_click=capture_bound_click,
            visual_delivery=visual_delivery,
        )
    )
