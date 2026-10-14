from __future__ import annotations

import pytest

from cua_s1.schema import (
    FIXED_ACTIONS,
    Element,
    Entity,
    decode,
    encode,
    render_context,
    render_options,
)


def test_options_are_entity_pointers_followed_by_fixed_actions():
    entities = [Entity("Primary email", "user@example.invalid"), Entity("City", "Oslo")]

    options = render_options(entities)

    assert options[:2] == [
        "fill Primary email: user@example.invalid",
        "fill City: Oslo",
    ]
    assert options[2:] == list(FIXED_ACTIONS)


def test_action_encoding_round_trips_without_copying_entity_values():
    entities = [Entity("Account", "synthetic-001"), Entity("Account", "synthetic-002")]

    for entity_index in range(len(entities)):
        encoded = encode("fill", entity_index, entities)
        assert decode(encoded, entities) == ("fill", entity_index)
    for action in FIXED_ACTIONS:
        encoded = encode(action, None, entities)
        assert decode(encoded, entities) == (action, None)


def test_invalid_action_encodings_are_rejected():
    entities = [Entity("Name", "Example Person")]

    with pytest.raises((ValueError, IndexError)):
        encode("fill", 4, entities)
    with pytest.raises((ValueError, IndexError)):
        encode("unknown", None, entities)
    with pytest.raises((ValueError, IndexError)):
        decode(len(entities) + len(FIXED_ACTIONS), entities)
    with pytest.raises((ValueError, IndexError)):
        decode(-1, entities)


def test_context_is_bounded_and_represents_checkbox_state():
    long_text = "x" * 500
    context = render_context(
        long_text,
        Element("CheckBox", long_text, checked=True),
        placeholder=long_text,
    )

    assert "checked" in context
    assert "unchecked" not in context
    assert len(context.encode("utf-8")) < 400
