from __future__ import annotations

import json
from pathlib import Path

import pytest

from cua_s1.driver import BaseDriver, DriverError, WindowSnapshot, WindowTarget
from cua_s1.schema import Element


class StubDriver(BaseDriver):
    def __init__(self, windows):
        super().__init__(session="test")
        self.windows = windows

    def call(self, tool, **args):
        assert tool == "list_windows"
        return {"windows": self.windows}


class MutationDriver(BaseDriver):
    def __init__(self, action):
        super().__init__(session="test")
        self.action = action

    def call(self, tool, **args):
        if tool == "click":
            return self.action
        if tool == "get_window_state":
            return {
                "snapshot_id": "snapshot-2",
                "elements_complete": True,
                "elements": [],
            }
        raise AssertionError(tool)


def test_find_window_rejects_ambiguous_and_missing_targets():
    driver = StubDriver(
        [
            {"pid": 10, "window_id": 20, "title": "Application form", "is_on_screen": True},
            {"pid": 10, "window_id": 21, "title": "Application form", "is_on_screen": True},
        ]
    )

    with pytest.raises(DriverError) as ambiguous:
        driver.find_window(title_contains="application")
    assert ambiguous.value.code == "ambiguous_window"
    assert len(ambiguous.value.details["matches"]) == 2

    with pytest.raises(DriverError) as missing:
        driver.find_window(window_id=999)
    assert missing.value.code == "window_not_found"

    assert driver.find_window(pid=10, window_id=21)["window_id"] == 21


def test_snapshot_requires_a_unique_fresh_element_token():
    planned = Element("Edit", "Email", index=4)
    target = WindowTarget(10, 20)
    snapshot = WindowSnapshot(
        target=target,
        elements=[Element("Edit", "Email", index=4, element_token="snapshot-token")],
        snapshot_id="snapshot-1",
        tree_markdown="",
        elements_complete=True,
        raw={},
    )
    assert snapshot.token_for(planned) == "snapshot-token"

    duplicate = WindowSnapshot(
        target=target,
        elements=[
            Element("Edit", "Email", index=4, element_token="one"),
            Element("Edit", "Email", index=4, element_token="two"),
        ],
        snapshot_id="snapshot-2",
        tree_markdown="",
        elements_complete=True,
        raw={},
    )
    with pytest.raises(DriverError) as caught:
        duplicate.token_for(planned)
    assert caught.value.code == "element_not_uniquely_resolved"


@pytest.mark.parametrize("action", [{}, {"effect": "unverifiable", "route": "accessibility"}])
def test_mutation_requires_a_confirmed_driver_effect(action):
    driver = MutationDriver(action)

    with pytest.raises(DriverError) as caught:
        driver.click(WindowTarget(10, 20), "token", delivery_mode="background")

    assert caught.value.code == "action_effect_unconfirmed"
    assert caught.value.outcome_unknown is True
    assert caught.value.details["post_action_observation"]["snapshot_id"] == "snapshot-2"


def test_set_value_is_optional_and_fails_closed_without_discovery():
    driver = MutationDriver({"effect": "confirmed", "route": "accessibility"})

    assert driver.supports_value_mutation() is False
    with pytest.raises(DriverError) as caught:
        driver.set_value(WindowTarget(10, 20), "token", "value")
    assert caught.value.code == "unsupported_driver_capability"


def test_click_payload_and_effect_expectations_conform_to_portable_manifest():
    manifest_path = Path(__file__).parents[3] / "cua-driver" / "contract" / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    click = next(tool for tool in manifest["tools"] if tool["name"] == "click")

    assert {"target", "delivery_mode"} <= set(click["input_schema"]["required"])
    effect = click["success_output_schema"]["properties"]["effect"]
    assert "effect" in click["success_output_schema"]["required"]
    assert set(effect["enum"]) == {
        "confirmed",
        "partial",
        "unverifiable",
        "suspected_noop",
        "refused",
    }
    assert all(tool["name"] != "set_value" for tool in manifest["tools"])
