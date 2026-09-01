"""Shift delivery for the VNC backend.

RFB leaves keysym-to-key translation to the server, so a compliant server
presses Shift itself when it receives the ``underscore`` keysym. QEMU only does
that for uppercase letters, so shifted punctuation arrives unshifted and
``Hello_World`` is typed as ``Hello-World``. ``--vnc-force-caps`` moves the
modifier to the client.
"""

import pytest
from computer_server.backend_policy import env_flag, vnc_force_caps
from computer_server.cli import parse_args
from computer_server.handlers.vnc import VNCAutomationHandler, _VNCConnection

# The exact characters from the bug report that QEMU types unshifted.
SHIFTED_PUNCTUATION = "_(>)&{|}#~$"


@pytest.fixture
def vnc_client_factory():
    """Return a builder for a real vncdotool client bound to a cua-built factory."""

    pytest.importorskip("vncdotool", reason="requires cua-computer-server[vnc]")
    from vncdotool.client import VNCDoToolClient

    def build(force_caps: bool):
        connection = _VNCConnection("127.0.0.1", 5900, "secret", force_caps=force_caps)
        client = VNCDoToolClient.__new__(VNCDoToolClient)
        client.factory = connection._build_factory()
        return client

    return build


def _record_key_events(client, key: str):
    """Return the (keysym, down) events ``keyPress`` would put on the wire."""

    events: list = []
    client.keyEvent = lambda keysym, down: events.append((keysym, down))
    client.keyPress(key)
    return events


def test_shifted_punctuation_reaches_the_wire_without_shift_by_default(vnc_client_factory):
    client = vnc_client_factory(force_caps=False)

    for character in SHIFTED_PUNCTUATION:
        assert _record_key_events(client, character) == [
            (ord(character), True),
            (ord(character), False),
        ]


def test_force_caps_sends_shift_around_every_shifted_character(vnc_client_factory):
    from vncdotool.client import KEYMAP

    client = vnc_client_factory(force_caps=True)
    shift = KEYMAP["shift"]

    for character in SHIFTED_PUNCTUATION + "A":
        assert _record_key_events(client, character) == [
            (shift, True),
            (ord(character), True),
            (ord(character), False),
            (shift, False),
        ]


def test_force_caps_leaves_unshifted_characters_alone(vnc_client_factory):
    client = vnc_client_factory(force_caps=True)

    for character in "a4-=[]`":
        assert _record_key_events(client, character) == [
            (ord(character), True),
            (ord(character), False),
        ]


def test_force_caps_preserves_the_password(vnc_client_factory):
    assert vnc_client_factory(force_caps=True).factory.password == "secret"


def test_handler_defaults_to_server_side_shift():
    pytest.importorskip("vncdotool", reason="requires cua-computer-server[vnc]")

    assert VNCAutomationHandler(host="127.0.0.1")._conn._build_factory().force_caps is False


def test_handler_forwards_force_caps_to_every_connection():
    pytest.importorskip("vncdotool", reason="requires cua-computer-server[vnc]")

    handler = VNCAutomationHandler(host="127.0.0.1", force_caps=True)

    assert handler._conn._build_factory().force_caps is True


def test_factory_builds_a_force_caps_handler_from_the_environment(monkeypatch):
    pytest.importorskip("vncdotool", reason="requires cua-computer-server[vnc]")
    monkeypatch.setenv("CUA_BACKEND", "vnc")
    monkeypatch.setenv("CUA_VNC_HOST", "127.0.0.1")
    monkeypatch.setenv("CUA_VNC_FORCE_CAPS", "true")
    from computer_server.handlers.factory import HandlerFactory

    automation = HandlerFactory.create_handlers()[1]

    assert automation._conn._build_factory().force_caps is True


def test_cli_flag_is_off_unless_requested():
    assert parse_args(["--backend", "vnc", "--vnc-host", "127.0.0.1"]).vnc_force_caps is False


def test_cli_flag_selects_client_side_shift():
    args = parse_args(["--backend", "vnc", "--vnc-host", "127.0.0.1", "--vnc-force-caps"])

    assert args.vnc_force_caps is True


def test_cli_flag_defaults_to_the_environment_variable(monkeypatch):
    monkeypatch.setenv("CUA_VNC_FORCE_CAPS", "1")

    assert parse_args(["--backend", "vnc", "--vnc-host", "127.0.0.1"]).vnc_force_caps is True


@pytest.mark.parametrize("value", ["1", "true", "TRUE", "yes", "on", " true "])
def test_env_flag_accepts_common_truthy_spellings(monkeypatch, value):
    monkeypatch.setenv("CUA_VNC_FORCE_CAPS", value)

    assert vnc_force_caps() is True


@pytest.mark.parametrize("value", ["", "0", "false", "no", "off", "maybe"])
def test_env_flag_rejects_everything_else(monkeypatch, value):
    monkeypatch.setenv("CUA_VNC_FORCE_CAPS", value)

    assert vnc_force_caps() is False


def test_env_flag_is_false_when_unset(monkeypatch):
    monkeypatch.delenv("CUA_VNC_FORCE_CAPS", raising=False)

    assert env_flag("CUA_VNC_FORCE_CAPS") is False
