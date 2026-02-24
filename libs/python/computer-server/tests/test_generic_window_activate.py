import asyncio

from computer_server.handlers.generic import GenericWindowHandler


def test_activate_window_converts_int_id_to_hex(monkeypatch):
    handler = GenericWindowHandler()
    captured = {}

    monkeypatch.setattr("computer_server.handlers.generic.platform.system", lambda: "Linux")
    monkeypatch.setattr("computer_server.handlers.generic.pwc", None)

    def fake_run_cmd(cmd, timeout=3.0):
        captured["cmd"] = cmd
        return 0, "", ""

    monkeypatch.setattr(handler, "_run_cmd", fake_run_cmd)

    result = asyncio.run(handler.activate_window(54525955))

    assert result["success"] is True
    assert captured["cmd"][0:2] == ["wmctrl", "-ia"]
    assert captured["cmd"][2] == hex(54525955)
