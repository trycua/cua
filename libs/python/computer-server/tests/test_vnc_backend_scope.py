"""Fail-closed transport coverage for the remote VNC backend."""

from unittest.mock import AsyncMock, Mock

import pytest
from fastapi.testclient import TestClient
from fastmcp.exceptions import NotFoundError

from computer_server.backend_policy import (
    VNC_REMOTE_COMMANDS,
    VNC_REMOTE_MCP_TOOLS,
    VNC_UNSUPPORTED_CODE,
    VNCUnavailableHandler,
    exposed_command_registry,
)
from computer_server.handlers.factory import HandlerFactory
from computer_server.mcp_server import create_mcp_server


@pytest.fixture
def vnc_backend(monkeypatch):
    monkeypatch.setenv("CUA_BACKEND", "vnc")
    monkeypatch.setenv("CUA_VNC_HOST", "127.0.0.1")


@pytest.mark.asyncio
async def test_vnc_factory_never_constructs_host_file_desktop_or_window_handlers(
    vnc_backend, tmp_path
):
    handlers = HandlerFactory.create_handlers()

    assert all(isinstance(handler, VNCUnavailableHandler) for handler in handlers[3:])

    marker = tmp_path / "must-not-exist"
    file_result = await handlers[3].write_text(str(marker), "host mutation")
    shell_result = await handlers[1].run_command(f"touch {marker}")

    assert file_result["code"] == VNC_UNSUPPORTED_CODE
    assert shell_result["code"] == VNC_UNSUPPORTED_CODE
    assert not marker.exists()


@pytest.mark.asyncio
async def test_vnc_mcp_registry_contains_only_remote_target_tools(vnc_backend, tmp_path):
    server = create_mcp_server()
    names = {tool.name for tool in await server.list_tools()}

    assert names == VNC_REMOTE_MCP_TOOLS

    marker = tmp_path / "must-not-exist"
    with pytest.raises(NotFoundError, match="computer_file_write"):
        await server.call_tool(
            "computer_file_write",
            {"path": str(marker), "content": "host mutation"},
        )
    assert not marker.exists()


@pytest.mark.asyncio
async def test_non_vnc_mcp_registry_keeps_existing_host_tools(monkeypatch):
    monkeypatch.setenv("CUA_BACKEND", "native")
    monkeypatch.delenv("CUA_VNC_HOST", raising=False)

    names = {tool.name for tool in await create_mcp_server().list_tools()}

    assert {
        "computer_screenshot",
        "computer_run_command",
        "computer_file_write",
        "computer_set_wallpaper",
        "computer_close_window",
    } <= names


def test_vnc_command_registry_is_an_allowlist(vnc_backend):
    candidate = {name: object() for name in VNC_REMOTE_COMMANDS}
    candidate["future_host_operation"] = object()

    exposed = exposed_command_registry(candidate)

    assert set(exposed) == VNC_REMOTE_COMMANDS


def test_non_vnc_command_registry_is_unchanged(monkeypatch):
    monkeypatch.setenv("CUA_BACKEND", "native")
    monkeypatch.delenv("CUA_VNC_HOST", raising=False)
    candidate = {"run_command": object(), "screenshot": object()}

    assert exposed_command_registry(candidate) == candidate


def test_vnc_http_and_websocket_commands_refuse_before_host_mutation(
    vnc_backend, monkeypatch, tmp_path
):
    from computer_server import main

    marker = tmp_path / "must-not-exist"

    async def write_text(path: str, content: str):
        marker.write_text(content)
        return {"success": True}

    monkeypatch.setattr(
        main,
        "handlers",
        exposed_command_registry(
            {
                "write_text": write_text,
                "screenshot": AsyncMock(return_value={"success": True}),
            }
        ),
    )
    monkeypatch.setattr(main, "COMMAND_ALIASES", {})

    with TestClient(main.app) as client:
        advertised = client.get("/commands").json()
        response = client.post(
            "/cmd",
            json={"command": "write_text", "params": {"path": str(marker), "content": "x"}},
        )
        assert response.status_code == 400

        with client.websocket_connect("/ws") as websocket:
            websocket.send_json(
                {
                    "command": "write_text",
                    "params": {"path": str(marker), "content": "x"},
                }
            )
            result = websocket.receive_json()

    assert set(advertised["commands"]) == {"screenshot"}
    assert advertised["aliases"] == {}
    assert result["success"] is False
    assert "Unknown command" in result["error"]
    assert not marker.exists()


def test_vnc_pty_and_browser_http_surfaces_are_refused(vnc_backend, monkeypatch):
    from computer_server import main

    create_pty = AsyncMock()
    browser_command = AsyncMock()
    monkeypatch.setattr(main.pty_manager, "create", create_pty)
    monkeypatch.setattr(main.get_browser_manager(), "execute_command", browser_command)

    with TestClient(main.app) as client:
        pty_response = client.post("/pty", json={"command": "echo wrong-host"})
        browser_response = client.post(
            "/playwright_exec",
            json={"command": "visit_url", "params": {"url": "https://example.com"}},
        )

    assert pty_response.status_code == 409
    assert pty_response.json()["code"] == VNC_UNSUPPORTED_CODE
    assert browser_response.status_code == 409
    assert browser_response.json()["code"] == VNC_UNSUPPORTED_CODE
    create_pty.assert_not_awaited()
    browser_command.assert_not_awaited()


def test_vnc_pty_websocket_is_refused_before_subscription(vnc_backend, monkeypatch):
    from computer_server import main

    subscribe = Mock()
    monkeypatch.setattr(main.pty_manager, "subscribe", subscribe)

    with TestClient(main.app) as client:
        with client.websocket_connect("/pty/123/ws") as websocket:
            result = websocket.receive_json()

    assert result["code"] == VNC_UNSUPPORTED_CODE
    subscribe.assert_not_called()


@pytest.mark.asyncio
async def test_vnc_direct_agent_interface_refuses_host_browser(vnc_backend):
    from computer_server.main import DirectComputerInterface

    browser = AsyncMock()
    interface = DirectComputerInterface(automation_handler=object(), browser_manager=browser)

    result = await interface.playwright_exec("visit_url", {"url": "https://example.com"})

    assert result["code"] == VNC_UNSUPPORTED_CODE
    browser.execute_command.assert_not_awaited()


@pytest.mark.asyncio
async def test_non_vnc_direct_agent_interface_keeps_browser_behavior(monkeypatch):
    from computer_server.main import DirectComputerInterface

    monkeypatch.setenv("CUA_BACKEND", "native")
    monkeypatch.delenv("CUA_VNC_HOST", raising=False)
    browser = AsyncMock()
    browser.execute_command.return_value = {"success": True}
    interface = DirectComputerInterface(automation_handler=object(), browser_manager=browser)

    result = await interface.playwright_exec("visit_url", {"url": "https://example.com"})

    assert result == {"success": True}
    browser.execute_command.assert_awaited_once()
