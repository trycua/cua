# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Flow tests for run_bench.execute_trial and the gates, with run_attempt and the pauses faked: retries,
backoff, exclusion, the quota stop rule, the wall-clock cutoff, the optional budget cap, resume and
finalize. No GUI, no model."""

from __future__ import annotations

import argparse
import base64
import os
import contextlib
import json
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import bench_core as core  # noqa: E402
import run_bench as rb  # noqa: E402

ENTRY = {
    "trial_id": "MB-01-r1-cc-cua-driver",
    "phase": 1,
    "block": "P1-MB-01",
    "run_index": 0,
    "run": 1,
    "task": "MB-01",
    "arm": "cc-cua-driver",
    "task_pos": 0,
    "arm_slot": 0,
    "first_arm": True,
    "schedule_seed": 1,
    "order_index": 0,
}


def make_ctx(tmp: str, **overrides: object) -> rb.Ctx:
    args = argparse.Namespace(
        ledger_who="test",
        model="m",
        max_budget_usd=6.0,
        budget_cap_usd=None,
        seven_day_stop=0.95,
        smoke=False,
        phase1_runs=1,
        phase2_runs=0,
        schedule_seed=1,
        build_dir=None,
        offline=False,
        disk_floor_gb=rb.DISK_FLOOR_GB,
    )
    for key, value in overrides.items():
        setattr(args, key, value)
    run_dir = Path(tmp) / "run"
    run_dir.mkdir()
    ctx = rb.Ctx(
        args,
        run_dir,
        core.Ledger(Path(tmp) / "spend.jsonl"),
        {},
        ["MB-01"],
        ["cc-cua-driver", "cc-codex-cu"],
        full_task_ids=["MB-01"],
    )
    return ctx


def test_browser_cli_auth_rejected_before_provisioning(tmp_path):
    import pytest

    args = rb.build_parser().parse_args(["run", "--only-arm", "cx-browser-use-cli"])
    args.runs_dir = tmp_path
    args.ledger = tmp_path / "ledger.jsonl"
    with (
        mock.patch.dict("os.environ", {"CDB_CODEX_AUTH": "gateway"}),
        mock.patch.object(rb.subprocess, "run", side_effect=AssertionError("subprocess")),
        mock.patch.object(
            rb.arms, "link_chatgpt_auth", side_effect=AssertionError("credential link")
        ),
    ):
        with pytest.raises((ValueError, SystemExit), match="chatgpt"):
            rb.make_ctx(args)


@pytest.mark.skipif(sys.platform != "darwin", reason="browser CLI arm is macOS-only")
def test_browser_cli_run_fails_closed(tmp_path):
    with tempfile.TemporaryDirectory() as tmp:
        ctx = make_ctx(tmp, no_model_preflight=True, build_dir=None)
        ctx.args = rb.build_parser().parse_args(["run", "--only-arm", "cx-browser-use-cli"])
        ctx.arm_names = ["cx-browser-use-cli"]
        with (
            mock.patch.object(rb.ca, "observed_pins", return_value={}),
            mock.patch.object(rb.ca, "cua_version_string", return_value=None),
            mock.patch.object(rb.ca, "CUA_BUILDS", {}),
        ):
            rb.setup_arms(ctx)
        assert ctx.mcp == {}
        for offline in (False, True):
            for smoke in (False, True):
                ctx.args.offline, ctx.args.smoke = offline, smoke
                assert any(
                    status == "fail" and "not yet implemented" in detail
                    for _, status, detail in rb.preflight(ctx, with_models=False)
                )
        args = ctx.args
        args.no_model_preflight = True
        with contextlib.ExitStack() as stack:
            for name, value in (
                ("cmd_gate_check", 0),
                ("make_ctx", ctx),
                ("setup_arms", None),
                ("start_heartbeat", mock.Mock()),
                ("unregister_stale_copies", []),
                ("stop_agent_daemons", None),
                ("start_recorder", None),
                ("shutdown", None),
            ):
                stack.enter_context(mock.patch.object(rb, name, return_value=value))
            stack.enter_context(mock.patch.object(rb.ca, "stop_cua_daemon"))
            loop = stack.enter_context(
                mock.patch.object(rb, "run_loop", side_effect=AssertionError("trial started"))
            )
            assert rb.cmd_run(args) == 2
            assert (ctx.run_dir / "PREFLIGHT_FAILED").is_file()
            loop.assert_not_called()


class CodexAuthPreflightTest(unittest.TestCase):
    def test_chatgpt_checks_metadata_not_gateway_or_model_and_skips_offline(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            arm = "cx-codex-cu"
            mcp = Path(tmp) / "mcp.json"
            mcp.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "sky": {
                                "command": "node",
                                "env": {"CUA_REPL_ENABLED_SURFACES": "computer"},
                            }
                        }
                    }
                )
            )
            ctx.mcp[arm] = (mcp, "cua_repl")
            source = Path(tmp) / "auth.json"
            source.touch()
            with (
                mock.patch.dict(
                    "os.environ", {"CDB_CODEX_AUTH": "chatgpt", "CDB_CODEX_AUTH_FILE": str(source)}
                ),
                mock.patch(
                    "urllib.request.urlopen", side_effect=AssertionError("gateway called")
                ) as gateway,
                mock.patch.object(
                    rb.cxn, "run_codex", side_effect=AssertionError("model called")
                ) as model,
                mock.patch.object(
                    rb.cxn, "chatgpt_checks", return_value=[("metadata", "pass", "ok")]
                ) as probe,
            ):
                checks = rb.native_codex_checks(ctx, arm, with_models=True)
                self.assertIn(("metadata", "pass", "ok"), checks)
                env = probe.call_args.args[1]
                self.assertFalse((Path(env["CODEX_HOME"]) / "auth.json").is_symlink())
                probe.reset_mock()
                rb.native_codex_checks(ctx, "cx-cua-driver", with_models=False)
                probe.assert_not_called()
                ctx.versions.pop("codex_auth_checks")
                rb.native_codex_checks(ctx, arm, with_models=False)
                probe.assert_called_once()
                probe.reset_mock()
                ctx.args.offline = True
                rb.native_codex_checks(ctx, arm, with_models=False)
                probe.assert_not_called()
                gateway.assert_not_called()
                model.assert_not_called()
                ctx.args.offline = False
                ctx.versions.pop("codex_auth_checks")

                def replace_credentials(cli, env, model):
                    link = Path(env["CODEX_HOME"]) / "auth.json"
                    link.unlink()
                    link.write_text("FAKE_TOKEN")
                    return [("metadata", "pass", "ok")]

                probe.side_effect = replace_credentials
                checks = rb.native_codex_checks(ctx, arm, with_models=False)
                self.assertTrue(any(status == "fail" for _, status, _ in checks))
                for secret in (str(source), "FAKE_TOKEN", rb.cxn.gateway()):
                    self.assertNotIn(secret, json.dumps(checks))

    def test_native_trial_removes_auth_link_and_flags_replacement(self) -> None:
        for replaced in (False, True, "setup"):
            with tempfile.TemporaryDirectory() as tmp, contextlib.ExitStack() as stack:
                ctx = make_ctx(
                    tmp,
                    timeout_s=60,
                    max_turns=3,
                    effort="low",
                    idle_min=0,
                    no_sentinel=True,
                    build_dir=tmp,
                    check_occlusion=False,
                    tell_budget=False,
                )
                ctx.tasks["MB-01"] = argparse.Namespace(id="MB-01", tags=[], timeout_s=60)
                arm = "cx-codex-cu"
                entry = {**ENTRY, "arm": arm}
                source = Path(tmp) / "shared-auth.json"
                source.write_text("FAKE_TOKEN")
                mcp = Path(tmp) / "mcp.json"
                mcp.write_text(
                    json.dumps(
                        {
                            "mcpServers": {
                                "sky": {
                                    "command": "node",
                                    "env": {"CUA_REPL_ENABLED_SURFACES": "computer"},
                                }
                            }
                        }
                    )
                )
                ctx.mcp[arm] = (mcp, "cua_repl")
                stack.enter_context(
                    mock.patch.dict(
                        "os.environ",
                        {
                            "CDB_CODEX_AUTH": "chatgpt",
                            "CDB_CODEX_AUTH_FILE": str(source),
                            "DISPLAY": os.environ.get("DISPLAY") or ":99",
                        },
                    )
                )
                ctx.versions["codex_native"] = rb.cxn.observed(Path("/nonexistent"))
                for owner, name, result in (
                    (rb.desktop, "capture_screenshot", None),
                    (rb, "task_spec", {}),
                    (rb, "kill_bench_apps", None),
                    (rb, "reset_needs_apps", None),
                    (rb.subprocess, "run", None),
                    (rb.pilot, "hid_idle_seconds", 1000),
                    (rb.pilot, "sweep_processes", 0),
                    (rb.pilot, "prepare_probe", ("BRIEF", {})),
                    (rb.pilot, "launch_lab", None),
                    (rb.ca, "prepare_cwd", Path(tmp)),
                    (rb, "apps_running", {}),
                    (rb, "evaluate_probe", {"passed": True}),
                    (rb, "FrontWatcher", None),
                ):
                    stack.enter_context(mock.patch.object(owner, name, return_value=result))
                sentinel = stack.enter_context(mock.patch.object(rb.pilot, "Sentinel"))
                sentinel.return_value.enabled = False
                sentinel.return_value.summary.return_value = {}
                stack.enter_context(mock.patch.object(rb.pilot, "IdleSampler"))

                def run_codex(cmd, env, *args, **kwargs):
                    link = Path(env["CODEX_HOME"]) / "auth.json"
                    self.assertTrue(link.is_symlink())
                    if replaced is True:
                        link.unlink()
                        link.write_text("FAKE_TOKEN")
                    if replaced == "setup":
                        events = [
                            {"type": "system", "subtype": "init"},
                            {
                                "type": "user",
                                "message": {
                                    "content": [
                                        {
                                            "type": "tool_result",
                                            "is_error": True,
                                            "content": "screen_recording_permission_denied",
                                        }
                                    ]
                                },
                            },
                            {"type": "result", "result": "DONE"},
                        ]
                        args[5].write_text("\n".join(json.dumps(e) for e in events))
                    return {"returncode": 0, "result_seen": True, "wall_s": 0}

                stack.enter_context(mock.patch.object(rb.cxn, "run_codex", side_effect=run_codex))
                row = rb.run_attempt(ctx, entry, 1)
                self.assertIn("infra_pattern", row)
                if replaced == "setup":
                    self.assertEqual(row["infra_pattern"], "screen_recording")
                    self.assertEqual(row["infra_failure"], "tool_setup")
                    self.assertFalse(row["passed"])
                    self.assertTrue(row["excluded"])
                else:
                    self.assertIsNone(row["infra_pattern"])
                self.assertEqual(row["infra_failure"] == "harness_exception", replaced is True)
                if replaced is True:
                    self.assertIn("symlink replaced", row["infra_message"])
                link = ctx.run_dir / "trials" / entry["trial_id"] / "a1/codex-home/.codex/auth.json"
                self.assertFalse(link.is_symlink())
                self.assertEqual(row["codex_native"]["codex_auth"], "chatgpt")
                saved = link.parents[2] / "trial.json"
                for secret in (str(source), "FAKE_TOKEN", rb.cxn.gateway()):
                    self.assertNotIn(secret, saved.read_text())

    def test_gateway_retains_model_listing_even_offline(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            with (
                mock.patch.dict("os.environ", {"CDB_CODEX_AUTH": "gateway"}),
                mock.patch("urllib.request.urlopen") as fetch,
            ):
                fetch.return_value.__enter__.return_value.read.return_value = json.dumps(
                    {"data": [{"id": rb.cxn.MODEL}]}
                ).encode()
                ctx.args.offline = True
                checks = rb.native_codex_checks(ctx, "cx-codex-cu", with_models=False)
                self.assertIn(
                    "gateway lists the model",
                    [label for label, status, _ in checks if status == "pass"],
                )
                fetch.assert_called_once()

    def linux_evidence_trial(self, *, replace_link: bool, screenshot_error, copy_error=None):
        with tempfile.TemporaryDirectory() as tmp, contextlib.ExitStack() as stack:
            ctx = make_ctx(
                tmp,
                timeout_s=60,
                max_turns=3,
                effort="low",
                idle_min=0,
                no_sentinel=True,
                build_dir=tmp,
                check_occlusion=False,
                tell_budget=False,
            )
            ctx.tasks["MB-01"] = argparse.Namespace(id="MB-01", tags=[], timeout_s=60)
            arm = "cx-codex-cu"
            entry = {**ENTRY, "arm": arm}
            source = Path(tmp) / "shared-auth.json"
            source.write_text("FAKE_TOKEN")
            mcp = Path(tmp) / "mcp.json"
            mcp.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "sky": {
                                "command": "node",
                                "env": {"CUA_REPL_ENABLED_SURFACES": "computer"},
                            }
                        }
                    }
                )
            )
            ctx.mcp[arm] = (mcp, "cua_repl")
            stack.enter_context(
                mock.patch.dict(
                    "os.environ",
                    {
                        "CDB_CODEX_AUTH": "chatgpt",
                        "CDB_CODEX_AUTH_FILE": str(source),
                        "DISPLAY": ":99",
                    },
                )
            )
            ctx.versions["codex_native"] = rb.cxn.observed(Path("/nonexistent"))
            mocks = {}
            for owner, name, result in (
                (rb.desktop, "detect_platform", "linux"),
                (rb, "task_spec", {}),
                (rb, "kill_bench_apps", None),
                (rb, "reset_needs_apps", None),
                (rb.subprocess, "run", None),
                (rb.pilot, "hid_idle_seconds", 1000),
                (rb.pilot, "sweep_processes", 0),
                (rb.pilot, "prepare_probe", ("BRIEF", {})),
                (rb.pilot, "launch_lab", None),
                (rb.ca, "prepare_cwd", Path(tmp)),
                (rb, "apps_running", {}),
                (rb, "evaluate_probe", {"passed": True}),
                (rb, "FrontWatcher", None),
            ):
                mocks[name] = stack.enter_context(
                    mock.patch.object(owner, name, return_value=result)
                )
            sentinel = stack.enter_context(mock.patch.object(rb.pilot, "Sentinel"))
            sentinel.return_value.enabled = False
            sentinel.return_value.summary.return_value = {}
            stack.enter_context(mock.patch.object(rb.pilot, "IdleSampler"))

            def capture(display, path):
                if path.name == "startup.png":
                    return None
                if screenshot_error is not None:
                    raise screenshot_error

            stack.enter_context(
                mock.patch.object(rb.desktop, "capture_screenshot", side_effect=capture)
            )
            log = Path(tmp) / "recorder.log"
            log.write_text("log")
            stack.enter_context(mock.patch.object(rb.ca, "RECORDER_STATE", Path(tmp)))
            Path(tmp, "daemon.log").write_text("log")
            if copy_error is not None:
                stack.enter_context(
                    mock.patch.object(rb.shutil, "copyfile", side_effect=copy_error)
                )

            def run_codex(cmd, env, *args, **kwargs):
                link = Path(env["CODEX_HOME"]) / "auth.json"
                self.assertTrue(link.is_symlink())
                if replace_link:
                    link.unlink()
                    link.write_text("FAKE_TOKEN")
                return {"returncode": 0, "result_seen": True, "wall_s": 0}

            stack.enter_context(mock.patch.object(rb.cxn, "run_codex", side_effect=run_codex))
            row = rb.run_attempt(ctx, entry, 1)
            trial = ctx.run_dir / "trials" / entry["trial_id"] / "a1"
            link = trial / "codex-home/.codex/auth.json"
            self.assertFalse(link.is_symlink())
            self.assertEqual(source.read_text(), "FAKE_TOKEN")
            mocks["sweep_processes"].assert_called()
            mocks["kill_bench_apps"].assert_called()
            errors = trial / "artifacts" / "screenshot-error.txt"
            return row, (errors.read_text() if errors.is_file() else None)

    def test_linux_screenshot_failure_still_unlinks_auth(self) -> None:
        row, errors = self.linux_evidence_trial(
            replace_link=False, screenshot_error=AttributeError("secret /path")
        )
        self.assertNotEqual(row["infra_failure"], "harness_exception")
        self.assertEqual(errors, "screenshot: AttributeError")

    def test_linux_log_copy_failure_still_unlinks_auth(self) -> None:
        row, errors = self.linux_evidence_trial(
            replace_link=False, screenshot_error=None, copy_error=OSError("secret /path")
        )
        self.assertNotEqual(row["infra_failure"], "harness_exception")
        self.assertIn("recorder log: OSError", errors)
        self.assertNotIn("secret", errors)

    def test_linux_evidence_failure_keeps_credential_integrity_error(self) -> None:
        row, _ = self.linux_evidence_trial(
            replace_link=True, screenshot_error=AttributeError("boom")
        )
        self.assertEqual(row["infra_failure"], "harness_exception")
        self.assertIn("shared ChatGPT credential symlink replaced", row["infra_message"])


def attempt_row(
    attempt: int,
    infra: str | None = None,
    cost: float = 0.5,
    quota7: float | None = 0.9,
    **extra: object,
) -> dict:
    row = {
        "trial_id": ENTRY["trial_id"],
        "attempt": attempt,
        "final": False,
        "smoke": False,
        "arm": ENTRY["arm"],
        "status": "infra_error" if infra else "completed",
        "passed": not infra,
        "wall_s": 1.0,
        "turns": 3,
        "cost_usd": cost,
        "infra_failure": infra,
        "infra_message": infra or "",
        "excluded": bool(infra),
        "quota_seven_day_after": quota7,
    }
    row.update(extra)
    return row


class RealCaptureTest(unittest.TestCase):
    def test_each_family_validates_capture_bytes_and_closes_client(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ffmpeg = Path(tmp) / "ffmpeg"
            ffmpeg.write_text(
                '#!/usr/bin/env python3\nimport sys\nfrom pathlib import Path\nsys.stdout.buffer.write(Path(sys.argv[sys.argv.index("-i")+1]).read_bytes())\n'
            )
            ffmpeg.chmod(0o755)
            image = b"P6\n2 1\n255\n" + b"\x00\x01\x02\xff\xfe\xfd"
            capture_detail = rb.rec.capture_detail
            families = [
                "cc-cua-driver",
                "cc-codex-cu",
                "cc-claude-cu-helper",
                "cx-cua-driver",
                "cx-codex-cu",
                "cx-claude-cu-helper",
            ]
            for arm in families:
                source = rb.cxn.SOURCE_ARM.get(arm, arm)
                tools = [
                    {"name": n}
                    for n in ["list_apps", "get_desktop_state", "js", "js_reset", "run_script"]
                    + [f"tool{i}" for i in range(50)]
                ]
                if source in rb.ca.CU_HELPER_ARMS:
                    tools = [{"name": n} for n in rb.ca.load_pins()["claude_cu_helper"]["tools"]]
                elif (
                    source in rb.ca.CUA_ARMS
                    and rb.ca.SCRIPT_FLAG not in rb.ca.CUA_BUILDS[source].daemon_env
                ):
                    tools = [t for t in tools if t["name"] != "run_script"]
                ctx.mcp[arm] = (Path(tmp) / f"{arm}.json", "server")
                for payload, message in [
                    (image, None),
                    ("file", None),
                    ("legacy", None),
                    (b"", "no image block"),
                    (b"garbage", "decode failed"),
                    (b"P6\n2 1\n255\n" + b"\x00" * 6, "blank image"),
                    (b"P6\n2 1\n255\n" + b"\xff" * 6, "blank image"),
                    (None, "no image block"),
                    ("error", "could not create image from window"),
                    ("rpc", "permission denied"),
                    ("windows", "no layer-0 Calculator window"),
                ]:
                    if payload == "windows" and source not in rb.ca.CU_HELPER_ARMS:
                        continue
                    if payload in ("file", "legacy") and source not in rb.ca.CUA_ARMS:
                        continue
                    advertised = [
                        {
                            **t,
                            "inputSchema": {
                                "properties": {"screenshot_out_file": {"type": "string"}}
                            },
                        }
                        if t["name"] == "get_desktop_state" and payload == "file"
                        else {"name": "screenshot"}
                        if t["name"] == "get_desktop_state" and payload == "legacy"
                        else t
                        for t in tools
                    ]
                    calls = []
                    client = mock.Mock()

                    def request(method, params, **kwargs):
                        calls.append((method, params))
                        if method == "tools/list":
                            return {"result": {"tools": advertised}}
                        if params["name"] == "check_permissions":
                            return {"result": {"content": []}}
                        if params["name"] == "app_list_windows":
                            return {
                                "result": {
                                    "content": [
                                        {
                                            "type": "text",
                                            "text": json.dumps(
                                                []
                                                if payload == "windows"
                                                else [{"app": "Calculator", "window_id": 7}]
                                            ),
                                        }
                                    ]
                                }
                            }
                        if payload == "error":
                            return {
                                "result": {
                                    "isError": True,
                                    "content": [
                                        {
                                            "type": "text",
                                            "text": "could not create image from window",
                                        }
                                    ],
                                }
                            }
                        if payload == "rpc":
                            return {"error": {"message": "permission denied"}}
                        if payload == "file":
                            Path(params["arguments"]["screenshot_out_file"]).write_bytes(image)
                            return {"result": {"content": None}}
                        return {
                            "result": {
                                "content": None
                                if payload is None
                                else [
                                    {
                                        "type": "image",
                                        "data": base64.b64encode(
                                            image if payload == "legacy" else payload
                                        ).decode(),
                                    }
                                ]
                            }
                        }

                    client.request.side_effect = request
                    with (
                        self.subTest(arm=arm, message=message),
                        mock.patch.object(rb.rec, "McpStdioClient", return_value=client),
                        mock.patch.object(rb, "ensure_agent_daemon") as daemon,
                        mock.patch.object(rb, "mcp_entry", return_value=("mcp", [], {})),
                        mock.patch.object(
                            rb.cxn,
                            "server_entry",
                            return_value={"command": "native", "args": [], "env": {}},
                        ) as native,
                        mock.patch.object(
                            rb.rec,
                            "capture_detail",
                            side_effect=lambda path: capture_detail(path, str(ffmpeg)),
                        ),
                    ):
                        name, status, detail = rb.real_capture_check(ctx, arm)
                        self.assertEqual(name, f"{arm} real capture")
                        self.assertEqual(status, "fail" if message else "pass")
                        self.assertIn(message or "2x1 bytes=", detail)
                        self.assertLess(len(detail), 500)
                        client.close.assert_called_once()
                        if arm in rb.cxn.ARMS:
                            native.assert_called_once_with(ctx.mcp[arm][0], arm)
                        if source in rb.ca.CUA_ARMS:
                            daemon.assert_called_once_with(ctx, source)
                            self.assertEqual(
                                [p["name"] for m, p in calls if m == "tools/call"],
                                [
                                    "check_permissions",
                                    "screenshot" if payload == "legacy" else "get_desktop_state",
                                ],
                            )
                        if source == "cc-codex-cu":
                            code = calls[-1][1]["arguments"]["code"]
                            self.assertIn('cua.getApp("Calculator")', code)
                            self.assertIn("await app.getScreenshot()", code)
                            self.assertNotIn("emitImage", code)
                            self.assertNotIn(".screenshot()", code)
            with self.assertRaises(FileNotFoundError):
                capture_detail(Path(tmp) / "missing.png", str(Path(tmp) / "missing-ffmpeg"))

    def test_stdio_client_answers_calculator_elicitation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            script = Path(tmp) / "server.py"
            script.write_text("""import json, sys
for line in sys.stdin:
    req = json.loads(line)
    if req.get("method") == "initialize":
        assert req["params"]["capabilities"] == {"elicitation": {}}
        print(json.dumps({"id": req["id"], "result": {}}), flush=True)
    elif req.get("method") == "tools/call":
        pending = req["id"]
        print(json.dumps({"id": "approval", "method": "elicitation/create", "params": {"message": 'Allow Computer Use to use "Calculator"?'}}), flush=True)
    elif req.get("id") == "approval":
        print(json.dumps({"id": pending, "result": req["result"]}), flush=True)
""")
            client = rb.rec.McpStdioClient(
                [sys.executable, str(script)],
                dict(os.environ),
                elicitation=lambda text: rb.claude_driver.answer_elicitation(text, {"calculator"}),
            )
            try:
                reply = client.request("tools/call", {"name": "js", "arguments": {}}, timeout=3)
                self.assertEqual(reply["result"]["action"], "accept")
            finally:
                client.close()


class PreflightTest(unittest.TestCase):
    def test_offline_captures_every_native_arm_without_models(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp, offline=True, allow_non_terminal=True)
            ctx.task_ids = []
            cli = Path(tmp) / "codex"
            cli.touch()
            with (
                mock.patch.object(rb, "terminal_ancestor", return_value=True),
                mock.patch.object(rb.rec, "free_gb", return_value=1000),
                mock.patch.object(rb, "check_pins", return_value=[]),
                mock.patch.object(rb, "registered_bench_copies", return_value=[]),
                mock.patch.object(
                    rb.subprocess, "run", return_value=mock.Mock(returncode=1, stdout="")
                ),
                mock.patch.object(rb.cxn, "CODEX_CLI", cli),
                mock.patch.object(
                    rb.cxn,
                    "server_entry",
                    return_value={"command": "mcp", "args": [], "env": {}},
                ),
                mock.patch.object(rb.ca, "cu_helper_observed", return_value={}),
                mock.patch.object(rb.ca, "check_cu_helper_pins", return_value=[]),
                mock.patch.object(rb, "cu_helper_live_checks", return_value=[]),
                mock.patch.object(rb, "native_codex_checks", return_value=[]) as native,
                mock.patch.object(
                    rb,
                    "real_capture_check",
                    side_effect=lambda ctx, arm: (
                        f"{arm} real capture",
                        "fail" if arm == rb.cxn.ARMS[0] else "pass",
                        "capture",
                    ),
                ) as capture,
                mock.patch.object(rb, "reset_needs_apps") as reset,
                mock.patch.object(rb, "capture_subject", contextlib.nullcontext),
                mock.patch.object(rb, "mcp_list_tools") as probe,
                mock.patch.object(rb, "ensure_agent_daemon") as daemon,
            ):
                for arm in rb.cxn.MCP_ARMS:
                    with self.subTest(arm=arm):
                        ctx.arm_names = [arm]
                        ctx.mcp[arm] = (Path(tmp) / "mcp.json", rb.cxn.SERVER_OF[arm])
                        ctx.args.offline = True
                        probe.reset_mock()
                        daemon.reset_mock()
                        capture.reset_mock()
                        rows = rb.preflight(ctx)
                        probe.assert_not_called()
                        daemon.assert_not_called()
                        native.assert_called_with(ctx, arm, False)
                        capture.assert_called_once_with(ctx, arm)
                        self.assertTrue(any(name == f"{arm} real capture" for name, _, _ in rows))
                        reset.assert_called_with(["Calculator"])
                        ctx.args.offline = False
                        probe.side_effect = None
                        probe.return_value = [
                            "js",
                            "js_reset",
                            "list_apps",
                            "run_script",
                        ] + rb.ca.load_pins()["claude_cu_helper"]["tools"]
                        daemon.side_effect = None
                        rows = rb.preflight(ctx)
                        self.assertEqual(capture.call_count, 2)
                ctx.arm_names = list(rb.cxn.MCP_ARMS)
                ctx.args.offline = True
                capture.reset_mock()
                rows = rb.preflight(ctx)
                self.assertEqual(capture.call_count, len(ctx.arm_names))
                self.assertEqual(
                    len([r for r in rows if r[0].endswith(" real capture")]), len(ctx.arm_names)
                )
                ctx.recorder = mock.Mock(name="recording")
                raw = Path(tmp) / "video.mov"
                raw.write_bytes(b"video")
                ctx.recorder.stop.return_value = raw
                for blank in (True, False):
                    with contextlib.ExitStack() as patches:
                        patches.enter_context(mock.patch.object(rb.time, "sleep"))
                        patches.enter_context(
                            mock.patch.object(rb.rec, "ffprobe_duration", return_value=3)
                        )
                        patches.enter_context(
                            mock.patch.object(
                                rb.rec,
                                "capture_detail",
                                side_effect=RuntimeError("blank image") if blank else None,
                                return_value=f"2x1 bytes=5 path={raw}",
                            )
                        )
                        rows = rb.preflight(ctx)
                    recording = next(r for r in rows if r[0].startswith("recorder real capture"))
                    self.assertEqual(recording[1], "fail" if blank else "pass")


class ExecuteTrialTest(unittest.TestCase):
    def setUp(self) -> None:
        disk = mock.patch.object(rb.rec, "free_gb", return_value=100.0)
        disk.start()
        self.addCleanup(disk.stop)

    def test_daemon_setup_failure_retries_with_restart_then_excludes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            rows = [
                attempt_row(i, "tool_setup", infra_pattern="daemon_not_running") for i in (1, 2)
            ]
            with (
                mock.patch.object(rb.rec, "free_gb", return_value=1000),
                mock.patch.object(rb, "ensure_agent_daemon") as restart,
            ):
                row, pauses, error = self.run_trial(ctx, rows)
            self.assertIsNone(error)
            self.assertEqual(row["infra_pattern"], "daemon_not_running")
            self.assertTrue(row["excluded"])
            self.assertFalse(row["passed"])
            self.assertEqual(row["attempt"], 2)
            self.assertEqual(len(pauses), 1)
            restart.assert_called_once_with(ctx, ENTRY["arm"])

    def run_trial(
        self, ctx: rb.Ctx, rows: list[dict]
    ) -> tuple[dict | None, list[tuple[str, float]], Exception | None]:
        pauses: list[tuple[str, float]] = []
        calls = iter(rows)

        def fake_attempt(_ctx: rb.Ctx, _entry: dict, attempt: int) -> dict:
            row = next(calls)
            row["attempt"] = attempt
            return row

        def fake_pause(
            _ctx: rb.Ctx, kind: str, seconds: float, message: str, trial_id: str | None
        ) -> None:
            pauses.append((kind, seconds))

        with (
            mock.patch.object(rb, "run_attempt", fake_attempt),
            mock.patch.object(rb, "log_pause", fake_pause),
        ):
            try:
                return rb.execute_trial(ctx, dict(ENTRY)), pauses, None
            except rb.StopRun as stop:
                return None, pauses, stop

    def test_clean_trial_is_final_and_written_to_results_and_ledger(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            row, pauses, err = self.run_trial(ctx, [attempt_row(1)])
            self.assertIsNone(err)
            self.assertTrue(row["final"])
            self.assertEqual(pauses, [])
            saved = core.load_rows(ctx.results_path)
            self.assertEqual(len(saved), 1)
            self.assertAlmostEqual(ctx.ledger.cumulative(), 0.5)

    def test_rate_limit_waits_then_retries_the_same_trial_id_without_counting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            rows = [attempt_row(1, "rate_limit"), attempt_row(2, "rate_limit"), attempt_row(3)]
            row, pauses, err = self.run_trial(ctx, rows)
            self.assertIsNone(err)
            self.assertEqual([k for k, _ in pauses], ["retry_rate_limit", "retry_rate_limit"])
            self.assertEqual([s for _, s in pauses], [60.0, 120.0])  # exponential backoff from 60 s
            saved = core.load_rows(ctx.results_path)
            self.assertEqual([r["final"] for r in saved], [False, False, True])
            self.assertEqual({r["trial_id"] for r in saved}, {ENTRY["trial_id"]})
            self.assertEqual(core.completed_trial_ids(saved), {ENTRY["trial_id"]})

    def test_rate_limit_with_reset_time_waits_until_reset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            reset = time.time() + 1800
            _, pauses, _ = self.run_trial(
                ctx, [attempt_row(1, "rate_limit", infra_reset_epoch=reset), attempt_row(2)]
            )
            self.assertAlmostEqual(pauses[0][1], 1830, delta=5)

    def test_seven_day_rejected_stops_instead_of_waiting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.quota = {"status": "rejected", "type": "seven_day", "seven_day": 0.97}
            row, pauses, err = self.run_trial(ctx, [attempt_row(1, "rate_limit")])
            self.assertIsInstance(err, rb.StopRun)
            self.assertEqual(err.reason, "QUOTA")
            self.assertEqual(pauses, [])

    def test_overloaded_retries_three_times_then_excludes_and_moves_on(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            rows = [attempt_row(i, "overloaded") for i in range(1, 5)]
            row, pauses, err = self.run_trial(ctx, rows)
            self.assertIsNone(err)
            self.assertTrue(row["final"])
            self.assertTrue(row["excluded"])
            self.assertEqual([s for _, s in pauses], [60.0, 120.0, 240.0])
            self.assertEqual(len(core.load_rows(ctx.results_path)), 4)

    def test_other_infra_failure_is_retried_once(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            row, pauses, err = self.run_trial(
                ctx, [attempt_row(1, "mcp_start"), attempt_row(2, "mcp_start")]
            )
            self.assertIsNone(err)
            self.assertTrue(row["final"] and row["excluded"])
            self.assertEqual(len(pauses), 1)

    def test_auth_failure_stops_the_run_after_one_retry(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            _, pauses, err = self.run_trial(ctx, [attempt_row(1, "auth"), attempt_row(2, "auth")])
            self.assertIsInstance(err, rb.StopRun)
            self.assertEqual(err.reason, "AUTH")
            self.assertEqual(len(pauses), 1)

    def test_agent_failures_are_not_retried(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            failed = attempt_row(1, None, passed=False, status="timeout")
            row, pauses, err = self.run_trial(ctx, [failed])
            self.assertTrue(row["final"])
            self.assertEqual(pauses, [])
            self.assertFalse(row["passed"])


class GateTest(unittest.TestCase):
    def setUp(self) -> None:
        disk = mock.patch.object(rb.rec, "free_gb", return_value=100.0)
        disk.start()
        self.addCleanup(disk.stop)

    def test_quota_stop_rule_blocks_new_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.quota = {
                "seven_day": 0.95,
                "five_hour": 0.2,
                "status": "allowed_warning",
                "type": "seven_day",
            }
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "QUOTA")
            ctx.quota = {"seven_day": 0.949, "status": "allowed_warning", "type": "seven_day"}
            rb.check_gates(ctx)  # below the line: no exception

    def test_cutoff_blocks_new_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.cutoff = time.time() - 1
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "TIME")

    def test_stop_file_blocks_new_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            (ctx.run_dir / "STOP").write_text("x")
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "USER")

    def test_optional_budget_cap_only_when_set(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.ledger.append("x", "spent", "m", 148.0)
            rb.check_gates(ctx)  # no cap by default
            ctx.args.budget_cap_usd = 150.0
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "BUDGET")

    def test_five_hour_rejected_waits_for_the_reset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.quota = {
                "status": "rejected",
                "type": "five_hour",
                "seven_day": 0.9,
                "five_hour_resets_at": time.time() + 600,
            }
            pauses: list[float] = []
            with mock.patch.object(
                rb, "log_pause", lambda c, kind, seconds, message, trial: pauses.append(seconds)
            ):
                rb.check_gates(ctx)
            self.assertEqual(len(pauses), 1)
            self.assertAlmostEqual(pauses[0], 630, delta=5)

    def test_five_hour_wait_that_passes_the_cutoff_stops(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.cutoff = time.time() + 300
            ctx.quota = {
                "status": "rejected",
                "type": "five_hour",
                "seven_day": 0.9,
                "five_hour_resets_at": time.time() + 3600,
            }
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "TIME")


class FinalizeTest(unittest.TestCase):
    def test_finalize_flags_incomplete_blocks_and_keeps_rows(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp, phase1_runs=1, phase2_runs=0)
            blocks = ctx.blocks()
            entries = core.flat_entries(blocks)
            self.assertEqual(len(entries), 2)
            rows = [
                {
                    "trial_id": entries[0]["trial_id"],
                    "final": True,
                    "smoke": False,
                    "excluded": False,
                    "arm": entries[0]["arm"],
                    "task": "MB-01",
                }
            ]
            ctx.results_path.write_text("".join(json.dumps(r) + "\n" for r in rows))
            summary = rb.finalize(ctx)
            self.assertEqual(summary["blocks_complete"], 0)
            saved = core.load_rows(ctx.results_path)
            self.assertEqual(len(saved), 1)
            self.assertTrue(saved[0]["block_incomplete"])
            rows.append(
                {
                    "trial_id": entries[1]["trial_id"],
                    "final": True,
                    "smoke": False,
                    "excluded": False,
                    "arm": entries[1]["arm"],
                    "task": "MB-01",
                }
            )
            ctx.results_path.write_text("".join(json.dumps(r) + "\n" for r in rows))
            self.assertEqual(rb.finalize(ctx)["blocks_complete"], 1)
            self.assertFalse(any(r["block_incomplete"] for r in core.load_rows(ctx.results_path)))

    def test_resume_skips_final_ids_only(self) -> None:
        rows = [
            {"trial_id": "a", "final": True},
            {"trial_id": "b", "final": False},
            {"trial_id": "c", "smoke": True, "final": True},
        ]
        self.assertEqual(core.completed_trial_ids(rows), {"a"})


class LinuxSetupTest(unittest.TestCase):
    def test_macos_arms_are_skipped_before_setup(self) -> None:
        import contextlib
        import io
        import desktop_runtime as desktop

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        args = rb.build_parser().parse_args(
            [
                "dry-run",
                "--ledger",
                str(Path(tmp.name) / "ledger.jsonl"),
                "--bench-v2",
                "--arms",
                "cc-cua-driver",
                "cc-codex-cu",
                "cc-claude-cu-helper",
            ]
        )
        output = io.StringIO()
        with (
            mock.patch.object(desktop, "detect_platform", return_value="linux"),
            contextlib.redirect_stderr(output),
        ):
            ctx = rb.make_ctx(args)
        self.assertEqual(ctx.arm_names, ["cc-cua-driver"])
        self.assertIn("unsupported-platform: cc-codex-cu requires macOS", output.getvalue())
        self.assertIn(
            "unsupported-platform: cc-claude-cu-helper requires macOS",
            ctx.versions["unsupported_arms"],
        )

    def test_no_supported_arms_fails_clearly(self) -> None:
        import desktop_runtime as desktop

        args = rb.build_parser().parse_args(["dry-run", "--arms", "cc-codex-cu"])
        with (
            mock.patch.object(desktop, "detect_platform", return_value="linux"),
            self.assertRaisesRegex(SystemExit, "no supported arms remain"),
        ):
            rb.make_ctx(args)

    def test_linux_pack_descriptor_selection(self) -> None:
        import cdb_adapter
        import desktop_runtime as desktop

        with mock.patch.object(desktop, "detect_platform", return_value="linux"):
            descriptor = cdb_adapter._descriptor({"pack_task": "shared/cdb-s02"})
        self.assertEqual(descriptor["platform"], "linux")
        self.assertTrue(any("libreoffice" in " ".join(a["command"]) for a in descriptor["apps"]))
        with mock.patch.object(desktop, "detect_platform", return_value="macos"):
            self.assertEqual(
                cdb_adapter._descriptor({"pack_task": "shared/cdb-s02"})["platform"], "macos"
            )


class MiscTest(unittest.TestCase):
    def test_strip_bulk_removes_screenshots_but_keeps_text(self) -> None:
        big = "A" * 50000
        out = rb.strip_bulk(
            {"type": "image", "source": {"data": big}, "text": "keep", "list": [big, "x"]}
        )
        self.assertIn("stripped", out["source"]["data"])
        self.assertEqual(out["text"], "keep")
        self.assertLess(len(json.dumps(out)), 1000)

    def test_estimate_cost_uses_cache_prices(self) -> None:
        cost = rb.estimate_cost(
            {"input": 1_000_000, "output": 0, "cache_read": 1_000_000, "cache_write": 0}
        )
        self.assertAlmostEqual(cost, 3.30)

    def test_merge_quota_keeps_known_windows(self) -> None:
        merged = rb.merge_quota(
            {"five_hour": 0.5, "seven_day": 0.9},
            {"five_hour": 0.6, "seven_day": None, "status": "allowed"},
        )
        self.assertEqual(merged["five_hour"], 0.6)
        self.assertEqual(merged["seven_day"], 0.9)
        self.assertEqual(rb.merge_quota(None, None), None)


if __name__ == "__main__":
    unittest.main()
