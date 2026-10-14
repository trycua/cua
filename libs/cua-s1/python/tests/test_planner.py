from __future__ import annotations

from dataclasses import replace

import pytest

from cua_s1.driver import BaseDriver, MutationResult, WindowSnapshot, WindowTarget
from cua_s1.planner import Planner, PlannerError, order_decisions, run_form
from cua_s1.schema import Decision, Element, Entity

TARGET = WindowTarget(42, 7)
ENTITIES = [Entity("Email", "user@example.invalid")]


def _elements(generation: int = 1):
    return [
        Element("Edit", "Email", index=1, element_token=f"email-{generation}"),
        Element(
            "CheckBox",
            "I agree",
            index=2,
            checked=generation >= 3,
            element_token=f"agree-{generation}",
        ),
        Element("Button", "Submit", index=3, element_token=f"submit-{generation}"),
        Element("Button", "Cancel", index=4, element_token=f"cancel-{generation}"),
        Element("StaticText", "Heading", index=5, element_token=f"heading-{generation}"),
    ]


def _snapshot(generation: int):
    return WindowSnapshot(
        target=TARGET,
        elements=_elements(generation),
        snapshot_id=f"snapshot-{generation}",
        tree_markdown="",
        elements_complete=True,
        raw={},
    )


class RecordingDriver(BaseDriver):
    def __init__(self):
        super().__init__(session="test")
        self.generation = 1
        self.mutations = []

    def window_state(self, target):
        assert target == TARGET
        return _snapshot(self.generation)

    def set_value(self, target, element_token, value):
        self.mutations.append(("fill", element_token, value))
        self.generation += 1
        return MutationResult(
            {"effect": "confirmed", "route": "accessibility"}, _snapshot(self.generation)
        )

    def click(self, target, element_token, *, delivery_mode):
        self.mutations.append(("click", element_token, delivery_mode))
        self.generation += 1
        return MutationResult(
            {"effect": "confirmed", "route": "accessibility"}, _snapshot(self.generation)
        )


def backend(_title, elements, _entities):
    actions = {
        "Email": ("fill", 0, 0.99),
        "I agree": ("check", None, 0.95),
        "Submit": ("click", None, 0.90),
        "Cancel": ("click", None, 0.98),
    }
    return [Decision(element, *actions[element.label]) for element in elements]


def test_run_form_is_dry_run_by_default_and_submit_stays_gated():
    driver = RecordingDriver()

    report = run_form(Planner(backend), driver, ENTITIES, target=TARGET, form_title="Example form")

    assert driver.mutations == []
    assert [entry["action"] for entry in report["execution_order"]] == ["fill", "check"]
    assert all(entry["status"] == "planned" for entry in report["execution_order"])
    assert report["authorization"] == {
        "execute": False,
        "submit": False,
        "delivery_mode": "background",
        "note": None,
    }


def test_execution_reobserves_before_each_mutation_and_requires_submit_opt_in():
    driver = RecordingDriver()

    report = run_form(
        Planner(backend),
        driver,
        ENTITIES,
        target=TARGET,
        form_title="Example form",
        execute=True,
        submit=True,
    )

    assert driver.mutations == [
        ("fill", "email-1", "user@example.invalid"),
        ("click", "agree-2", "background"),
        ("click", "submit-3", "background"),
    ]
    assert [entry["post_snapshot_id"] for entry in report["execution_order"]] == [
        "snapshot-2",
        "snapshot-3",
        "snapshot-4",
    ]
    assert report["final_snapshot"]["snapshot_id"] == "snapshot-4"


def test_order_decisions_keeps_one_submit_and_applies_confidence_threshold():
    email, checkbox, submit, cancel, _ = _elements()
    decisions = [
        Decision(submit, "click", None, 0.8),
        Decision(email, "fill", 0, 0.9),
        Decision(cancel, "click", None, 0.7),
        Decision(checkbox, "check", None, 0.4),
    ]

    ordered = order_decisions(decisions, 0.5, allow_submit=True)

    assert [(decision.action, decision.element.label) for decision in ordered] == [
        ("fill", "Email"),
        ("click", "Submit"),
    ]


@pytest.mark.parametrize(
    ("role", "label"),
    [
        ("Button", "Cancel"),
        ("Button", "Delete"),
        ("Button", "Continue"),
        ("Edit", "Submit"),
    ],
)
def test_submit_authorization_never_executes_non_submit_clicks(role, label):
    element = Element(role, label, index=1, element_token="target-1")

    class SingleElementDriver(RecordingDriver):
        def window_state(self, target):
            return WindowSnapshot(target, [element], "snapshot-1", "", True, {})

    planner = Planner(
        lambda _title, elements, _entities: [Decision(elements[0], "click", None, 1.0)]
    )
    driver = SingleElementDriver()

    report = run_form(
        planner,
        driver,
        [],
        target=TARGET,
        form_title="Form",
        execute=True,
        submit=True,
    )

    assert driver.mutations == []
    assert report["execution_order"] == []


def test_planner_rejects_foreign_elements_and_invalid_entity_pointers():
    element = Element("Edit", "Email", index=1)
    foreign = replace(element)

    with pytest.raises(PlannerError) as caught:
        Planner(lambda *_args: [Decision(foreign, "fill", 0, 0.9)]).plan(
            "form", [element], ENTITIES
        )
    assert caught.value.code == "foreign_element"

    with pytest.raises(PlannerError) as caught:
        Planner(lambda *_args: [Decision(element, "fill", 9, 0.9)]).plan(
            "form", [element], ENTITIES
        )
    assert caught.value.code == "invalid_entity_index"


def test_planner_rejects_duplicate_decisions_even_when_count_matches():
    first = Element("Edit", "First", index=1, element_token="first")
    second = Element("Edit", "Second", index=2, element_token="second")

    with pytest.raises(PlannerError) as caught:
        Planner(
            lambda *_args: [
                Decision(first, "skip", None, 1.0),
                Decision(first, "skip", None, 1.0),
            ]
        ).plan("form", [first, second], [])

    assert caught.value.code == "duplicate_decision"
    assert caught.value.details["element_index"] == 1


def test_planner_rejects_omitted_element_decisions():
    first = Element("Edit", "First", index=1, element_token="first")
    second = Element("Edit", "Second", index=2, element_token="second")

    with pytest.raises(PlannerError) as caught:
        Planner(lambda *_args: [Decision(first, "skip", None, 1.0)]).plan(
            "form", [first, second], []
        )

    assert caught.value.code == "missing_decisions"
    assert caught.value.details["missing_element_indices"] == [2]


def test_planner_matches_recreated_decisions_by_stable_element_token():
    supplied = Element("Button", "Submit", index=1, element_token="submit-token")
    recreated = replace(supplied, label="untrusted replacement")

    decisions = Planner(lambda *_args: [Decision(recreated, "click", None, 1.0)]).plan(
        "form", [supplied], []
    )

    assert decisions[0].element is supplied


def test_check_is_idempotent_when_checkbox_is_already_checked():
    driver = RecordingDriver()
    driver.generation = 3

    report = run_form(
        Planner(backend),
        driver,
        ENTITIES,
        target=TARGET,
        form_title="Example form",
        execute=True,
    )

    assert driver.mutations == [("fill", "email-3", "user@example.invalid")]
    check = next(entry for entry in report["execution_order"] if entry["action"] == "check")
    assert check["status"] == "already_satisfied"
    assert check["executed"] is False


@pytest.mark.parametrize(
    ("element", "code"),
    [
        (
            Element("Button", "I agree", index=2, checked=False, element_token="token"),
            "check_requires_checkbox",
        ),
        (
            Element("CheckBox", "I agree", index=2, checked=None, element_token="token"),
            "checkbox_state_unknown",
        ),
    ],
)
def test_check_refuses_non_checkbox_or_unknown_state(element, code):
    class RefusingDriver(RecordingDriver):
        def window_state(self, target):
            return WindowSnapshot(target, [element], "snapshot", "", True, {})

    planner = Planner(
        lambda _title, elements, _entities: [Decision(elements[0], "check", None, 1.0)]
    )
    report = run_form(planner, RefusingDriver(), [], target=TARGET, form_title="Form", execute=True)

    assert report["execution_order"][0]["error"]["code"] == code
    assert report["stopped_after_error"] is True


def test_execution_refuses_incomplete_or_unbound_snapshots():
    class IncompleteDriver(RecordingDriver):
        def __init__(self, snapshot_id, complete):
            super().__init__()
            self.snapshot_id = snapshot_id
            self.complete = complete

        def window_state(self, target):
            return WindowSnapshot(target, _elements(), self.snapshot_id, "", self.complete, {})

    for snapshot_id, complete, code in [
        (None, True, "snapshot_id_required"),
        ("snapshot", False, "incomplete_window_snapshot"),
    ]:
        report = run_form(
            Planner(backend),
            IncompleteDriver(snapshot_id, complete),
            ENTITIES,
            target=TARGET,
            form_title="Form",
            execute=True,
        )
        assert report["execution_order"][0]["error"]["code"] == code


def test_check_verifies_checked_postcondition():
    class FailedCheckboxDriver(RecordingDriver):
        def click(self, target, element_token, *, delivery_mode):
            self.mutations.append(("click", element_token, delivery_mode))
            observation = WindowSnapshot(
                TARGET,
                [Element("CheckBox", "I agree", index=2, checked=False, element_token="next")],
                "snapshot-next",
                "",
                True,
                {},
            )
            return MutationResult({"effect": "confirmed", "route": "accessibility"}, observation)

        def window_state(self, target):
            return WindowSnapshot(
                TARGET,
                [Element("CheckBox", "I agree", index=2, checked=False, element_token="token")],
                "snapshot",
                "",
                True,
                {},
            )

    planner = Planner(
        lambda _title, elements, _entities: [Decision(elements[0], "check", None, 1.0)]
    )
    report = run_form(
        planner, FailedCheckboxDriver(), [], target=TARGET, form_title="Form", execute=True
    )

    assert report["execution_order"][0]["error"]["code"] == "checkbox_postcondition_failed"


def test_execution_refuses_legacy_action_response_without_effect():
    class LegacyDriver(RecordingDriver):
        def set_value(self, target, element_token, value):
            return MutationResult({}, _snapshot(2))

    report = run_form(
        Planner(backend),
        LegacyDriver(),
        ENTITIES,
        target=TARGET,
        form_title="Form",
        execute=True,
    )

    error = report["execution_order"][0]["error"]
    assert error["code"] == "action_effect_unconfirmed"
    assert error["outcome_unknown"] is True
