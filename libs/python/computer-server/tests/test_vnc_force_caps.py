"""Shift delivery for the VNC backend.

RFB leaves keysym-to-key translation to the server, so a compliant server
presses Shift itself when it receives the ``underscore`` keysym. QEMU only does
that for uppercase letters, so shifted punctuation arrives unshifted and
``Hello_World`` is typed as ``Hello-World``. ``--vnc-force-caps`` moves the
modifier to the client.
"""

import os
import sys
from types import ModuleType, SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest
from computer_server.backend_policy import env_flag, vnc_force_caps
from computer_server.cli import parse_args
from computer_server.handlers.vnc import VNCAutomationHandler, _VNCConnection

# The exact characters from the bug report that QEMU types unshifted.
SHIFTED_PUNCTUATION = "_(>)&{|}#~$"


def _require_vncdotool() -> None:
    """Require the `vnc` extra, except when running outside CI without it.

    CI installs `cua-computer-server[vnc]` for this package, so a missing import
    there is a packaging regression and must fail rather than quietly skip.
    """

    if os.environ.get("CI"):
        import vncdotool  # noqa: F401
    else:
        pytest.importorskip("vncdotool", reason="requires cua-computer-server[vnc]")


@pytest.fixture
def vnc_client_factory():
    """Return a builder for a real vncdotool client bound to a cua-built factory."""

    _require_vncdotool()
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
    _require_vncdotool()

    assert VNCAutomationHandler(host="127.0.0.1")._conn._build_factory().force_caps is False


def test_handler_forwards_force_caps_to_every_connection():
    _require_vncdotool()

    handler = VNCAutomationHandler(host="127.0.0.1", force_caps=True)

    assert handler._conn._build_factory().force_caps is True


def test_factory_builds_a_force_caps_handler_from_the_environment(monkeypatch):
    _require_vncdotool()
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


def test_computer_forwards_force_caps_to_the_server_env():
    """The SDK has to hand the flag down, or --vnc-force-caps only helps people
    who start computer-server by hand."""
    from computer.computer import Computer

    computer = Computer(
        use_host_computer_server=True,
        backend="vnc",
        vnc_host="127.0.0.1",
        vnc_password="secret",
        vnc_force_caps=True,
    )

    assert computer._backend_env() == {
        "CUA_BACKEND": "vnc",
        "CUA_VNC_HOST": "127.0.0.1",
        "CUA_VNC_PORT": "5900",
        "CUA_VNC_PASSWORD": "secret",
        "CUA_VNC_FORCE_CAPS": "true",
    }


def test_computer_leaves_force_caps_unset_by_default():
    from computer.computer import Computer

    computer = Computer(use_host_computer_server=True, backend="vnc", vnc_host="127.0.0.1")

    assert "CUA_VNC_FORCE_CAPS" not in computer._backend_env()


def test_computer_sends_no_backend_env_for_the_native_backend():
    from computer.computer import Computer

    assert Computer(use_host_computer_server=True)._backend_env() == {}


def test_computer_preserves_positional_run_opts():
    from computer.computer import Computer

    run_opts = {"env": {"CUSTOM_OPTION": "preserved"}}
    computer = Computer(
        "1024x768",
        "8GB",
        "4",
        "macos",
        "",
        None,
        None,
        True,
        20,
        False,
        "lume",
        7777,
        8006,
        None,
        "localhost",
        None,
        None,
        False,
        None,
        None,
        None,
        None,
        100,
        "vnc",
        "127.0.0.1",
        5900,
        "",
        run_opts,
    )

    assert computer.custom_run_opts == run_opts
    assert computer.vnc_force_caps is False
    assert "CUA_VNC_FORCE_CAPS" not in computer._backend_env()


def test_cli_startup_passes_force_caps_to_server(monkeypatch):
    from computer_server.cli import main

    # Restore every environment variable CLI startup can change.
    for name in ("CUA_BACKEND", "CUA_VNC_HOST", "CUA_VNC_PORT", "CUA_VNC_FORCE_CAPS"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(
        sys, "argv", ["computer-server", "--vnc-host", "127.0.0.1", "--vnc-force-caps"]
    )
    startup_env = {}
    server = Mock()
    server.start.side_effect = lambda: startup_env.update(os.environ)
    server_module = ModuleType("computer_server.server")
    server_module.Server = Mock(return_value=server)
    monkeypatch.setitem(sys.modules, "computer_server.server", server_module)

    main()

    server.start.assert_called_once_with()
    assert startup_env["CUA_BACKEND"] == "vnc"
    assert startup_env["CUA_VNC_FORCE_CAPS"] == "true"


@pytest.mark.asyncio
async def test_computer_run_passes_force_caps_to_provider(monkeypatch):
    from computer.computer import Computer, InterfaceFactory, VMProviderFactory, helpers

    provider = AsyncMock()
    provider.get_vm.return_value = {"status": "stopped"}
    monkeypatch.setattr(VMProviderFactory, "create_provider", Mock(return_value=provider))
    interface = AsyncMock()
    monkeypatch.setattr(InterfaceFactory, "create_interface_for_os", Mock(return_value=interface))
    monkeypatch.setattr(helpers, "set_default_computer", Mock())
    computer = Computer(
        name="force-caps-test",
        provider_type="lume",
        telemetry_enabled=False,
        backend="vnc",
        vnc_host="127.0.0.1",
        vnc_force_caps=True,
    )
    monkeypatch.setattr(computer, "get_ip", AsyncMock(return_value="127.0.0.1"))

    try:
        await computer.run()

        provider.run_vm.assert_awaited_once()
        options = provider.run_vm.call_args.kwargs["run_opts"]
        assert options["env"]["CUA_BACKEND"] == "vnc"
        assert options["env"]["CUA_VNC_FORCE_CAPS"] == "true"
        interface.wait_for_ready.assert_awaited_once()
    finally:
        if hasattr(computer, "_keep_alive_task"):
            computer._stop_event.set()
            await computer._keep_alive_task


def test_connection_uses_force_caps_factory(monkeypatch):
    _require_vncdotool()
    import twisted.internet

    client = SimpleNamespace(transport=Mock(), keyPress=Mock())
    factories = []

    def connect(host, port, factory):
        assert (host, port) == ("127.0.0.1", 5900)
        factories.append(factory)
        factory.deferred.callback(client)

    reactor = SimpleNamespace(
        running=True,
        connectTCP=Mock(side_effect=connect),
        callFromThread=lambda work: work(),
    )
    monkeypatch.setattr(twisted.internet, "reactor", reactor)
    connection = _VNCConnection("127.0.0.1", 5900, "secret", force_caps=True)

    connection.key_press("_")

    reactor.connectTCP.assert_called_once()
    assert factories[0].force_caps is True
    assert factories[0].password == "secret"
    client.keyPress.assert_called_once_with("_")
    client.transport.loseConnection.assert_called_once_with()
