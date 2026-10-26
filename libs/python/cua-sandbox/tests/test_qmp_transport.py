from __future__ import annotations

from typing import Any

from cua_sandbox.transport.qmp import QMPTransport


class _RecordingQMPTransport(QMPTransport):
    def __init__(self) -> None:
        super().__init__()
        self.commands: list[tuple[str, dict[str, Any] | None]] = []

    async def _execute(
        self, command: str, arguments: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        self.commands.append((command, arguments))
        return {"return": {}}


async def test_type_text_maps_shifted_punctuation_to_base_key_qcodes():
    shifted_punctuation = '!@#$%^&*()_+{}|:"<>?~'
    expected_qcodes = [
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "0",
        "minus",
        "equal",
        "bracket_left",
        "bracket_right",
        "backslash",
        "semicolon",
        "apostrophe",
        "comma",
        "dot",
        "slash",
        "grave_accent",
    ]
    transport = _RecordingQMPTransport()

    await transport._type_text(shifted_punctuation)

    assert transport.commands == [
        (
            "send-key",
            {
                "keys": [
                    {"type": "qcode", "data": "shift"},
                    {"type": "qcode", "data": qcode},
                ],
                "hold-time": 50,
            },
        )
        for qcode in expected_qcodes
    ]


async def test_type_text_preserves_unshifted_and_uppercase_key_events():
    transport = _RecordingQMPTransport()

    await transport._type_text("aA1-")

    assert transport.commands == [
        (
            "send-key",
            {"keys": [{"type": "qcode", "data": "a"}], "hold-time": 50},
        ),
        (
            "send-key",
            {
                "keys": [
                    {"type": "qcode", "data": "shift"},
                    {"type": "qcode", "data": "a"},
                ],
                "hold-time": 50,
            },
        ),
        (
            "send-key",
            {"keys": [{"type": "qcode", "data": "1"}], "hold-time": 50},
        ),
        (
            "send-key",
            {"keys": [{"type": "qcode", "data": "minus"}], "hold-time": 50},
        ),
    ]
