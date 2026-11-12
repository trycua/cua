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
    assert "!failure() && !cancelled() && inputs.lane == 'all'" in certification
    for job in ("source", "shared", "native", "capture", "installer"):
        assert f"needs.{job}.result == 'success'" in certification
    assert "needs: [quick, source, shared, native, capture, installer]" in certification
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
            "if: ${{ !failure() && !cancelled() && (inputs.lane == '' || inputs.lane == 'verify') && needs.quick.outputs.skip != 'true' }}",
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
            self.assertIn("needs: [quick, source, shared, native, capture, installer]", section)
            self.assertNotIn("verify-skill", section)
        summary = workflow.split("\n  summary:\n")[1].split("    needs:")[0]
        self.assertIn("always() && inputs.lane != '' && inputs.lane != 'verify'", summary)

    def test_evidence_boundary(self) -> None:
        checker = ROOT / ".cursor/skills/verify-cua-driver/scripts/check-evidence.sh"
        self.assertTrue(checker.is_file(), "standalone evidence checker is required")
        cell = "linux-electron-left-click-ax-background"
        fg_cell = "linux-electron-left-click-px-foreground"
        refused_cell = "linux-electron-type-text-ax-background"
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
                "cases.jsonl",
                "empty-cases",
                "malformed-cases",
                "undeclared-recording-missing",
                "foreground",
                "foreground-wrong-marker",
                "foreground-no-click",
                "refused-background",
                "refused-background-no-heartbeat",
            ]
            for scenario in scenarios:
                with self.subTest(scenario=scenario), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    rows = []
                    declarations = []
                    foreground = scenario.startswith("foreground")
                    refused = scenario.startswith("refused-background")
                    if foreground:
                        labels = (fg_cell, "environment-preflight")
                    elif refused:
                        labels = (refused_cell, "environment-preflight")
                    else:
                        labels = (cell, cell + "-typed-sdk", "environment-preflight")
                    for label in labels:
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
                        action = "type_text" if refused else "left_click"
                        delivery = "foreground" if foreground else "background"
                        declarations.append(
                            json.dumps(
                                {
                                    "cell_id": label,
                                    "action": action,
                                    "delivery": delivery,
                                    "expected_behavior": (
                                        {
                                            "kind": "refuse",
                                            "allowed_codes": ["background_unavailable"],
                                        }
                                        if refused
                                        else {"kind": "deliver"}
                                    ),
                                }
                            )
                            + "\n"
                        )
                        rows.append(
                            "| "
                            + " | ".join(
                                [
                                    label,
                                    "Linux/X11",
                                    "electron",
                                    action,
                                    "Ax",
                                    delivery.title(),
                                    "Window",
                                    "Mcp",
                                    "State",
                                    "Refuse" if refused else "Deliver",
                                    "Refused" if refused else "Delivered",
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
                                    "text": (
                                        "journal mirror="
                                        if refused
                                        else "journal last_action=left_click"
                                    ),
                                }
                            )
                        )
                        if not foreground:
                            (directory / "sentinel-events.jsonl").write_text(
                                '{"kind":"heartbeat"}\n'
                            )
                            (directory / "desktop-observation.txt").write_text("stable")
                        turn = directory / "turn-00001"
                        turn.mkdir()
                        for name in ("before.png", "after.png"):
                            (turn / name).write_bytes(b"png")
                        (turn / "action.json").write_text(
                            json.dumps(
                                {
                                    "tool": (
                                        "press_key"
                                        if label.endswith("-typed-sdk")
                                        else "type_text"
                                        if refused
                                        else "click"
                                    ),
                                }
                            )
                        )
                        (directory / "turn-00002").mkdir()
                    (root / "summary.md").write_text("".join(rows))
                    (root / "cases.jsonl").write_text("".join(declarations))
                    target = root / "recordings" / labels[0]
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
                    elif scenario == "empty-cases":
                        (root / "cases.jsonl").write_text("\n")
                    elif scenario == "malformed-cases":
                        (root / "cases.jsonl").write_text('{"cell_id":"' + cell + '"}\n')
                    elif scenario == "undeclared-recording-missing":
                        with (root / "cases.jsonl").open("a") as stream:
                            stream.write(
                                declarations[0].replace(cell, cell.replace("-ax-", "-px-"))
                            )
                    elif scenario == "foreground-wrong-marker":
                        (target / "fixture-state.json").write_text(
                            '{"text":"last_action=right_click"}'
                        )
                    elif scenario == "foreground-no-click":
                        (target / "turn-00001/action.json").write_text('{"tool":"press_key"}')
                    elif scenario == "refused-background-no-heartbeat":
                        (target / "sentinel-events.jsonl").write_text('{"kind":"focus"}\n')
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
                    elif scenario in ("summary.md", "cases.jsonl"):
                        (root / scenario).unlink()
                    elif scenario.endswith((".json", ".jsonl", ".txt")):
                        (target / scenario).unlink()
                    result = subprocess.run(
                        [str(checker), str(root)], capture_output=True, text=True, timeout=10
                    )
                    if scenario in (
                        "complete",
                        "null-window-title",
                        "foreground",
                        "refused-background",
                    ):
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


def test_shared_leg_keeps_its_own_sccache_family() -> None:
    """The release driver and Tauri units of the shared leg live in their own entry.

    In the default libs-cua-driver-rust family the day's entry is saved first by
    debug-only unit jobs; the shared leg then gets an exact hit, never saves, and
    rebuilds its release units on every run.
    """
    workflow = WORKFLOW.read_text(encoding="utf-8")
    shared = workflow.split("\n  shared:\n", 1)[1].split("\n  native:\n", 1)[0]
    assert "shared-key: cua-driver-e2e-shared" in shared
    assert workflow.count("shared-key: cua-driver-e2e-shared") == 1
