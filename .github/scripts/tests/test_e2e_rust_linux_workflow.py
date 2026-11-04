import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/e2e-rust-linux.yml"


def test_canonical_certification_is_fail_closed_and_complete() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    certification = workflow.split("\n  certify:\n", 1)[1]

    # Manual dispatches and stable-tag release-gate calls both certify.
    assert "if: success() && inputs.lane == 'all'" in certification
    assert "needs: [source, shared, native, capture, installer]" in certification
    assert 'schema "cua-driver/e2e-certification/v1"' in certification
    assert 'platform "linux"' in certification
    for job in ("shared", "native", "capture", "installer"):
        assert f'{job}: "success"' in certification
    assert "name: rust-linux-e2e-certification" in certification


def test_release_gate_call_matches_dispatch_and_ignores_caller_artifacts() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    trigger = workflow.split("\npermissions:", 1)[0]
    call, dispatch = trigger.split("  workflow_call:\n", 1)[1].split("  workflow_dispatch:\n", 1)
    for name in ("ref:", "lane:", "cell_filter:"):
        assert name in call
        assert name in dispatch
    assert "github.event.inputs" not in workflow
    assert "pattern: rust-linux-*" in workflow


class VerifySkillTests(unittest.TestCase):
    def test_workflow_proof_is_isolated_and_retained(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        for path in (
            ".cursor/skills/verify-cua-driver/**",
            "libs/cua-driver/**",
            ".github/workflows/e2e-rust-linux.yml",
        ):
            self.assertIn(f"      - {path}", workflow.split("  workflow_call:")[0])
        self.assertIn("          - verify", workflow)
        job = workflow.split("\n  verify-skill:\n")[1].split("\n  shared:\n")[0]
        for setting in (
            "if: inputs.lane == '' || inputs.lane == 'verify'",
            "timeout-minutes: 25",
            'CUA_E2E_RETRY_ONCE: "0"',
            "key: e2e-shared",
            "CUA_E2E_SOURCE_SHA: ${{ needs.source.outputs.sha }}",
            "run: .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11",
            "name: verify-cua-driver-evidence",
            "path: artifacts/cua-driver/verify/run-*",
            "if-no-files-found: error",
            "if: always()",
            "test_e2e_rust_linux_workflow.py",
        ):
            self.assertIn(setting, job)
        self.assertNotIn("name: rust-linux-", job)
        for name in ("summary", "certify"):
            section = workflow.split(f"\n  {name}:\n")[1].split("    steps:")[0]
            self.assertIn("needs: [source, shared, native, capture, installer]", section)
            self.assertNotIn("verify-skill", section)
        summary = workflow.split("\n  summary:\n")[1].split("    needs:")[0]
        self.assertIn("always() && inputs.lane != '' && inputs.lane != 'verify'", summary)

    def test_evidence_boundary(self) -> None:
        checker = ROOT / ".cursor/skills/verify-cua-driver/scripts/check-evidence.sh"
        self.assertTrue(checker.is_file(), "standalone evidence checker is required")
        cell = "linux-electron-left-click-ax-background"
        sleeper = subprocess.Popen(["sleep", "60"])
        dead = subprocess.Popen(["true"])
        dead.wait(timeout=5)
        try:
            scenarios = [
                "complete",
                "zero-pids",
                "survivor",
                "fixture-survivor",
                "malformed-transcript",
                "null-window-title",
                "observed-refused",
                "status-failed",
                "missing-sdk-row",
                "retry-decoy",
                "duplicate-label",
                "missing-label",
                "wrong-marker",
                "no-heartbeat",
                "no-click",
                "incomplete-pair",
                "tool-calls.jsonl",
                "fixture-state.json",
                "sentinel-events.jsonl",
                "desktop-observation.txt",
                "before.png",
                "after.png",
                "daemon.json",
                "health_report.json",
                "check_permissions.json",
                "summary.md",
            ]
            for scenario in scenarios:
                with self.subTest(scenario=scenario), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    rows = []
                    for label in (cell, cell + "-typed-sdk", "environment-preflight"):
                        directory = root / "recordings" / label
                        directory.mkdir(parents=True)
                        (directory / "trajectory.json").write_text(
                            json.dumps(
                                {
                                    "label": label,
                                    "daemon_pid": dead.pid,
                                }
                            )
                        )
                        if label == "environment-preflight":
                            for name in (
                                "daemon.json",
                                "health_report.json",
                                "check_permissions.json",
                            ):
                                (directory / name).write_text("{}")
                            continue
                        rows.append(
                            "| "
                            + " | ".join(
                                [
                                    label,
                                    "Linux/X11",
                                    "electron",
                                    "left_click",
                                    "Ax",
                                    "Background",
                                    "Window",
                                    "Mcp",
                                    "State",
                                    "Delivered",
                                    "Delivered",
                                    "Pass",
                                    "1 ms",
                                    "-",
                                    r"details \| escaped",
                                ]
                            )
                            + " |\n"
                        )
                        (directory / "tool-calls.jsonl").write_text(
                            json.dumps(
                                {
                                    "request": {"params": {"name": "list_windows"}},
                                    "response": {
                                        "result": {
                                            "structuredContent": {
                                                "windows": [
                                                    {"title": "CuaTestHarness", "pid": dead.pid},
                                                ]
                                            }
                                        }
                                    },
                                }
                            )
                            + "\n"
                        )
                        (directory / "fixture-state.json").write_text(
                            json.dumps(
                                {
                                    "text": "journal last_action=left_click",
                                }
                            )
                        )
                        (directory / "sentinel-events.jsonl").write_text('{"kind":"heartbeat"}\n')
                        (directory / "desktop-observation.txt").write_text("stable")
                        turn = directory / "turn-00001"
                        turn.mkdir()
                        for name in ("before.png", "after.png"):
                            (turn / name).write_bytes(b"png")
                        (turn / "action.json").write_text(
                            json.dumps(
                                {
                                    "tool": (
                                        "press_key" if label.endswith("-typed-sdk") else "click"
                                    ),
                                }
                            )
                        )
                        (directory / "turn-00002").mkdir()
                    (root / "summary.md").write_text("".join(rows))
                    target = root / "recordings" / cell
                    if scenario == "zero-pids":
                        for path in root.glob("recordings/*/trajectory.json"):
                            data = json.loads(path.read_text())
                            data.pop("daemon_pid")
                            path.write_text(json.dumps(data))
                        for path in root.glob("recordings/*/tool-calls.jsonl"):
                            path.write_text("{}\n")
                    elif scenario == "survivor":
                        (target / "trajectory.json").write_text(
                            json.dumps(
                                {
                                    "label": cell,
                                    "daemon_pid": sleeper.pid,
                                }
                            )
                        )
                    elif scenario == "fixture-survivor":
                        path = target / "tool-calls.jsonl"
                        data = json.loads(path.read_text())
                        data["response"]["result"]["structuredContent"]["windows"][0][
                            "pid"
                        ] = sleeper.pid
                        path.write_text(json.dumps(data))
                    elif scenario == "malformed-transcript":
                        (target / "tool-calls.jsonl").write_text("{broken\n")
                    elif scenario == "null-window-title":
                        (target / "tool-calls.jsonl").write_text(
                            json.dumps(
                                {
                                    "request": {"params": {"name": "list_windows"}},
                                    "response": {
                                        "result": {
                                            "structuredContent": {
                                                "windows": [
                                                    {"title": None},
                                                    {"title": 7},
                                                ]
                                            }
                                        }
                                    },
                                }
                            )
                        )
                    elif scenario in ("observed-refused", "status-failed"):
                        (root / "summary.md").write_text(
                            "".join(rows).replace(
                                "Delivered | Pass",
                                (
                                    "Refused | Pass"
                                    if scenario == "observed-refused"
                                    else "Delivered | Fail"
                                ),
                            )
                        )
                    elif scenario == "missing-sdk-row":
                        (root / "summary.md").write_text(rows[0])
                    elif scenario == "retry-decoy":
                        destination = root / "retried-attempts/first/recordings"
                        destination.mkdir(parents=True)
                        (target).rename(destination / cell)
                    elif scenario == "duplicate-label":
                        duplicate = root / "recordings/duplicate"
                        duplicate.mkdir()
                        (duplicate / "trajectory.json").write_text(
                            (target / "trajectory.json").read_text()
                        )
                    elif scenario == "missing-label":
                        (target / "trajectory.json").write_text('{"daemon_pid":1}')
                    elif scenario == "wrong-marker":
                        (target / "fixture-state.json").write_text(
                            '{"text":"last_action=left_click_extra"}'
                        )
                    elif scenario == "no-heartbeat":
                        (target / "sentinel-events.jsonl").write_text('{"kind":"focus"}\n')
                    elif scenario == "no-click":
                        (target / "turn-00001/action.json").write_text('{"tool":"press_key"}')
                    elif scenario == "incomplete-pair":
                        (target / "turn-00001/after.png").rename(target / "turn-00002/after.png")
                    elif scenario in (
                        "daemon.json",
                        "health_report.json",
                        "check_permissions.json",
                    ):
                        (root / "recordings/environment-preflight" / scenario).unlink()
                    elif scenario in ("before.png", "after.png"):
                        (target / "turn-00001" / scenario).unlink()
                    elif scenario == "summary.md":
                        (root / scenario).unlink()
                    elif scenario.endswith((".json", ".jsonl", ".txt")):
                        (target / scenario).unlink()
                    result = subprocess.run(
                        [str(checker), str(root)], capture_output=True, text=True, timeout=10
                    )
                    if scenario in ("complete", "null-window-title"):
                        self.assertEqual(result.returncode, 0, result.stderr)
                    else:
                        self.assertNotEqual(result.returncode, 0, scenario)
                        self.assertTrue(result.stderr.strip(), scenario)
        finally:
            sleeper.terminate()
            sleeper.wait(timeout=5)


def test_cache_saves_never_write_pull_request_scope_but_keep_dispatches() -> None:
    """A PR must not save its own sccache entry, but a dispatch still may.

    `github.ref_type == 'branch'` is true on pull_request too, so it never gated
    a PR save. The in-repo rust-cache action decides `save` itself and only drops
    a save when `save-if` is the literal "false", so these guards must evaluate
    false on pull_request and true otherwise. Swatinem/rust-cache honours
    `save-if` directly and was the one site writing PR-scoped entries.
    """
    workflow = WORKFLOW.read_text(encoding="utf-8")
    # Only the save guards: setup-node's `cache:` inputs are a separate decision.
    save_guards = [line for line in workflow.splitlines() if 'save-if:' in line]
    assert not any("ref_type == 'branch'" in line for line in save_guards)

    # `needs.source.outputs.sha == github.sha` must survive: a dispatch can
    # certify an older SHA, and that run must not seed main's scope with it.
    swatinem = workflow.split("- uses: Swatinem/rust-cache", 1)[1].split("- uses:", 1)[0]
    assert (
        "save-if: ${{ (github.ref == 'refs/heads/main' || github.event_name == 'workflow_dispatch')"
        " && needs.source.outputs.sha == github.sha }}" in swatinem
    )

    assert (
        workflow.count(
            "save-if: ${{ github.event_name != 'pull_request' && needs.source.outputs.sha == github.sha }}"
        )
        == 4
    )
