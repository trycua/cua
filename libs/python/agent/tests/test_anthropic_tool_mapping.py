import pytest

from cua_agent.loops.anthropic import _get_tool_config_for_model

COMPUTER_20251124 = {
    "tool_version": "computer_20251124",
    "beta_flag": "computer-use-2025-11-24",
}


@pytest.mark.parametrize(
    "model",
    [
        "claude-sonnet-5",
        "anthropic/claude-sonnet-5",
        "claude-sonnet-5-20260929",
        "claude-opus-5",
        "anthropic/claude-opus-5-20260805",
    ],
)
def test_claude_5_uses_20251124_computer_tool(model: str) -> None:
    assert _get_tool_config_for_model(model) == COMPUTER_20251124


@pytest.mark.parametrize("model", ["claude-sonnet-5-5", "claude-opus-5.5"])
def test_claude_5_5_does_not_match_claude_5_mapping(model: str) -> None:
    assert _get_tool_config_for_model(model) == {
        "tool_version": "computer_20241022",
        "beta_flag": "computer-use-2024-10-22",
    }


@pytest.mark.parametrize(
    ("model", "expected"),
    [
        ("claude-opus-4-6", COMPUTER_20251124),
        (
            "claude-sonnet-4",
            {
                "tool_version": "computer_20250124",
                "beta_flag": "computer-use-2025-01-24",
            },
        ),
        (
            "unknown-model",
            {
                "tool_version": "computer_20241022",
                "beta_flag": "computer-use-2024-10-22",
            },
        ),
    ],
)
def test_existing_model_mapping_behavior_is_unchanged(model: str, expected: dict[str, str]) -> None:
    assert _get_tool_config_for_model(model) == expected
