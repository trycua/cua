from __future__ import annotations

import asyncio
import importlib.util
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest

MODULE_PATH = Path(__file__).parents[1] / "src" / "cua_driver" / "testing.py"
SPEC = importlib.util.spec_from_file_location("cua_driver_testing", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
testing = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = testing
SPEC.loader.exec_module(testing)


@dataclass
class FakeImage:
    mime_type: str = "image/png"
    data_base64: str = "iVBORw0KGgo="


@dataclass
class FakeResult:
    text: str = "ok"
    structured_json: str | None = None
    is_error: bool = False
    error_code: str | None = None
    images: list[FakeImage] = field(default_factory=list)


class FakeDriver:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict[str, Any]]] = []
        self.active_pid = 41
        self.target_pid = 99
        self.window_id = 700
        self.window_x = 100.0
        self.window_y = 100.0
        self.target_on_screen = False
        self.target_z_index = 1
        self.counter = 0
        self.value = ""
        self.steal_on: str | None = None
        self.raise_on: str | None = None
        self.fail_on: str | None = None
        self.hide_active = False
        self.shutdown_called = False
        self.initial_placement_verified = True

    async def call_tool(self, name: str, arguments_json: str) -> FakeResult:
        arguments = json.loads(arguments_json)
        self.calls.append((name, arguments))
        if name == self.fail_on:
            return FakeResult(
                text="fixture refusal",
                is_error=True,
                error_code="fixture_refusal",
            )
        if name == "get_frontmost_app":
            return self._result(
                {} if self.hide_active else {"pid": self.active_pid, "name": "Foreground"}
            )
        if name == "list_apps":
            return self._result(
                {
                    "apps": (
                        []
                        if self.hide_active
                        else [
                            {
                                "pid": self.active_pid,
                                "name": "Foreground",
                                "active": True,
                            }
                        ]
                    )
                }
            )
        if name == "launch_app":
            initial_position = arguments.get("initial_window_position")
            initial_placement = None
            if isinstance(initial_position, dict):
                before = self._bounds()
                self.window_x = float(initial_position["x"])
                self.window_y = float(initial_position["y"])
                initial_placement = {
                    "window_id": self.window_id,
                    "requested_position": initial_position,
                    "before_bounds": before,
                    "after_bounds": self._bounds(),
                    "verified": self.initial_placement_verified,
                    "activated": False,
                }
            return self._result(
                {
                    "pid": self.target_pid,
                    "bundle_id": "com.example.Counter",
                    "name": "Counter",
                    "self_activation_suppressed": True,
                    "windows": [self._window()],
                    "initial_window_placement": initial_placement,
                }
            )
        if name == "list_windows":
            pid = arguments.get("pid")
            windows = [self._window()]
            if pid is None and self.active_pid != self.target_pid:
                windows.append(self._active_window())
            elif pid == self.active_pid and self.active_pid != self.target_pid:
                windows = [self._active_window()]
            return self._result({"windows": windows})
        if name == "get_screen_size":
            return self._result(
                {
                    "width": 1920,
                    "height": 1080,
                    "scale_factor": 2,
                    "displays": [
                        {
                            "display_id": 1,
                            "bounds": {
                                "x": 0,
                                "y": 0,
                                "width": 1920,
                                "height": 1080,
                            },
                            "scale_factor": 2,
                            "is_main": True,
                            "is_builtin": False,
                            "is_mirrored": False,
                        },
                        {
                            "display_id": 2,
                            "bounds": {
                                "x": 1920,
                                "y": 0,
                                "width": 1280,
                                "height": 1024,
                            },
                            "scale_factor": 2,
                            "is_main": False,
                            "is_builtin": True,
                            "is_mirrored": False,
                        },
                    ],
                }
            )
        if name == "move_window":
            before = self._bounds()
            self.window_x = float(arguments["x"])
            self.window_y = float(arguments["y"])
            return self._result(
                {
                    "before_bounds": before,
                    "after_bounds": self._bounds(),
                    "verified": True,
                    "activated": False,
                }
            )
        if name == "get_window_state":
            return self._snapshot(arguments)
        if name == "click":
            self.counter += 1
        if name in {"type_text", "set_value"}:
            self.value = arguments.get("text", arguments.get("value", ""))
        if name == self.steal_on:
            self.active_pid = self.target_pid
        if name == self.raise_on:
            self.target_on_screen = True
            self.target_z_index = 3
        return self._result({"verified": True})

    async def shutdown(self) -> None:
        self.shutdown_called = True

    def _window(self) -> dict[str, Any]:
        return {
            "pid": self.target_pid,
            "window_id": self.window_id,
            "title": "Counter",
            "app_name": "Counter",
            "is_on_screen": self.target_on_screen,
            "bounds": self._bounds(),
            "z_index": self.target_z_index,
        }

    def _active_window(self) -> dict[str, Any]:
        return {
            "pid": self.active_pid,
            "window_id": 701,
            "title": "Foreground",
            "app_name": "Foreground",
            "is_on_screen": True,
            "bounds": {
                "x": 50,
                "y": 50,
                "width": 1200,
                "height": 900,
            },
            "z_index": 2,
        }

    def _bounds(self) -> dict[str, float]:
        return {
            "x": self.window_x,
            "y": self.window_y,
            "width": 700,
            "height": 860,
        }

    def _snapshot(self, arguments: dict[str, Any]) -> FakeResult:
        tree = (
            '- [0] AXWindow "Counter" [id=counter-window]\n'
            '  - [1] AXButton "Increment" [id=btn-increment actions=[press]]\n'
            f'  - AXStaticText "counter={self.counter}"\n'
            '  - [2] AXTextField "Input" = '
            f'"{self.value}" [id=txt-input actions=[setvalue]]\n'
        )
        data = {
            "pid": self.target_pid,
            "window_id": self.window_id,
            "tree_markdown": tree,
            "elements": [
                {
                    "element_index": 0,
                    "element_token": "snapshot:0",
                    "role": "AXWindow",
                    "label": "Counter",
                    "identifier": "counter-window",
                },
                {
                    "element_index": 1,
                    "element_token": "snapshot:1",
                    "role": "AXButton",
                    "label": "Increment",
                    # Omit the structured identifier to exercise the markdown
                    # fallback used with older Cua Driver releases.
                },
                {
                    "element_index": 2,
                    "element_token": "snapshot:2",
                    "role": "AXTextField",
                    "label": "Input",
                    "value": self.value,
                },
            ],
        }
        images = []
        if arguments.get("include_screenshot"):
            images = [FakeImage()]
        return self._result(data, images)

    @staticmethod
    def _result(
        structured: dict[str, Any],
        images: list[FakeImage] | None = None,
    ) -> FakeResult:
        return FakeResult(
            structured_json=json.dumps(structured),
            images=images or [],
        )


def run(coroutine: Any) -> Any:
    return asyncio.run(coroutine)


def test_launch_tap_and_assert_text_without_foreground_delivery(tmp_path: Path) -> None:
    driver = FakeDriver()

    async def scenario() -> None:
        async with testing.CuaTestSession(
            driver,
            artifacts_dir=tmp_path,
            owns_driver=True,
        ) as session:
            app = await session.launch(bundle_id="com.example.Counter")
            assert app.window.is_on_screen is False
            assert app.window.frame is not None
            assert app.window.frame.x >= 1920
            assert await app.buttons.by_id("btn-increment").exists()
            await app.buttons["Increment"].tap()
            await app.wait_for_text("counter=1")

    run(scenario())

    launch = next(arguments for name, arguments in driver.calls if name == "launch_app")
    click = next(arguments for name, arguments in driver.calls if name == "click")
    moved = next(arguments for name, arguments in driver.calls if name == "move_window")
    assert launch["creates_new_application_instance"] is True
    assert launch["initial_window_position"]["x"] >= 1920
    assert "title" not in launch["initial_window_position"]
    assert moved["x"] >= 1920
    assert driver.calls.index(("move_window", moved)) < driver.calls.index(("click", click))
    assert click["pid"] == driver.target_pid
    assert click["window_id"] == driver.window_id
    assert click["element_token"] == "snapshot:1"
    assert click["delivery_mode"] == "background"
    assert "x" not in click and "y" not in click
    assert not any(name == "bring_to_front" for name, _ in driver.calls)
    assert any(name == "kill_app" for name, _ in driver.calls)
    assert driver.shutdown_called


def test_text_input_is_element_scoped_and_background_only(tmp_path: Path) -> None:
    driver = FakeDriver()

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        field = app.text_fields.by_id("txt-input")
        await field.type_text("hello")
        await field.wait_for_value("hello")
        await session.close()

    run(scenario())
    typed = next(arguments for name, arguments in driver.calls if name == "type_text")
    assert typed["text"] == "hello"
    assert typed["element_token"] == "snapshot:2"
    assert typed["delivery_mode"] == "background"


def test_target_activation_fails_the_background_contract(tmp_path: Path) -> None:
    driver = FakeDriver()
    driver.steal_on = "click"

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        with pytest.raises(testing.CuaForegroundViolation, match="activated target pid 99"):
            await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())
    assert list(tmp_path.glob("*-foreground-violation/snapshot.json"))
    assert list(tmp_path.glob("*-foreground-violation/tree.txt"))
    assert list(tmp_path.glob("*-foreground-violation/window.png"))


def test_window_raise_fails_the_background_contract(tmp_path: Path) -> None:
    driver = FakeDriver()
    driver.target_on_screen = True
    driver.raise_on = "click"

    async def scenario() -> None:
        session = testing.CuaTestSession(
            driver,
            artifacts_dir=tmp_path,
            placement="unchanged",
        )
        app = await session.launch(bundle_id="com.example.Counter")
        with pytest.raises(
            testing.CuaWindowOrderViolation,
            match="raised target window 700 above",
        ):
            await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())
    assert list(tmp_path.glob("*-foreground-violation/snapshot.json"))


def test_window_raise_on_secondary_display_also_fails(tmp_path: Path) -> None:
    driver = FakeDriver()
    driver.target_on_screen = True
    driver.raise_on = "click"

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        assert app.window.frame is not None
        assert app.window.frame.x >= 1920
        with pytest.raises(
            testing.CuaWindowOrderViolation,
            match="above the active application's visible windows",
        ):
            await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())


def test_tool_failure_writes_failure_artifacts(tmp_path: Path) -> None:
    driver = FakeDriver()
    driver.fail_on = "click"

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        with pytest.raises(testing.CuaToolError, match="fixture_refusal"):
            await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())
    assert list(tmp_path.glob("*-click/snapshot.json"))


def test_ambiguous_label_requires_a_stable_identifier(tmp_path: Path) -> None:
    driver = FakeDriver()
    original_snapshot = driver._snapshot

    def duplicate_snapshot(arguments: dict[str, Any]) -> FakeResult:
        result = original_snapshot(arguments)
        data = json.loads(result.structured_json or "{}")
        duplicate = dict(data["elements"][1])
        duplicate["element_index"] = 3
        duplicate["element_token"] = "snapshot:3"
        data["elements"].append(duplicate)
        return FakeResult(structured_json=json.dumps(data))

    driver._snapshot = duplicate_snapshot  # type: ignore[method-assign]

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        with pytest.raises(testing.CuaQueryError, match="matched 2 elements"):
            await app.buttons["Increment"].tap()
        await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())


def test_subscript_prefers_identifier_over_the_same_label(tmp_path: Path) -> None:
    driver = FakeDriver()
    original_snapshot = driver._snapshot

    def conflicting_snapshot(arguments: dict[str, Any]) -> FakeResult:
        result = original_snapshot(arguments)
        data = json.loads(result.structured_json or "{}")
        data["elements"].append(
            {
                "element_index": 3,
                "element_token": "snapshot:3",
                "role": "AXButton",
                "label": "Different label",
                "identifier": "Increment",
            }
        )
        return FakeResult(structured_json=json.dumps(data))

    driver._snapshot = conflicting_snapshot  # type: ignore[method-assign]

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        await app.buttons["Increment"].tap()
        await session.close()

    run(scenario())
    click = next(arguments for name, arguments in driver.calls if name == "click")
    assert click["element_index"] == 3
    assert click["element_token"] == "snapshot:3"


def test_attach_does_not_kill_a_process_it_does_not_own(tmp_path: Path) -> None:
    driver = FakeDriver()

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.attach(driver.target_pid, window_id=driver.window_id)
        assert await app.buttons.by_id("btn-increment").exists()
        await session.close()

    run(scenario())
    assert not any(name == "kill_app" for name, _ in driver.calls)


def test_placement_can_be_explicitly_left_unchanged(tmp_path: Path) -> None:
    driver = FakeDriver()

    async def scenario() -> None:
        session = testing.CuaTestSession(
            driver,
            artifacts_dir=tmp_path,
            placement="unchanged",
        )
        app = await session.attach(driver.target_pid, window_id=driver.window_id)
        assert app.window.frame == testing.Frame(100, 100, 700, 860)
        await session.close()

    run(scenario())
    assert not any(name == "move_window" for name, _ in driver.calls)


def test_unverified_early_placement_fails_and_cleans_up(tmp_path: Path) -> None:
    driver = FakeDriver()
    driver.initial_placement_verified = False

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        with pytest.raises(testing.CuaTestError, match="did not verify early placement"):
            await session.launch(bundle_id="com.example.Counter")
        await session.close()

    run(scenario())
    assert any(name == "kill_app" for name, _ in driver.calls)
    assert not any(name == "move_window" for name, _ in driver.calls)


def test_action_fails_closed_when_frontmost_app_is_unknown(tmp_path: Path) -> None:
    driver = FakeDriver()

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        driver.hide_active = True
        with pytest.raises(testing.CuaTestError, match="background safety cannot be verified"):
            await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())
    assert not any(name == "click" for name, _ in driver.calls)
    assert not any(name == "list_apps" for name, _ in driver.calls)


def test_action_refuses_a_target_that_is_already_foreground(tmp_path: Path) -> None:
    driver = FakeDriver()

    async def scenario() -> None:
        session = testing.CuaTestSession(driver, artifacts_dir=tmp_path)
        app = await session.launch(bundle_id="com.example.Counter")
        driver.active_pid = driver.target_pid
        with pytest.raises(testing.CuaForegroundViolation, match="already foreground"):
            await app.buttons.by_id("btn-increment").tap()
        await session.close()

    run(scenario())
    assert not any(name == "click" for name, _ in driver.calls)
    assert list(tmp_path.glob("*-target-already-foreground/snapshot.json"))


def test_kill_failure_still_shuts_down_the_owned_driver(tmp_path: Path) -> None:
    driver = FakeDriver()
    driver.fail_on = "kill_app"

    async def scenario() -> None:
        session = testing.CuaTestSession(
            driver,
            artifacts_dir=tmp_path,
            owns_driver=True,
        )
        await session.launch(bundle_id="com.example.Counter")
        with pytest.raises(testing.CuaToolError, match="fixture_refusal"):
            await session.close()

    run(scenario())
    assert driver.shutdown_called
