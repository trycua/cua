"""Opt-in native GTK fixture proof through the real Driver MCP transport.

Run only in a disposable Hyprland session with the two isolated-input GTK
fixtures already mapped. An external operator must sign grant-request.json
and place ONLY the public grant in grant.json. This process cannot sign.
This focused gate supplements, rather than replaces, the desktop matrix.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import time


def wait_for(check, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("bounded evidence wait expired")


def journal(path):
    # Ignore an in-progress final line, never manufacture missing evidence.
    return [json.loads(line) for line in path.read_text().splitlines()
            if line.endswith("}")]


def state(path):
    return next(row for row in reversed(journal(path)) if row["kind"] == "state")


def wm():
    def query(name):
        return json.loads(subprocess.check_output(["hyprctl", "-j", name], timeout=5))
    active = query("activewindow")
    return {"pid": active.get("pid"), "address": active.get("address"),
            "cursor": query("cursorpos"), "workspace": query("activeworkspace")["id"]}


class MCP:
    def __init__(self, args):
        self.directory = args.evidence
        self.log = (self.directory / "mcp.stderr").open("w")
        self.process = subprocess.Popen(
            # The service must already be started with unrestricted permission
            # mode and --dangerously-bypass-approvals; the MCP proxy cannot set it.
            [str(args.driver), "mcp", "--socket", str(args.driver_socket)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
            text=True, bufsize=1)
        self.counter = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                               "clientInfo": {"name": "isolated-input-fixture", "version": "1"}})
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.process.stdin.flush()

    def rpc(self, method, params):
        self.counter += 1
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.counter,
                                            "method": method, "params": params}) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if not select.select([self.process.stdout], [], [], 1)[0]:
                continue
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("MCP closed before response")
            response = json.loads(line)
            if response.get("id") != self.counter:
                continue
            if "error" in response:
                raise RuntimeError(response["error"])
            result = response["result"]
            for index, content in enumerate(result.get("content", [])):
                if content.get("type") == "image":
                    image = self.directory / f"{self.counter:03d}-{index}.png"
                    image.write_bytes(base64.b64decode(content.pop("data")))
                    content["image_file"] = image.name
            (self.directory / f"{self.counter:03d}.json").write_text(json.dumps(result, indent=2))
            return result
        raise TimeoutError("MCP response timeout; never replay the action")

    def tool(self, name, arguments):
        return self.rpc("tools/call", {"name": name, "arguments": arguments})

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=5)
        self.log.close()


def run(args):
    args.evidence.mkdir(parents=True, exist_ok=False)
    mcp, grab, operator = MCP(args), None, None
    try:
        windows = mcp.tool("list_windows", {})["structuredContent"]["windows"]
        def fixture(name):
            matches = [w for w in windows if w["title"] == "Cua Isolated Input " + name]
            assert len(matches) == 1, "need one exact synthetic fixture per actor"
            return {key: matches[0][key] for key in ("pid", "window_id")}
        background, foreground = fixture("Background"), fixture("Foreground")
        assert background["pid"] != foreground["pid"]
        def snapshot(target):
            result = mcp.tool("get_window_state", target)
            assert not result.get("isError"), result
            return result["structuredContent"]
        bg_before = snapshot(background)
        fg_before = snapshot(foreground)
        assert bg_before["screenshot_width"] > 400 and bg_before["screenshot_height"] > 300
        pending = mcp.tool("click", {**background, "x": 200, "y": 300, "delivery_mode": "background"})
        snapshot(background)
        grant_request = pending.get("structuredContent", {})
        assert pending.get("isError") and grant_request.get("reason") == "pending_operator_approval", pending
        (args.evidence / "grant-request.json").write_text(json.dumps(grant_request))
        print(json.dumps({"event": "grant_required", **grant_request}), flush=True)
        grant_path = args.evidence / "grant.json"
        wait_for(grant_path.exists, 120)
        grant = json.loads(grant_path.read_text())
        for key in ("epoch", "challenge", "target"):
            assert grant[key] == grant_request[key], "grant belongs to another pending request"
        operator = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        operator.settimeout(5)
        operator.connect(str(args.input_socket))
        def exchange(packet):
            operator.sendall(packet.encode("ascii"))
            return json.loads(operator.recv(2048))
        assert exchange("HELLO")["ok"]
        assert exchange(grant["packet"])["ok"], "operator approval failed"
        fg_box = fg_before["window_bounds"]
        desktop = mcp.tool("get_desktop_state", {})["structuredContent"]
        # Fixture-only adversary, not the background action actuator.
        grab = subprocess.Popen([str(args.primary_grab), str(fg_box["x"] + 300),
                                 str(fg_box["y"] + 300), str(desktop["screen_width"]),
                                 str(desktop["screen_height"]), "60000"], stdout=subprocess.PIPE, text=True)
        assert wait_for(lambda: select.select([grab.stdout], [], [], 0)[0])
        assert grab.stdout.readline().strip() == "HELD"
        wait_for(lambda: state(args.foreground_journal)["held"])
        snapshot(foreground)
        primary_before = wm()
        assert primary_before["pid"] == foreground["pid"]
        foreground_baseline = state(args.foreground_journal)
        background_baseline = state(args.background_journal)
        background_offset = len(journal(args.background_journal))
        wire_offset = args.foreground_wire.stat().st_size
        actions = [
            ("click", {"x": 200, "y": 300}),
            ("press_key", {"key": "a"}),
            ("hotkey", {"keys": ["shift", "b"]}),
            ("scroll", {"x": 200, "y": 300, "direction": "down", "amount": 3}),
            ("drag", {"from_x": 200, "from_y": 300, "to_x": 350, "to_y": 320, "duration_ms": 500}),
        ]
        for name, parameters in actions:
            snapshot(background)
            result = mcp.tool(name, {**background, **parameters, "delivery_mode": "background"})
            snapshot(background)
            snapshot(foreground)
            assert not result.get("isError"), (name, result)
            assert result["structuredContent"]["route"] == "synthetic_events", (name, result)
            assert result["structuredContent"]["effect"] == "unverifiable", (name, result)
            assert wm() == primary_before, "foreground focus, workspace, or cursor changed"
            current = state(args.foreground_journal)
            for key in ("clicks", "keys", "held"):
                assert current[key] == foreground_baseline[key], (key, current)
        final = wait_for(lambda: state(args.background_journal)
                         if state(args.background_journal)["clicks"] >= background_baseline["clicks"] + 2 else None)
        assert "a" in final["keys"] and "B" in final["keys"], final
        assert final["scroll"] > background_baseline["scroll"] and not final["held"], final
        assert final["motion"] > background_baseline["motion"] + 2, final
        presses = [row for row in journal(args.background_journal)[background_offset:]
                   if row["kind"] == "button-press"]
        assert presses, "missing raw pointer coordinate evidence"
        scale_x = bg_before["window_bounds"]["width"] / bg_before["screenshot_width"]
        scale_y = bg_before["window_bounds"]["height"] / bg_before["screenshot_height"]
        assert abs(presses[0]["x"] - 200 * scale_x) < 1, presses[0]
        assert abs(presses[0]["y"] - 300 * scale_y) < 1, presses[0]
        with args.foreground_wire.open("rb") as stream:
            stream.seek(wire_offset)
            wire = stream.read().decode()
        assert not any("wl_pointer#" in line and any(event in line for event in (".leave(", ".enter(", ".button("))
                       for line in wire.splitlines()), "primary pointer changed during background actions"
        assert not any("wl_keyboard#" in line and any(event in line for event in (".leave(", ".enter(", ".key("))
                       for line in wire.splitlines()), "primary keyboard changed during background actions"
        assert exchange("STOP")["ok"]
        snapshot(background)
        stopped = mcp.tool("click", {**background, "x": 200, "y": 300, "delivery_mode": "background"})
        snapshot(background)
        assert stopped.get("isError") and stopped["structuredContent"]["reason"] == "pending_operator_approval"
        result = {"result": "passed", "actions": [a[0] for a in actions],
                  "foreground_grab_preserved": True, "primary_wire_unchanged": True,
                  "foreground_focus_cursor_workspace_preserved": True, "stop_revoked": True,
                  "background_before": background_baseline, "background_after": final,
                  "full_desktop_matrix": False}
        (args.evidence / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result), flush=True)
    finally:
        if operator:
            try:
                operator.send(b"STOP")
                operator.recv(2048)
            except OSError:
                pass
            operator.close()
        if grab and grab.poll() is None:
            grab.terminate()
            grab.wait(timeout=5)
        mcp.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("driver", "driver-socket", "input-socket", "evidence", "primary-grab",
                 "background-journal", "foreground-journal", "foreground-wire"):
        parser.add_argument("--" + name, type=Path, required=True)
    run(parser.parse_args())
