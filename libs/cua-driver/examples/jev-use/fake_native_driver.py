#!/usr/bin/env python3
"""A scripted stand-in for `cua-driver mcp` that lets the native runners run offline.

It speaks just enough MCP over stdio (newline-delimited JSON-RPC) for
`run_native.py` and `run_native.ts`: `initialize`, `tools/list`, and the tools a
native task calls. Windows come from the recorded `fixtures/native/` window
states; a click updates the task-state file the runner's oracle reads. It makes
no claim about real Driver latency: each phase sleeps a fixed, configured time so
tests can assert lower bounds and phase relationships, not absolute speed.

Configuration (forwarded to the Driver process because of the CUA_DRIVER_ prefix):
  CUA_DRIVER_FAKE_SCENARIO    counter | canvas
  CUA_DRIVER_FAKE_STATE_FILE  task-state file the click tool updates
  CUA_DRIVER_FAKE_PID         pid to report and to write into the state file
  CUA_DRIVER_FAKE_DELAYS_MS   JSON {"observe": n, "parse": n, "act": n}
  CUA_DRIVER_FAKE_CALLS_FILE  optional file that receives one tool name per tools/call line
"""

from __future__ import annotations

import copy
import json
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
FIXTURES = ROOT / "fixtures" / "native"

SCENARIOS = {
    "counter": {
        "window_state": "appkit-window-state-initial-v1.json",
        "schema": "cua.appkit_task_state_v1",
        "title": "CuaTestHarness AppKit",
    },
    "canvas": {
        "window_state": "canvas-linux-window-state-initial-v1.json",
        "schema": "cua.visual_canvas_task_state_v1",
        "title": "Cua Visual-Only Canvas Fixture",
    },
}


def load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


class FakeDriver:
    def __init__(self) -> None:
        self.scenario = SCENARIOS[os.environ["CUA_DRIVER_FAKE_SCENARIO"]]
        self.state_file = Path(os.environ["CUA_DRIVER_FAKE_STATE_FILE"])
        self.pid = int(os.environ["CUA_DRIVER_FAKE_PID"])
        self.delays = json.loads(os.environ.get("CUA_DRIVER_FAKE_DELAYS_MS", "{}"))
        self.window_state = load(self.scenario["window_state"])
        self.counter = 0

    def sleep(self, phase: str) -> None:
        time.sleep(float(self.delays.get(phase, 0)) / 1000)

    def write_state(self, **fields: object) -> None:
        state = {"schema": self.scenario["schema"], "pid": self.pid, **fields}
        self.state_file.write_text(json.dumps(state), encoding="utf-8")

    def tools(self) -> list[dict]:
        def tool(name: str, **properties: dict) -> dict:
            return {"name": name, "description": name,
                    "inputSchema": {"type": "object", "properties": properties}}

        return [
            tool("list_windows", pid={"type": "integer"}),
            tool("get_window_state", pid={"type": "integer"}, window_id={"type": "integer"}),
            tool("parse_visual_regions", capture_id={"type": "string"}),
            tool("click", pid={"type": "integer"}, capture_id={"type": "string"},
                 element_token={"type": "string"}),
        ]

    def call(self, name: str, arguments: dict) -> dict:
        calls_file = os.environ.get("CUA_DRIVER_FAKE_CALLS_FILE")
        if calls_file:
            with open(calls_file, "a", encoding="utf-8") as stream:
                stream.write(name + "\n")
        if name == "list_windows":
            return {"windows": [{"window_id": self.window_state["window_id"], "pid": self.pid,
                                 "title": self.scenario["title"], "is_on_screen": True}]}
        if name == "get_window_state":
            self.sleep("observe")
            payload = copy.deepcopy(self.window_state)
            payload["pid"] = self.pid
            return payload
        if name == "parse_visual_regions":
            self.sleep("parse")
            capture = self.window_state
            template = json.loads((ROOT / "fixtures" / "parse-visual-regions-submit-v1.json").read_text(encoding="utf-8"))
            template["capture"]["capture_id"] = arguments["capture_id"]
            template["capture"]["source"] = {"kind": "window", "pid": self.pid,
                                             "window_id": capture["window_id"]}
            template["capture"]["screenshot"]["width"] = capture["screenshot_width"]
            template["capture"]["screenshot"]["height"] = capture["screenshot_height"]
            template["regions"] = [{"id": "cancel-text", "kind": "text",
                                    "bounds": {"x": 300, "y": 240, "width": 100, "height": 40},
                                    "text": "Cancel", "confidence": 0.96,
                                    "interactive": True, "reading_order": 1}]
            return template
        if name == "click":
            self.sleep("act")
            if self.scenario["schema"] == "cua.visual_canvas_task_state_v1":
                self.write_state(selected="cancel", action_count=1)
            else:
                self.counter += 1
                self.write_state(counter=self.counter)
            return {"ok": True}
        raise ValueError(f"unknown tool {name}")


def main() -> None:
    driver = FakeDriver()
    driver.write_state(**({"selected": None, "action_count": 0}
                          if driver.scenario["schema"] == "cua.visual_canvas_task_state_v1"
                          else {"counter": 0}))
    for line in sys.stdin:
        if not line.strip():
            continue
        message = json.loads(line)
        method, request_id = message.get("method"), message.get("id")
        if request_id is None:
            continue  # notifications need no reply
        if method == "initialize":
            result = {"protocolVersion": message["params"]["protocolVersion"],
                      "capabilities": {"tools": {}},
                      "serverInfo": {"name": "fake-native-driver", "version": "0"}}
        elif method == "tools/list":
            result = {"tools": driver.tools()}
        elif method == "tools/call":
            params = message["params"]
            structured = driver.call(params["name"], params.get("arguments", {}))
            result = {"content": [{"type": "text", "text": "ok"}],
                      "structuredContent": structured, "isError": False}
        else:
            result = {}
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()