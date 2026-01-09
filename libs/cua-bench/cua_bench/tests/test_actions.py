import pytest
from cua_bench.actions import repr_to_action
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


class TestReprToAction:
    """Test suite for repr_to_action function with all action types."""

    def test_click_action(self):
        """Test ClickAction parsing."""
        action = ClickAction(x=100, y=200)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, ClickAction)
        assert parsed.x == 100
        assert parsed.y == 200

    def test_right_click_action(self):
        """Test RightClickAction parsing."""
        action = RightClickAction(x=150, y=250)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, RightClickAction)
        assert parsed.x == 150
        assert parsed.y == 250

    def test_double_click_action(self):
        """Test DoubleClickAction parsing."""
        action = DoubleClickAction(x=75, y=125)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, DoubleClickAction)
        assert parsed.x == 75
        assert parsed.y == 125

    def test_middle_click_action(self):
        """Test MiddleClickAction parsing."""
        action = MiddleClickAction(x=300, y=400)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, MiddleClickAction)
        assert parsed.x == 300
        assert parsed.y == 400

    def test_drag_action_with_duration(self):
        """Test DragAction parsing with custom duration."""
        action = DragAction(from_x=10, from_y=20, to_x=30, to_y=40, duration=2.5)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, DragAction)
        assert parsed.from_x == 10
        assert parsed.from_y == 20
        assert parsed.to_x == 30
        assert parsed.to_y == 40
        assert parsed.duration == 2.5

    def test_drag_action_default_duration(self):
        """Test DragAction parsing with default duration."""
        action = DragAction(from_x=50, from_y=60, to_x=70, to_y=80)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, DragAction)
        assert parsed.from_x == 50
        assert parsed.from_y == 60
        assert parsed.to_x == 70
        assert parsed.to_y == 80
        assert parsed.duration == 1.0  # default value

    def test_move_to_action_with_duration(self):
        """Test MoveToAction parsing with custom duration."""
        action = MoveToAction(x=200, y=300, duration=1.5)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, MoveToAction)
        assert parsed.x == 200
        assert parsed.y == 300
        assert parsed.duration == 1.5

    def test_move_to_action_default_duration(self):
        """Test MoveToAction parsing with default duration."""
        action = MoveToAction(x=100, y=150)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, MoveToAction)
        assert parsed.x == 100
        assert parsed.y == 150
        assert parsed.duration == 0.0  # default value

    def test_scroll_action_up(self):
        """Test ScrollAction parsing with up direction."""
        action = ScrollAction(direction="up", amount=150)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, ScrollAction)
        assert parsed.direction == "up"
        assert parsed.amount == 150

    def test_scroll_action_down(self):
        """Test ScrollAction parsing with down direction."""
        action = ScrollAction(direction="down", amount=200)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, ScrollAction)
        assert parsed.direction == "down"
        assert parsed.amount == 200

    def test_scroll_action_defaults(self):
        """Test ScrollAction parsing with default values."""
        action = ScrollAction()
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, ScrollAction)
        assert parsed.direction == "up"  # default value
        assert parsed.amount == 100  # default value

    def test_type_action(self):
        """Test TypeAction parsing."""
        action = TypeAction(text="Hello, World!")
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, TypeAction)
        assert parsed.text == "Hello, World!"

    def test_type_action_empty_text(self):
        """Test TypeAction parsing with empty text."""
        action = TypeAction(text="")
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, TypeAction)
        assert parsed.text == ""

    def test_type_action_special_chars(self):
        """Test TypeAction parsing with special characters."""
        action = TypeAction(text="Test with symbols: @#$%")
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, TypeAction)
        assert parsed.text == "Test with symbols: @#$%"

    def test_key_action(self):
        """Test KeyAction parsing."""
        action = KeyAction(key="Enter")
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, KeyAction)
        assert parsed.key == "Enter"

    def test_key_action_special_keys(self):
        """Test KeyAction parsing with special keys."""
        special_keys = ["Escape", "Tab", "Space", "F1", "Ctrl+C", "Alt+Tab"]

        for key in special_keys:
            action = KeyAction(key=key)
            action_repr = repr(action)
            parsed = repr_to_action(action_repr)

            assert isinstance(parsed, KeyAction)
            assert parsed.key == key

    def test_hotkey_action(self):
        """Test HotkeyAction parsing."""
        action = HotkeyAction(keys=["ctrl", "c"])
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, HotkeyAction)
        assert parsed.keys == ["ctrl", "c"]

    def test_hotkey_action_multiple_keys(self):
        """Test HotkeyAction parsing with multiple keys."""
        action = HotkeyAction(keys=["ctrl", "shift", "z"])
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, HotkeyAction)
        assert parsed.keys == ["ctrl", "shift", "z"]

    def test_hotkey_action_single_key(self):
        """Test HotkeyAction parsing with single key."""
        action = HotkeyAction(keys=["f5"])
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, HotkeyAction)
        assert parsed.keys == ["f5"]

    def test_done_action(self):
        """Test DoneAction parsing."""
        action = DoneAction()
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, DoneAction)

    def test_wait_action_with_seconds(self):
        """Test WaitAction parsing with custom seconds."""
        action = WaitAction(seconds=3.5)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, WaitAction)
        assert parsed.seconds == 3.5

    def test_wait_action_default_seconds(self):
        """Test WaitAction parsing with default seconds."""
        action = WaitAction()
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, WaitAction)
        assert parsed.seconds == 1.0  # default value

    def test_wait_action_integer_seconds(self):
        """Test WaitAction parsing with integer seconds."""
        action = WaitAction(seconds=5)
        action_repr = repr(action)
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, WaitAction)
        assert parsed.seconds == 5

    # Edge cases and error handling

    def test_invalid_input_none(self):
        """Test with None input."""
        with pytest.raises(ValueError, match="action_repr must be a string"):
            repr_to_action(None)

    def test_invalid_input_non_string(self):
        """Test with non-string input."""
        with pytest.raises(ValueError, match="action_repr must be a string"):
            repr_to_action(123)

    def test_invalid_input_empty_string(self):
        """Test with empty string."""
        with pytest.raises(ValueError, match="Unknown action representation"):
            repr_to_action("")

    def test_invalid_input_malformed_string(self):
        """Test with malformed action string."""
        with pytest.raises(ValueError, match="Unknown action representation"):
            repr_to_action("NotAnAction")

    def test_invalid_input_unknown_action(self):
        """Test with unknown action type."""
        with pytest.raises(ValueError, match="Unknown action representation"):
            repr_to_action("UnknownAction(x=100)")

    def test_invalid_input_malformed_args(self):
        """Test with malformed arguments."""
        with pytest.raises(ValueError, match="Unknown action representation"):
            repr_to_action("ClickAction(invalid_args)")

    def test_invalid_input_missing_required_args(self):
        """Test with missing required arguments."""
        with pytest.raises(ValueError, match="Unknown action representation"):
            repr_to_action("ClickAction(x=100)")  # missing y

    def test_whitespace_handling(self):
        """Test that whitespace is properly handled."""
        action = ClickAction(x=50, y=75)
        action_repr = f"  {repr(action)}  "  # add whitespace
        parsed = repr_to_action(action_repr)

        assert isinstance(parsed, ClickAction)
        assert parsed.x == 50
        assert parsed.y == 75


class TestReprRoundTrip:
    """Test that repr -> parse -> repr produces consistent results."""

    def test_all_actions_round_trip(self):
        """Test round-trip conversion for all action types."""
        actions = [
            ClickAction(x=100, y=200),
            RightClickAction(x=150, y=250),
            DoubleClickAction(x=75, y=125),
            MiddleClickAction(x=300, y=400),
            DragAction(from_x=10, from_y=20, to_x=30, to_y=40, duration=2.5),
            MoveToAction(x=200, y=300, duration=1.5),
            ScrollAction(direction="down", amount=150),
            TypeAction(text="Test string"),
            KeyAction(key="Enter"),
            HotkeyAction(keys=["ctrl", "c"]),
            DoneAction(),
            WaitAction(seconds=2.0),
        ]

        for original_action in actions:
            # Convert to string representation
            action_repr = repr(original_action)

            # Parse back to action
            parsed_action = repr_to_action(action_repr)

            # Verify it's the same type and has same attributes
            assert type(parsed_action) == type(original_action)
            assert repr(parsed_action) == repr(original_action)


if __name__ == "__main__":
    pytest.main([__file__])
