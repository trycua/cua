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
        "anthropic.claude-sonnet-5-20260929-v1:0",
        "us.anthropic.claude-opus-5-20260805-v1:0",
        "bedrock/anthropic.claude-opus-5-v1:0",
        "claude-opus-5@20260805",
        "vertex_ai/claude-sonnet-5@20260929",
    ],
)
def test_claude_5_uses_20251124_computer_tool(model: str) -> None:
    assert _get_tool_config_for_model(model) == COMPUTER_20251124


@pytest.mark.parametrize(
    "model",
    [
        "claude-sonnet-5-5",
        "claude-opus-5.5",
        "anthropic/claude-sonnet-5-5",
        "claude-opus-5-5@20261001",
        "vertex_ai/claude-sonnet-5-5@20261001",
        "vertex_ai/claude-haiku-5-5@20261001",
    ],
)
def test_api_and_vertex_claude_5_5_do_not_use_20251124_computer_tool(model: str) -> None:
    assert _get_tool_config_for_model(model) != COMPUTER_20251124


@pytest.mark.parametrize(
    "model",
    [
        "anthropic.claude-sonnet-5-5-20261001-v1:0",
        "us.anthropic.claude-opus-5-5-20261001-v1:0",
        "bedrock/anthropic.claude-opus-5-5-v1:0",
        "anthropic.claude-haiku-5-5-v1:0",
    ],
)
def test_bedrock_claude_5_5_uses_20251124_computer_tool(model: str) -> None:
    assert _get_tool_config_for_model(model) == COMPUTER_20251124


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
