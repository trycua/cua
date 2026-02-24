from computer_server.handlers.linux import LinuxAccessibilityHandler


def test_find_element_ignores_transport_noise(monkeypatch):
    handler = LinuxAccessibilityHandler()

    class _RunResult:
        def __init__(self, returncode: int, stdout: str):
            self.returncode = returncode
            self.stdout = stdout

    def fake_run(cmd, capture_output=True, text=True, timeout=2, check=False):
        # wmctrl path: no window match
        if cmd[:2] == ["wmctrl", "-lx"]:
            return _RunResult(0, "")
        # ps path: includes self-noise (curl /cmd find_element) and one real match
        if cmd[:2] == ["ps", "-eo"]:
            return _RunResult(
                0,
                "1234 curl curl -X POST http://127.0.0.1:18000/cmd ... find_element ...\n"
                "4321 firefox /usr/lib/firefox/firefox --new-window\n",
            )
        return _RunResult(1, "")

    monkeypatch.setattr("computer_server.handlers.linux.subprocess.run", fake_run)

    result = __import__("asyncio").run(handler.find_element(title="firefox"))

    assert result["success"] is True
    assert result["element"]["backend"] == "ps"
    assert result["element"]["pid"] == 4321


def test_find_element_returns_false_when_only_transport_noise(monkeypatch):
    handler = LinuxAccessibilityHandler()

    class _RunResult:
        def __init__(self, returncode: int, stdout: str):
            self.returncode = returncode
            self.stdout = stdout

    def fake_run(cmd, capture_output=True, text=True, timeout=2, check=False):
        if cmd[:2] == ["wmctrl", "-lx"]:
            return _RunResult(0, "")
        if cmd[:2] == ["ps", "-eo"]:
            return _RunResult(
                0,
                "1234 curl curl -X POST http://127.0.0.1:18000/cmd ... find_element ...\n",
            )
        return _RunResult(1, "")

    monkeypatch.setattr("computer_server.handlers.linux.subprocess.run", fake_run)

    result = __import__("asyncio").run(handler.find_element(title="xterm"))

    assert result["success"] is False
