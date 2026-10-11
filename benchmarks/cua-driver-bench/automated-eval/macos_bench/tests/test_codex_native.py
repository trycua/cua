"""Amendment 20 (CUA-1347): the native Codex arm (Codex CLI + gpt-6-astra driving Codex computer use)."""

from __future__ import annotations

import json
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import arms  # noqa: E402
import bench_v2 as v2  # noqa: E402
import claude_events  # noqa: E402
import codex_native as cxn  # noqa: E402

ARM_B = {
    "mcpServers": {
        "codex-cu": {
            "type": "stdio",
            "command": "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node",
            "args": [
                "/Applications/ChatGPT.app/Contents/Resources/cua_node/lib/node_modules/@oai/cua-repl/bin/cua-repl.mjs"
            ],
            "env": {
                "CUA_REPL_ENABLED_SURFACES": "computer",
                "CODEX_HOME": "/Users/lume/.codex",
                "X": "1",
            },
        }
    }
}


def write_arm_b(folder: Path, surfaces: str = "computer") -> Path:
    data = json.loads(json.dumps(ARM_B))
    data["mcpServers"]["codex-cu"]["env"]["CUA_REPL_ENABLED_SURFACES"] = surfaces
    path = folder / "mcp.json"
    path.write_text(json.dumps(data), "utf-8")
    return path


class HomeTest(unittest.TestCase):
    def test_config_uses_arm_b_launcher_and_switches_routes_off(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            env = cxn.render_home(
                tmp_path / "home", write_arm_b(tmp_path), cxn.MODEL, cxn.EFFORT, "SYSTEM"
            )
            cfg = tomllib.loads((Path(env["CODEX_HOME"]) / "config.toml").read_text("utf-8"))
        self.assertEqual(cfg["model"], cxn.MODEL)
        self.assertEqual(cfg["model_reasoning_effort"], cxn.EFFORT)
        self.assertEqual(cfg["approval_policy"], "on-request")  # elicitations reach the host
        self.assertEqual(cfg["sandbox_mode"], "read-only")
        self.assertEqual(cfg["developer_instructions"], "SYSTEM")
        for feature in (
            "shell_tool",
            "unified_exec",
            "browser_use",
            "in_app_browser",
            "multi_agent",
            "plugins",
        ):
            self.assertIs(cfg["features"][feature], False, feature)
        server = cfg["mcp_servers"]["cua_repl"]
        self.assertEqual(server["command"], ARM_B["mcpServers"]["codex-cu"]["command"])
        self.assertEqual(server["args"], ARM_B["mcpServers"]["codex-cu"]["args"])
        self.assertEqual(server["enabled_tools"], ["js", "js_reset"])
        self.assertEqual(server["env"]["CUA_REPL_ENABLED_SURFACES"], "computer")
        self.assertEqual(server["env"]["CODEX_HOME"], "/Users/lume/.codex")  # unchanged from arm B
        self.assertEqual(server["env"]["X"], "1")
        self.assertEqual(env["PATH"], cxn.CLOSED_PATH)

    def test_browser_surface_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                cxn.server_entry(write_arm_b(Path(tmp), "browser,computer"))

    def test_arm_is_registered(self) -> None:
        self.assertIn("cx-codex-cu", arms.ALL_ARMS)
        self.assertEqual(arms.NATIVE_CODEX_ARMS, cxn.ARMS)


def item(method: str, **fields: object) -> tuple[str, dict]:
    return method, {"item": fields, "threadId": "t", "turnId": "u"}


def usage(total_in: int, cached: int, out: int) -> tuple[str, dict]:
    return "thread/tokenUsage/updated", {
        "tokenUsage": {
            "total": {
                "inputTokens": total_in,
                "cachedInputTokens": cached,
                "outputTokens": out,
                "reasoningOutputTokens": 0,
            }
        }
    }



class CuaDriverUnderCodexTest(unittest.TestCase):
    """Amendment 23: Cua Driver's MCP server under the same Codex host."""

    def test_cua_server_has_no_tool_filter_and_no_surface_check(self) -> None:
        cua = {"mcpServers": {"cua": {"type": "stdio", "command": "/w/cua-driver", "args": ["--socket", "/tmp/s", "mcp"],
                                      "env": {"CUA_DRIVER_HOME": "/w/h"}}}}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "mcp.json"
            path.write_text(json.dumps(cua), "utf-8")
            env = cxn.render_home(Path(tmp) / "home", path, cxn.MODEL, cxn.EFFORT, "SYSTEM", "cx-cua-driver")
            cfg = tomllib.loads((Path(env["CODEX_HOME"]) / "config.toml").read_text("utf-8"))
        server = cfg["mcp_servers"]["cua"]
        self.assertEqual(server["args"], ["--socket", "/tmp/s", "mcp"])
        self.assertNotIn("enabled_tools", server)
        self.assertEqual(server["env"], {"CUA_DRIVER_HOME": "/w/h"})
        self.assertNotIn("cua_repl", cfg["mcp_servers"])

    def test_registry(self) -> None:
        self.assertEqual(cxn.SOURCE_ARM["cx-cua-driver"], "cc-cua-driver-script")
        self.assertIn("cx-cua-driver", arms.ALL_ARMS)
        self.assertEqual(cxn.MODEL, "gpt-6-astra")  # until gpt-6.1-sol is served (A23.1)
        self.assertEqual(cxn.EFFORT, "medium")

    def test_translator_names_the_cua_server(self) -> None:
        out = cxn.Translator("cua").feed("thread/started", {"thread": {"id": "t"}})
        self.assertEqual(out[0]["mcp_servers"], [{"name": "cua", "status": "connected"}])


class TranslatorTest(unittest.TestCase):
    def run_events(self, events: list[tuple[str, dict]]) -> tuple[list[dict], cxn.Translator]:
        tr = cxn.Translator()
        out: list[dict] = []
        for method, params in events:
            out += tr.feed(method, params)
        out.append(tr.result(timed_out=False, capped=False, wall_ms=1000))
        return out, tr

    def test_mcp_calls_become_tool_uses_and_results(self) -> None:
        call = dict(
            id="exec-1",
            type="mcpToolCall",
            server="cua_repl",
            tool="js",
            arguments={"code": "await app.click(1)"},
        )
        out, tr = self.run_events(
            [
                ("thread/started", {"thread": {"id": "t"}}),
                usage(50, 0, 3),
                item("item/started", **call, status="inProgress"),
                item("item/completed", **call, status="completed", error=None),
                usage(100, 40, 7),
                item(
                    "item/completed", id="m", type="agentMessage", text="DONE", phase="final_answer"
                ),
                ("turn/completed", {"turn": {"status": "completed", "error": None}}),
            ]
        )
        summary = claude_events.summarize(out)
        self.assertEqual(summary["tool_calls"]["total"], 1)
        self.assertEqual(summary["tool_calls"]["by_name"], {"mcp__cua_repl__js": 1})
        self.assertEqual(summary["tool_calls"]["by_class"], {"click": 1})
        self.assertEqual(summary["final_text"], "DONE")
        self.assertEqual(
            summary["tokens"], {"input": 60, "output": 7, "cache_read": 40, "cache_write": 0}
        )
        self.assertEqual(summary["result"]["num_turns"], 2)
        self.assertFalse(summary["result"]["is_error"])
        self.assertTrue(summary["has_init"])
        self.assertEqual(v2.scan_events(out), ([], []))

    def test_non_gui_items_are_violations(self) -> None:
        out, tr = self.run_events(
            [
                item("item/started", id="a", type="commandExecution", command="ls"),
                item("item/completed", id="b", type="fileChange", changes=[{"path": "x"}]),
                item("item/started", id="c", type="collabAgentToolCall", tool="spawn_agent"),
                item("item/completed", id="d", type="imageView", path="/tmp/x.png"),
                item("item/completed", id="e", type="reasoning"),
            ]
        )
        hard, _ = v2.scan_events(out)
        self.assertEqual(hard, ["builtin:Agent", "builtin:Bash", "builtin:Edit", "builtin:Task"])
        self.assertEqual(
            tr.non_gui, ["commandExecution", "fileChange", "collabAgentToolCall", "imageView"]
        )

    def test_code_in_js_is_scanned_like_arm_b(self) -> None:
        call = dict(
            id="i",
            type="mcpToolCall",
            server="cua_repl",
            tool="js",
            arguments={"code": "const cp = await import('node:child_process')"},
        )
        out, _ = self.run_events([item("item/started", **call)])
        hard, _ = v2.scan_events(out)
        self.assertTrue(any("child_process" in h for h in hard), hard)

    def test_failed_call_and_errors(self) -> None:
        call = dict(
            id="i", type="mcpToolCall", server="cua_repl", tool="js", arguments={"code": "x"}
        )
        out, _ = self.run_events(
            [
                ("thread/started", {"thread": {"id": "t"}}),
                item("item/completed", **call, status="failed", error={"message": "boom"}),
                ("error", {"error": {"message": "unexpected status 429 Too Many Requests"}}),
                ("turn/completed", {"turn": {"status": "failed", "error": {"message": "429"}}}),
            ]
        )
        summary = claude_events.summarize(out)
        self.assertEqual(summary["tool_calls"]["failed"], 1)
        self.assertTrue(summary["result"]["is_error"])
        self.assertEqual(summary["result"]["api_error_status"], 429)
        self.assertEqual(claude_events.classify_failure(summary)["kind"], "rate_limit")

    def test_turn_cap_result(self) -> None:
        result = cxn.Translator().result(timed_out=False, capped=True, wall_ms=1)
        self.assertEqual(result["subtype"], "error_max_turns")
        self.assertTrue(result["is_error"])


class GuardTest(unittest.TestCase):
    def test_guard_reads_the_translated_stream_live(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            stream = Path(tmp) / "s.tsv"
            lines = cxn.Translator().feed(
                *item("item/started", id="a", type="commandExecution", command="osascript -e 1")
            )
            stream.write_text("".join(f"1.0\t{json.dumps(e)}\n" for e in lines), "utf-8")
            guard = v2.GuiOnlyGuard(stream, lambda: "com.example.app", pgrep=lambda _n: False)
            guard.sample()
            self.assertTrue(guard.stop.is_set())
            self.assertEqual(guard.violations[0]["detail"], "builtin:Bash")


if __name__ == "__main__":
    unittest.main()
