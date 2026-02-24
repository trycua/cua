from fastapi.testclient import TestClient

from computer_server import main


async def _fake_run_command(command: str):
    return {"stdout": f"ok:{command}", "stderr": "", "return_code": 0}


def test_cmd_accepts_params(monkeypatch):
    monkeypatch.setattr(main, "handlers", {"run_command": _fake_run_command})
    client = TestClient(main.app)

    res = client.post("/cmd", json={"command": "run_command", "params": {"command": "echo hi"}})

    assert res.status_code == 200
    assert "\"success\": true" in res.text
    assert "ok:echo hi" in res.text


def test_cmd_accepts_args_legacy(monkeypatch):
    monkeypatch.setattr(main, "handlers", {"run_command": _fake_run_command})
    client = TestClient(main.app)

    res = client.post("/cmd", json={"command": "run_command", "args": {"command": "echo hi"}})

    assert res.status_code == 200
    assert "\"success\": true" in res.text
    assert "ok:echo hi" in res.text


def test_run_command_legacy_endpoint(monkeypatch):
    monkeypatch.setattr(main, "handlers", {"run_command": _fake_run_command})
    client = TestClient(main.app)

    res = client.post("/run_command", json={"command": "echo legacy"})

    assert res.status_code == 200
    assert "\"success\": true" in res.text
    assert "ok:echo legacy" in res.text
