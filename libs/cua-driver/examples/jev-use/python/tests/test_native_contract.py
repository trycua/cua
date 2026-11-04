"""Role table and cua.jev_choice_request_v2 contract tests (RFC #4268)."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from choose_action import provider_observation, validate_request
from choose_decision import MockDecisionModel
from decision_models import (
    DecisionRequest,
    S1DecisionModel,
    TypeSafeDecisionModel,
    choose,
    native_elements_as_text,
)
from native_roles import (
    PLATFORMS,
    RAW_ROLES,
    ROLE_CLASS_ACTION,
    ROLE_CLASSES,
    ROLE_TABLES,
    normalized_role,
    role_class,
)

ROOT = Path(__file__).resolve().parents[2]


def v2_request() -> dict:
    return json.loads((ROOT / "fixtures/jev-choice-request-v2.json").read_text(encoding="utf-8"))


def v1_request() -> dict:
    return json.loads((ROOT / "fixtures/jev-choice-request-v1.json").read_text(encoding="utf-8"))


class RoleTableTest(unittest.TestCase):
    def test_normalized_role_matches_driver_cases(self) -> None:
        # The cases Driver's own expectation.rs normalizer documents.
        cases = {
            "AXCheckBox": "checkbox",
            "CheckBox": "checkbox",
            "check box": "checkbox",
            "push button": "button",
            "AXButton": "button",
            "page tab": "tab",
            "TabItem": "tab",
            "AXTextField": "textfield",
            "combo box": "combobox",
        }
        for raw, expected in cases.items():
            with self.subTest(raw=raw):
                self.assertEqual(normalized_role(raw), expected)

    def test_each_platform_maps_its_raw_roles(self) -> None:
        expectations = {
            "macos": {
                "AXButton": "button",
                "AXCheckBox": "checkbox",
                "AXRadioButton": "radio",
                "AXPopUpButton": "popup",
                "AXComboBox": "popup",
                "AXMenuItem": "menu_item",
                "AXMenuBarItem": "menu_item",
                "AXLink": "link",
                "AXTextField": "text_input",
                "AXTextArea": "text_input",
                "AXSecureTextField": "text_input",
                "AXSwitch": "toggle",
            },
            "windows": {
                "Button": "button",
                "SplitButton": "button",
                "CheckBox": "checkbox",
                "RadioButton": "radio",
                "ComboBox": "popup",
                "MenuItem": "menu_item",
                "Hyperlink": "link",
                "Edit": "text_input",
            },
            "linux": {
                "push button": "button",
                "toggle button": "toggle",
                "check box": "checkbox",
                "radio button": "radio",
                "combo box": "popup",
                "menu item": "menu_item",
                "check menu item": "menu_item",
                "link": "link",
                "entry": "text_input",
                "text": "text_input",
                "password text": "text_input",
            },
        }
        for platform, cases in expectations.items():
            for raw, expected in cases.items():
                with self.subTest(platform=platform, raw=raw):
                    self.assertEqual(role_class(raw, platform), expected)

    def test_unknown_and_platform_specific_roles_are_excluded(self) -> None:
        excluded = {
            "macos": ["AXStaticText", "AXSlider", "AXWindow", "AXMenuBar", "AXMenu", "AXGroup", ""],
            # UIA Text is static text on Windows, unlike AT-SPI text on Linux.
            "windows": ["Text", "Slider", "TabItem", "Pane", "Window", "Unknown"],
            "linux": ["label", "slider", "page tab", "frame", "filler"],
        }
        for platform, roles in excluded.items():
            for raw in roles:
                with self.subTest(platform=platform, raw=raw):
                    self.assertIsNone(role_class(raw, platform))
        self.assertIsNone(role_class(None, "macos"))  # type: ignore[arg-type]

    def test_tables_are_keyed_by_driver_normalization(self) -> None:
        # Raw roles that Driver's verify_state treats as one role (equal
        # normalized_role) always share one role class on every platform.
        for platform in PLATFORMS:
            for klass, raw_roles in RAW_ROLES[platform].items():
                self.assertIn(klass, ROLE_CLASSES)
                for raw in raw_roles:
                    self.assertEqual(ROLE_TABLES[platform][normalized_role(raw)], klass)
        # The same control spelled per platform lands in the same class.
        for names, klass in (
            (("AXCheckBox", "CheckBox", "check box"), "checkbox"),
            (("AXButton", "Button", "push button"), "button"),
            (("AXRadioButton", "RadioButton", "radio button"), "radio"),
            (("AXLink", "Hyperlink", "link"), "link"),
            (("AXTextField", "Edit", "entry"), "text_input"),
            (("AXPopUpButton", "ComboBox", "combo box"), "popup"),
            (("AXMenuItem", "MenuItem", "menu item"), "menu_item"),
        ):
            for platform, raw in zip(PLATFORMS, names):
                self.assertEqual(role_class(raw, platform), klass, (platform, raw))

    def test_every_role_class_has_one_action(self) -> None:
        self.assertEqual(set(ROLE_CLASS_ACTION), set(ROLE_CLASSES))


class V2ContractTest(unittest.TestCase):
    def test_v2_request_validates_with_sources_and_elements(self) -> None:
        validated = validate_request(v2_request())
        self.assertEqual(validated["schema"], "cua.jev_choice_request_v2")
        self.assertEqual(validated["snapshot_id"], "s0000002a")
        self.assertEqual(len(validated["elements"]), 3)
        self.assertEqual(validated["candidates"][0]["source"], "ax")
        self.assertNotIn("source", validated["candidates"][1])

    def test_v2_without_optional_fields_is_accepted(self) -> None:
        request = v2_request()
        del request["snapshot_id"], request["elements"]
        for candidate in request["candidates"]:
            candidate.pop("source", None)
        validated = validate_request(request)
        self.assertIsNone(validated["snapshot_id"])
        self.assertEqual(validated["elements"], [])

    def test_v1_rejects_every_v2_field(self) -> None:
        for mutate in (
            lambda r: r.__setitem__("snapshot_id", "s1"),
            lambda r: r.__setitem__("elements", []),
            lambda r: r["candidates"][0].__setitem__("source", "visual"),
        ):
            request = v1_request()
            mutate(request)
            with self.assertRaises(ValueError):
                validate_request(request)

    def test_v2_rejects_malformed_fields(self) -> None:
        mutations = {
            "unknown root key": lambda r: r.__setitem__("state", {}),
            "unknown source": lambda r: r["candidates"][0].__setitem__("source", "dom"),
            "reserved with source": lambda r: r["candidates"][1].__setitem__("source", "ax"),
            "candidate token": lambda r: r["candidates"][0].__setitem__("element_token", "s1:3"),
            "element value": lambda r: r["elements"][0].__setitem__("value", "secret"),
            "unknown role class": lambda r: r["elements"][0].__setitem__("role_class", "slider"),
            "unknown state": lambda r: r["elements"][0].__setitem__("state", "hidden"),
            "too many elements": lambda r: r.__setitem__("elements", r["elements"] * 22),
            "missing required": lambda r: r.pop("regions"),
            "unknown schema": lambda r: r.__setitem__("schema", "cua.jev_choice_request_v3"),
        }
        for name, mutate in mutations.items():
            request = copy.deepcopy(v2_request())
            mutate(request)
            with self.subTest(name=name), self.assertRaises(ValueError):
                validate_request(request)

    def test_provider_observation_is_unchanged_for_v1(self) -> None:
        observation = provider_observation(validate_request(v1_request()))
        self.assertEqual(set(observation), {"capture_id", "regions", "history"})
        v2 = provider_observation(validate_request(v2_request()))
        self.assertEqual(v2["candidate_sources"], {"ax:button:increment": "ax"})
        self.assertEqual(v2["snapshot_id"], "s0000002a")
        self.assertNotIn("element_token", json.dumps(v2))

    def test_mock_cli_answers_v2_with_the_unchanged_response(self) -> None:
        script = ROOT / "python/choose_action.py"
        result = subprocess.run(
            [sys.executable, str(script), "--mock"],
            input=json.dumps(v2_request()),
            text=True,
            capture_output=True,
            check=True,
        )
        response = json.loads(result.stdout)
        self.assertEqual(response["schema"], "cua.jev_choice_v1")
        self.assertEqual(response["selected_id"], "ax:button:increment")

    def test_decision_cli_answers_v2_with_decision_choice_v1(self) -> None:
        script = ROOT / "python/choose_decision.py"
        result = subprocess.run(
            [sys.executable, str(script), "--model", "mock"],
            input=json.dumps(v2_request()),
            text=True,
            capture_output=True,
            check=True,
        )
        response = json.loads(result.stdout)
        self.assertEqual(response["schema"], "cua.decision_choice_v1")
        self.assertEqual(response["kind"], "selected")
        self.assertEqual(response["selected_id"], "ax:button:increment")

    def test_jev_adapter_receives_native_observation(self) -> None:
        sent: dict = {}

        class Answer:
            choice = "ax:button:increment"
            confidence = 0.9
            probabilities = {"ax:button:increment": 0.9, "reobserve": 0.05, "abstain": 0.05}

        class Client:
            def system_one(self, **request):
                sent.update(request)
                return type("Response", (), {"choices": {"candidate": Answer()}, "model": "jev"})()

        request = DecisionRequest.from_validated(validate_request(v2_request()))
        result = choose(TypeSafeDecisionModel(Client()), request)
        self.assertEqual(result.selected_id, "ax:button:increment")
        observation = sent["state"]["observation"]
        self.assertEqual(observation["elements"][0]["label"], "Increment")
        self.assertEqual(observation["candidate_sources"], {"ax:button:increment": "ax"})

    def test_s1_text_adapter_renders_an_accessibility_tree(self) -> None:
        request = DecisionRequest.from_validated(validate_request(v2_request()))
        text = native_elements_as_text(request)
        self.assertIn('- button "Increment" (enabled)', text)
        self.assertIn('- text_input "Note" (empty)', text)
        self.assertIn("Prior bounded decisions:", text)

        seen: dict = {}

        class Scorer:
            modality = "text"

            def forward(self, options, **kwargs):
                seen.update(kwargs)
                return [
                    type("Score", (), {"element_id": option.element_id, "probability": p})()
                    for option, p in zip(options, (0.8, 0.1, 0.1))
                ]

        result = choose(S1DecisionModel(Scorer()), request)
        self.assertEqual(result.selected_id, "ax:button:increment")
        self.assertIn("Accessibility tree for snapshot", seen["ax_tree"])

    def test_mock_decision_model_is_unchanged_for_v2(self) -> None:
        request = DecisionRequest.from_validated(validate_request(v2_request()))
        result = choose(MockDecisionModel(), request)
        self.assertEqual(result.selected_id, "ax:button:increment")


if __name__ == "__main__":
    unittest.main()
