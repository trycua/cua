"""Tests for the worker server HTTP endpoints.

This module tests:
- Action serialization/deserialization (unit tests)
- FastAPI server endpoints using TestClient with real simulated environments (e2e)
"""

import pytest
from cua_bench.types import (
    ClickAction,
    DoneAction,
    DoubleClickAction,
    DragAction,
    HotkeyAction,
    KeyAction,
    MiddleClickAction,
    MoveToAction,
    RightClickAction,
    ScrollAction,
    TypeAction,
    WaitAction,
)
from cua_bench.workers.worker_server import (
    ResetRequest,
    ShutdownRequest,
    StepRequest,
    app,
    deserialize_action,
    serialize_action,
)
from fastapi.testclient import TestClient

# Simple HTML for test tasks
SIMPLE_BUTTON_HTML = """
<div class="flex flex-col items-center justify-center h-full w-full bg-gray-100 p-4">
    <h1 class="text-xl mb-4">Test Task</h1>
    <button
        id="test-btn"
        class="btn bg-blue-500 text-white px-4 py-2 rounded"
        onclick="window.__clicked = true; window.__score = 1.0;"
    >
        Click Me
    </button>
</div>
<script>
    window.__clicked = false;
    window.__score = 0.0;
</script>
"""


class TestDeserializeAction:
    """Tests for deserialize_action function."""

    def test_click_action_type_format(self):
        """Test deserializing ClickAction with type format."""
        action_dict = {"type": "ClickAction", "x": 100, "y": 200}
        action = deserialize_action(action_dict)
        assert isinstance(action, ClickAction)
        assert action.x == 100
        assert action.y == 200

    def test_click_action_lowercase_format(self):
        """Test deserializing ClickAction with lowercase format."""
        action_dict = {"action_type": "click", "x": 100, "y": 200}
        action = deserialize_action(action_dict)
        assert isinstance(action, ClickAction)
        assert action.x == 100
        assert action.y == 200

    def test_right_click_action(self):
        """Test deserializing RightClickAction."""
        action_dict = {"type": "RightClickAction", "x": 50, "y": 75}
        action = deserialize_action(action_dict)
        assert isinstance(action, RightClickAction)
        assert action.x == 50
        assert action.y == 75

    def test_double_click_action(self):
        """Test deserializing DoubleClickAction."""
        action_dict = {"type": "DoubleClickAction", "x": 300, "y": 400}
        action = deserialize_action(action_dict)
        assert isinstance(action, DoubleClickAction)
        assert action.x == 300
        assert action.y == 400

    def test_middle_click_action(self):
        """Test deserializing MiddleClickAction."""
        action_dict = {"type": "MiddleClickAction", "x": 150, "y": 250}
        action = deserialize_action(action_dict)
        assert isinstance(action, MiddleClickAction)
        assert action.x == 150
        assert action.y == 250

    def test_drag_action(self):
        """Test deserializing DragAction."""
        action_dict = {
            "type": "DragAction",
            "from_x": 100,
            "from_y": 100,
            "to_x": 200,
            "to_y": 200,
            "duration": 0.5,
        }
        action = deserialize_action(action_dict)
        assert isinstance(action, DragAction)
        assert action.from_x == 100
        assert action.from_y == 100
        assert action.to_x == 200
        assert action.to_y == 200
        assert action.duration == 0.5

    def test_drag_action_default_duration(self):
        """Test deserializing DragAction with default duration."""
        action_dict = {
            "type": "DragAction",
            "from_x": 100,
            "from_y": 100,
            "to_x": 200,
            "to_y": 200,
        }
        action = deserialize_action(action_dict)
        assert action.duration == 1.0  # Default

    def test_move_to_action(self):
        """Test deserializing MoveToAction."""
        action_dict = {"type": "MoveToAction", "x": 500, "y": 600, "duration": 0.2}
        action = deserialize_action(action_dict)
        assert isinstance(action, MoveToAction)
        assert action.x == 500
        assert action.y == 600
        assert action.duration == 0.2

    def test_scroll_action(self):
        """Test deserializing ScrollAction."""
        action_dict = {"type": "ScrollAction", "direction": "down", "amount": 50}
        action = deserialize_action(action_dict)
        assert isinstance(action, ScrollAction)
        assert action.direction == "down"
        assert action.amount == 50

    def test_scroll_action_defaults(self):
        """Test deserializing ScrollAction with defaults."""
        action_dict = {"type": "ScrollAction"}
        action = deserialize_action(action_dict)
        assert action.direction == "up"  # Default
        assert action.amount == 100  # Default

    def test_type_action(self):
        """Test deserializing TypeAction."""
        action_dict = {"type": "TypeAction", "text": "Hello World"}
        action = deserialize_action(action_dict)
        assert isinstance(action, TypeAction)
        assert action.text == "Hello World"

    def test_key_action(self):
        """Test deserializing KeyAction."""
        action_dict = {"type": "KeyAction", "key": "Enter"}
        action = deserialize_action(action_dict)
        assert isinstance(action, KeyAction)
        assert action.key == "Enter"

    def test_hotkey_action(self):
        """Test deserializing HotkeyAction."""
        action_dict = {"type": "HotkeyAction", "keys": ["ctrl", "c"]}
        action = deserialize_action(action_dict)
        assert isinstance(action, HotkeyAction)
        assert action.keys == ["ctrl", "c"]

    def test_wait_action(self):
        """Test deserializing WaitAction."""
        action_dict = {"type": "WaitAction", "seconds": 2.5}
        action = deserialize_action(action_dict)
        assert isinstance(action, WaitAction)
        assert action.seconds == 2.5

    def test_wait_action_default(self):
        """Test deserializing WaitAction with default seconds."""
        action_dict = {"type": "WaitAction"}
        action = deserialize_action(action_dict)
        assert action.seconds == 1.0

    def test_done_action(self):
        """Test deserializing DoneAction."""
        action_dict = {"type": "DoneAction"}
        action = deserialize_action(action_dict)
        assert isinstance(action, DoneAction)

    def test_unknown_action_raises(self):
        """Test that unknown action type raises ValueError."""
        action_dict = {"type": "UnknownAction"}
        with pytest.raises(ValueError, match="Unknown action type"):
            deserialize_action(action_dict)


class TestSerializeAction:
    """Tests for serialize_action function."""

    def test_serialize_click_action(self):
        """Test serializing ClickAction."""
        action = ClickAction(x=100, y=200)
        result = serialize_action(action)
        assert result["type"] == "ClickAction"
        assert result["x"] == 100
        assert result["y"] == 200

    def test_serialize_right_click_action(self):
        """Test serializing RightClickAction."""
        action = RightClickAction(x=50, y=75)
        result = serialize_action(action)
        assert result["type"] == "RightClickAction"
        assert result["x"] == 50
        assert result["y"] == 75

    def test_serialize_drag_action(self):
        """Test serializing DragAction."""
        action = DragAction(from_x=100, from_y=100, to_x=200, to_y=200, duration=0.5)
        result = serialize_action(action)
        assert result["type"] == "DragAction"
        assert result["from_x"] == 100
        assert result["from_y"] == 100
        assert result["to_x"] == 200
        assert result["to_y"] == 200
        assert result["duration"] == 0.5

    def test_serialize_type_action(self):
        """Test serializing TypeAction."""
        action = TypeAction(text="Hello")
        result = serialize_action(action)
        assert result["type"] == "TypeAction"
        assert result["text"] == "Hello"

    def test_serialize_done_action(self):
        """Test serializing DoneAction."""
        action = DoneAction()
        result = serialize_action(action)
        assert result["type"] == "DoneAction"


class TestRequestModels:
    """Tests for Pydantic request models."""

    def test_reset_request_defaults(self):
        """Test ResetRequest with default values."""
        request = ResetRequest(env_path="./task")
        assert request.env_path == "./task"
        assert request.task_index == 0
        assert request.split == "train"
        assert request.timeout == 300

    def test_reset_request_custom(self):
        """Test ResetRequest with custom values."""
        request = ResetRequest(
            env_path="./custom-task",
            task_index=5,
            split="test",
            timeout=600,
        )
        assert request.env_path == "./custom-task"
        assert request.task_index == 5
        assert request.split == "test"
        assert request.timeout == 600

    def test_step_request(self):
        """Test StepRequest."""
        request = StepRequest(
            action={"type": "ClickAction", "x": 100, "y": 200},
            env_id=0,
        )
        assert request.action["type"] == "ClickAction"
        assert request.env_id == 0

    def test_shutdown_request_all(self):
        """Test ShutdownRequest for all envs."""
        request = ShutdownRequest()
        assert request.env_id is None

    def test_shutdown_request_specific(self):
        """Test ShutdownRequest for specific env."""
        request = ShutdownRequest(env_id=1)
        assert request.env_id == 1


class TestActionRoundTrip:
    """Tests for serialize/deserialize round-trip."""

    def test_click_roundtrip(self):
        """Test ClickAction serialize/deserialize round-trip."""
        original = ClickAction(x=123, y=456)
        serialized = serialize_action(original)
        restored = deserialize_action(serialized)
        assert restored.x == original.x
        assert restored.y == original.y

    def test_drag_roundtrip(self):
        """Test DragAction serialize/deserialize round-trip."""
        original = DragAction(from_x=10, from_y=20, to_x=30, to_y=40, duration=1.5)
        serialized = serialize_action(original)
        restored = deserialize_action(serialized)
        assert restored.from_x == original.from_x
        assert restored.from_y == original.from_y
        assert restored.to_x == original.to_x
        assert restored.to_y == original.to_y
        assert restored.duration == original.duration

    def test_type_roundtrip(self):
        """Test TypeAction serialize/deserialize round-trip."""
        original = TypeAction(text="Hello World!")
        serialized = serialize_action(original)
        restored = deserialize_action(serialized)
        assert restored.text == original.text

    def test_hotkey_roundtrip(self):
        """Test HotkeyAction serialize/deserialize round-trip."""
        original = HotkeyAction(keys=["ctrl", "shift", "n"])
        serialized = serialize_action(original)
        restored = deserialize_action(serialized)
        assert restored.keys == original.keys


class TestServerEndpoints:
    """Tests for FastAPI server endpoints using TestClient.

    Note: These tests only cover endpoints that don't require Playwright.
    Full e2e tests with real environments are in test_worker_manager.py.
    """

    def test_health_endpoint(self):
        """Test GET /health returns ok status."""
        client = TestClient(app)
        response = client.get("/health")
        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "ok"
        assert "available_envs" in data
        assert "active_envs" in data
        assert "max_envs" in data

    def test_shutdown_endpoint(self):
        """Test POST /shutdown releases environments."""
        client = TestClient(app)
        response = client.post("/shutdown", json={})
        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "ok"

    def test_step_without_env_returns_error(self):
        """Test POST /step without valid env_id returns 404."""
        client = TestClient(app)
        response = client.post(
            "/step",
            json={
                "action": {"type": "ClickAction", "x": 100, "y": 200},
                "env_id": 999,
            },
        )
        assert response.status_code == 404

    def test_screenshot_without_env_returns_error(self):
        """Test GET /screenshot without valid env_id returns 404."""
        client = TestClient(app)
        response = client.get("/screenshot", params={"env_id": 999})
        assert response.status_code == 404
