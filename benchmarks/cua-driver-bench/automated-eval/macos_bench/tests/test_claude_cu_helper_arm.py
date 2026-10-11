"""Unit tests for arm cc-claude-cu-helper (Amendment 11, "Claude Desktop 2.31226.0 computer-use helper via a minimal
adapter"): arm registration, MCP config, argv, pins, the adapter's MCP surface, coordinate mapping, the helper
JSON-RPC client and the preflight probe, all against stand-ins. Nothing from Claude Desktop is run or read here,
no GUI is touched and no model is called."""

from __future__ import annotations

import base64
import json
import os
import struct
import subprocess
import sys
import tempfile
import textwrap
import unittest
import zlib
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "tools" / "claude_cu_helper"))

import arms  # noqa: E402
import claude_arms as ca  # noqa: E402
import claude_events as ce  # noqa: E402
import cu_helper_mcp as ad  # noqa: E402
import run_bench as rb  # noqa: E402

ARM = "cc-claude-cu-helper"
ADAPTER = HERE.parent / "tools" / "claude_cu_helper" / "cu_helper_mcp.py"


def png(width: int, height: int) -> bytes:
    raw = b"".join(b"\x00" + b"\xff" * (width * 3) for _ in range(height))
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


FAKE_HELPER = textwrap.dedent(
    """\
    #!/usr/bin/env python3
    import json, os, sys
    log = os.environ.get("FAKE_HELPER_LOG")
    for line in sys.stdin:
        req = json.loads(line)
        if log:
            open(log, "a").write(line)
        if req["method"] == "probe":
            res = {"skylight": True, "axTrusted": True, "focusWithoutRaise": True, "os": "26.5.2"}
        elif req["method"] == "wakeChromiumCompositor":
            res = {"woke": False}
        elif req["method"] == "dispatchRaw":
            p = req["params"]
            busy = os.environ.get("FAKE_HELPER_BUSY")
            if busy and not os.path.exists(busy):
                open(busy, "w").write("1")
                res = {"delivered": False, "path": "unavailable", "code": "user_actively_typing"}
            elif p["kind"] == "rclick":
                res = {"delivered": False, "path": "unavailable", "code": "context_menu",
                       "debugFields": {"blockedReason": "context_menu_rclick_refused"}}
            else:
                res = {"delivered": True, "path": "cgevent", "debugFields": {}}
                if p["kind"] == "type":
                    res["charsDelivered"] = len(p["text"])
        else:
            print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": "nope"}}), flush=True)
            continue
        print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": res}), flush=True)
    """
)

WINDOWS = [
    {"app": "BenchLab", "pid": 4242, "window_id": 77, "title": "BenchLab", "x": 100.0, "y": 50.0,
     "width": 800.0, "height": 600.0, "layer": 0},
    {"app": "Finder", "pid": 1, "window_id": 5, "title": "", "x": 0.0, "y": 0.0, "width": 400.0, "height": 300.0, "layer": 0},
]


class Fixture:
    def __init__(self, tmp: Path, shot: tuple[int, int] = (1600, 1200)) -> None:
        self.tmp = tmp
        self.helper = tmp / "app-cu-helper"
        self.helper.write_text(FAKE_HELPER)
        self.helper.chmod(0o755)
        self.winlist = tmp / "winlist"
        self.winlist.write_text("#!/bin/sh\ncat <<'EOF'\n" + json.dumps(WINDOWS) + "\nEOF\n")
        self.winlist.chmod(0o755)
        (tmp / "shot.png").write_bytes(png(*shot))
        self.screencapture = tmp / "screencapture"
        # last argument is the output path; -l<id> must be present
        self.screencapture.write_text(
            "#!/bin/sh\nfor a; do last=$a; done\ncase \"$*\" in *-l77*) cp '%s' \"$last\";; *) exit 1;; esac\n"
            % (tmp / "shot.png")
        )
        self.screencapture.chmod(0o755)
        self.sips = tmp / "sips"  # stand-in: writes a PNG of the requested size (-z H W path)
        self.sips.write_text(
            "#!/usr/bin/env python3\nimport sys\nsys.path.insert(0, %r)\nfrom test_claude_cu_helper_arm import png\n"
            "h, w, path = int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]\nopen(path, 'wb').write(png(w, h))\n"
            % str(HERE)
        )
        self.sips.chmod(0o755)
        self.log = tmp / "helper.log"

    def adapter(self, actions: list[str] | None = None, busy: bool = False) -> ad.Adapter:
        env = {**os.environ, "FAKE_HELPER_LOG": str(self.log)}
        if busy:
            env["FAKE_HELPER_BUSY"] = str(self.tmp / "busy-once")
        helper = ad.Helper([str(self.helper)], env=env)
        return ad.Adapter(helper, str(self.winlist), list(ad.ALL_ACTIONS) if actions is None else actions,
                          host_pid=99, screencapture=str(self.screencapture), sips=str(self.sips), busy_wait_s=0.01)

    def requests(self, method: str = "dispatchRaw") -> list[dict]:
        rows = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return [r for r in rows if r["method"] == method]


class ArmRegistrationTest(unittest.TestCase):
    def test_arm_is_a_named_claude_arm_with_the_agreed_label(self) -> None:
        self.assertIn(ARM, arms.CLAUDE_ARMS)
        self.assertNotIn(ARM, arms.DEFAULT_CLAUDE_ARMS)
        self.assertIn(ARM, ca.CU_HELPER_ARMS)
        self.assertNotIn(ARM, ca.CUA_ARMS)
        self.assertEqual(ca.CU_HELPER_LABEL, "Claude Desktop 2.31226.0 computer-use helper via a minimal adapter")
        self.assertIn(ca.CU_HELPER_LABEL, ca.ARM_DESCRIPTIONS[ARM])

    def test_same_system_prompt_no_skill_and_argv_differs_only_in_server(self) -> None:
        self.assertEqual(ca.system_prompt_for(ARM), ca.SYSTEM_PROMPT)
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(list(ca.prepare_cwd(ARM, Path(tmp) / "w").iterdir()), [])
        common = dict(model="claude-sonnet-5-5", max_turns=45, max_budget_usd=6)
        a = ca.claude_argv(mcp_config=Path("/r/mcp-x.json"), server="claude-cu-helper", **common)
        b = ca.claude_argv(mcp_config=Path("/r/mcp-x.json"), server="arc", **common)
        self.assertEqual([(x, y) for x, y in zip(a, b) if x != y], [("mcp__claude-cu-helper", "mcp__arc")])


class McpConfigTest(unittest.TestCase):
    def test_config_runs_the_adapter_with_absolute_paths_and_a_closed_env(self) -> None:
        pins = {"claude_cu_helper": {"actions": ["click", "type"]}}
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(ca, "load_pins", return_value=pins):
            path, server = ca.mcp_config_for(ARM, Path(tmp))
            self.assertEqual(server, "claude-cu-helper")
            ((name, entry),) = json.loads(path.read_text())["mcpServers"].items()
        self.assertEqual(name, "claude-cu-helper")
        self.assertEqual(entry["command"], "/opt/homebrew/bin/python3")
        self.assertEqual(entry["args"][:3], ["-I", "-B", str(ca.CU_HELPER_ADAPTER)])
        self.assertIn("--helper", entry["args"])
        helper = entry["args"][entry["args"].index("--helper") + 1]
        self.assertTrue(helper.endswith("claude-desktop-2.31226.0/extracted/Claude.app/Contents/Helpers/app-cu-helper"))
        self.assertEqual(entry["args"][-2:], ["--actions", "click,type"])
        self.assertEqual(entry["args"][entry["args"].index("--launcher") + 1], str(ca.CU_HELPER_LAUNCHER))
        self.assertEqual(entry["env"]["PATH"], "/usr/bin:/bin:/usr/sbin:/sbin")
        self.assertNotIn("cua", json.dumps(entry).replace("cua-bench", "").replace("claude-cu", ""))

    def test_pins_must_all_match(self) -> None:
        keys = ("desktop_zip_sha256", "helper_sha256", "adapter_sha256", "winlist_sha256", "launcher_sha256")
        pins = {"claude_cu_helper": {k: "a" for k in keys}}
        self.assertTrue(all(s == "pass" for _, s, _ in ca.check_cu_helper_pins(pins, {k: "a" for k in keys})))
        bad = ca.check_cu_helper_pins(pins, {**{k: "a" for k in keys}, "helper_sha256": "b"})
        self.assertEqual([n for n, s, _ in bad if s == "fail"], ["pin claude_cu_helper.helper_sha256"])
        self.assertTrue(all(s == "fail" for _, s, _ in ca.check_cu_helper_pins({}, {k: None for k in keys})))


class ToolClassTest(unittest.TestCase):
    def test_classes(self) -> None:
        self.assertEqual(ce.tool_class("mcp__claude-cu-helper__app_click", {}), "click")
        self.assertEqual(ce.tool_class("mcp__claude-cu-helper__app_screenshot", {}), "observe")
        self.assertEqual(ce.tool_class("mcp__claude-cu-helper__app_list_windows", {}), "observe")
        self.assertEqual(ce.tool_class("mcp__claude-cu-helper__app_type", {}), "type")
        self.assertEqual(ce.tool_class("mcp__claude-cu-helper__app_hover", {}), "other")
        self.assertEqual(ce.tool_class("mcp__claude-cu-helper__nonsense", {}), "other")


class AdapterTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.fx = Fixture(Path(self._tmp.name))

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def shot(self, adapter: ad.Adapter) -> tuple[int, int]:
        content = adapter.call("app_screenshot", {"app": "benchlab"})
        self.assertEqual(content[1]["type"], "image")
        return ad.png_size(base64.b64decode(content[1]["data"]))

    def test_tools_offered_follow_the_pinned_actions_only(self) -> None:
        names = [t["name"] for t in self.fx.adapter().tools()]
        self.assertEqual(names, ["app_list_windows", "app_screenshot", "app_click", "app_type", "app_key",
                                 "app_scroll", "app_drag", "app_hover"])
        only_click = self.fx.adapter(["click"]).tools()
        self.assertEqual([t["name"] for t in only_click], ["app_list_windows", "app_screenshot", "app_click"])
        self.assertEqual(only_click[2]["inputSchema"]["properties"]["button"]["enum"], ["left"])
        self.assertEqual([t["name"] for t in self.fx.adapter(["nonsense"]).tools()], ["app_list_windows", "app_screenshot"])

    def test_target_size_keeps_aspect_and_budget(self) -> None:
        w, h = ad.target_size(3456, 2234)
        self.assertLessEqual(w * h, ad.MAX_PIXELS)
        self.assertAlmostEqual(w / h, 3456 / 2234, places=2)
        self.assertEqual(ad.target_size(800, 600), (800, 600))  # never upscaled

    def test_screenshot_wakes_the_compositor_then_click_maps_pixels_to_window_points(self) -> None:
        adapter = self.fx.adapter()
        w, h = self.shot(adapter)
        self.assertEqual((w, h), ad.target_size(1600, 1200))
        self.assertEqual(self.fx.requests("wakeChromiumCompositor")[0]["params"]["windowId"], 77)
        out = adapter.call("app_click", {"app": "BenchLab", "coordinate": [w / 2, h / 4], "count": 2})
        self.assertTrue(json.loads(out[0]["text"])["delivered"])
        (req,) = self.fx.requests()
        p = req["params"]
        self.assertEqual((p["pid"], p["windowId"], p["kind"], p["count"], p["hostPid"], p["focusedTarget"],
                          p["nativeMouseVariant"]), (4242, 77, "click", 2, 99, False, False))
        self.assertAlmostEqual(p["winLocalPt"]["x"], 400.0, delta=0.6)
        self.assertAlmostEqual(p["winLocalPt"]["y"], 150.0, delta=0.6)
        adapter.helper.close()

    def test_desktop_mapping_for_type_key_scroll_drag_hover(self) -> None:
        adapter = self.fx.adapter()
        w, h = self.shot(adapter)
        adapter.call("app_type", {"app": "BenchLab", "text": "abc"})
        adapter.call("app_type", {"app": "BenchLab", "text": "xyz", "coordinate": [10, 10], "mode": "replace"})
        adapter.call("app_key", {"app": "BenchLab", "combo": "cmd+shift+Z"})
        adapter.call("app_key", {"app": "BenchLab", "combo": "Enter"})
        adapter.call("app_scroll", {"app": "BenchLab", "coordinate": [10, 10], "dy": 3})
        adapter.call("app_drag", {"app": "BenchLab", "coordinate": [0, 0], "to_coordinate": [w / 2, h / 2]})
        adapter.call("app_hover", {"app": "BenchLab", "coordinate": [w / 2, h / 2]})
        ps = [r["params"] for r in self.fx.requests()]
        self.assertEqual([p["kind"] for p in ps], ["type", "key", "type", "key", "key", "scroll", "drag", "hover"])
        self.assertTrue(ps[0]["focusedTarget"])  # no coordinate: the app's focused element
        self.assertEqual(ps[0]["winLocalPt"], {"x": 400.0, "y": 300.0})
        self.assertEqual((ps[1]["keyName"], ps[1]["modifiers"], ps[1]["partOfTextWrite"], ps[1]["focusedTarget"]),
                         ("a", ["cmd"], True, False))
        self.assertEqual(ps[2]["text"], "xyz")
        self.assertEqual((ps[3]["keyName"], ps[3]["modifiers"]), ("z", ["cmd", "shift"]))
        self.assertEqual((ps[4]["keyName"], ps[4]["modifiers"]), ("return", []))
        self.assertEqual((ps[5]["dy"], ps[5]["dx"], ps[5]["ticks"]), (-120, 0, 1))  # Desktop: round(-dy * 40)
        self.assertAlmostEqual(ps[6]["toWinLocalPt"]["x"], 400.0, delta=0.6)
        adapter.helper.close()

    def test_refusals_come_back_as_tool_errors_with_the_helper_reason(self) -> None:
        adapter = self.fx.adapter()
        self.shot(adapter)
        with self.assertRaisesRegex(RuntimeError, "context_menu_rclick_refused"):
            adapter.call("app_click", {"app": "BenchLab", "coordinate": [1, 1], "button": "right"})
        with self.assertRaisesRegex(ValueError, "not available"):
            self.fx.adapter(["click"]).call("app_click", {"app": "BenchLab", "coordinate": [1, 1], "button": "right"})
        with self.assertRaisesRegex(ValueError, "exactly one key"):
            adapter.call("app_key", {"app": "BenchLab", "combo": "cmd+shift"})
        with self.assertRaisesRegex(ValueError, "longer than"):
            adapter.call("app_type", {"app": "BenchLab", "text": "x" * 4001})
        adapter.helper.close()

    def test_busy_user_typing_is_retried_like_desktop(self) -> None:
        adapter = self.fx.adapter(busy=True)
        self.shot(adapter)
        out = adapter.call("app_key", {"app": "BenchLab", "combo": "tab"})
        self.assertTrue(json.loads(out[0]["text"])["delivered"])
        self.assertEqual(len(self.fx.requests()), 2)
        adapter.helper.close()

    def test_input_needs_a_screenshot_and_a_point_inside_it(self) -> None:
        adapter = self.fx.adapter()
        with self.assertRaisesRegex(ValueError, "screenshot"):
            adapter.call("app_click", {"app": "BenchLab", "coordinate": [1, 1]})
        adapter.call("app_screenshot", {"app": "BenchLab", "window_id": 77})
        with self.assertRaisesRegex(ValueError, "outside"):
            adapter.call("app_click", {"app": "BenchLab", "coordinate": [99999, 1]})
        with self.assertRaisesRegex(ValueError, "no on-screen window"):
            adapter.call("app_screenshot", {"app": "Nope"})
        self.assertEqual(self.fx.requests(), [])  # nothing reached dispatchRaw
        adapter.helper.close()

    def test_unknown_tool_and_list_windows_filter(self) -> None:
        adapter = self.fx.adapter(["click"])
        self.assertEqual(json.loads(adapter.call("app_list_windows", {"app": "finder"})[0]["text"])[0]["window_id"], 5)
        with self.assertRaises(ValueError):
            adapter.call("app_type", {"app": "BenchLab", "text": "x"})

    def test_mcp_stdio_end_to_end(self) -> None:
        argv = [sys.executable, "-I", "-B", str(ADAPTER), "--helper", str(self.fx.helper), "--winlist",
                str(self.fx.winlist), "--screencapture", str(self.fx.screencapture), "--sips", str(self.fx.sips),
                "--actions", "click,key"]
        msgs = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "app_click", "arguments": {
                "app": "BenchLab", "coordinate": [1, 1]}}},
            {"jsonrpc": "2.0", "id": 4, "method": "nope", "params": {}},
        ]
        done = subprocess.run(argv, input="".join(json.dumps(m) + "\n" for m in msgs), capture_output=True,
                              text=True, timeout=30)
        replies = {r["id"]: r for r in map(json.loads, done.stdout.splitlines())}
        self.assertEqual(replies[1]["result"]["serverInfo"]["name"], "claude-cu-helper")
        self.assertEqual([t["name"] for t in replies[2]["result"]["tools"]],
                         ["app_list_windows", "app_screenshot", "app_click", "app_key"])
        self.assertTrue(replies[3]["result"]["isError"])
        self.assertEqual(replies[4]["error"]["code"], -32601)


class HelperClientTest(unittest.TestCase):
    def test_helper_errors_and_restart(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fx = Fixture(Path(tmp))
            helper = ad.Helper([str(fx.helper)])
            self.assertTrue(helper.call("probe", {})["axTrusted"])
            with self.assertRaisesRegex(ad.HelperError, "nope"):
                helper.call("whatever", {})
            helper.close()
            self.assertTrue(helper.call("probe", {})["skylight"])  # restarted after close
            helper.close()

    def test_preflight_probe_reads_the_helper_directly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fx = Fixture(Path(tmp))
            probe = rb.cu_helper_probe(fx.helper)
            self.assertTrue(probe["axTrusted"])
            missing = Path(tmp) / "missing"
            with self.assertRaises(OSError):
                rb.cu_helper_probe(missing)


if __name__ == "__main__":
    unittest.main()
