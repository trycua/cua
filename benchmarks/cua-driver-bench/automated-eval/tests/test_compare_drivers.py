from __future__ import annotations

import importlib.util
import json
from dataclasses import asdict
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


MODULE_PATH = Path(__file__).resolve().parents[1] / "compare_drivers.py"
SPEC = importlib.util.spec_from_file_location("test_compare_drivers_module", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
compare_drivers = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = compare_drivers
SPEC.loader.exec_module(compare_drivers)


def _foreground_event(kind: str, *, window: int | None = None, button: int | None = None):
    return compare_drivers.ForegroundEvent(
        kind=kind,
        observed_at="2026-09-08T12:00:00+00:00",
        monotonic_ns=1,
        window=window,
        button=button,
    )


def _foreground_sample(
    *,
    focus_window: int = 10,
    focus_viewable: bool = True,
    cursor_x: int = 100,
    cursor_y: int = 100,
    button_mask: int = 0,
    pointer_window: int = 20,
    pointer_viewable: bool = True,
    events=(),
):
    return compare_drivers.ForegroundSample(
        focus_window=focus_window,
        cursor_x=cursor_x,
        cursor_y=cursor_y,
        button_mask=button_mask,
        focus_window_viewable=focus_viewable,
        pointer_window=pointer_window,
        pointer_window_viewable=pointer_viewable,
        events=tuple(events),
    )


def _write_foreground_evidence(trial: Path) -> None:
    (trial / "observer" / "foreground-disturbance.ndjson").write_text(
        json.dumps(
            {
                "type": "sample",
                "observed_at": "2026-09-08T12:00:00+00:00",
                "monotonic_ns": 1,
            }
        )
        + "\n",
        encoding="utf-8",
    )


class CompareDriversTests(unittest.TestCase):
    def test_codex_runtime_root_uses_cache_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cache_home = Path(temporary) / "cache"
            with patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache_home)}):
                root = compare_drivers._codex_runtime_root()

            self.assertEqual(
                root,
                (cache_home / "cua-driver-bench" / "codex").resolve(),
            )
            self.assertTrue(root.is_dir())

    def test_copies_selected_codex_model_provider_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            codex_home = Path(temporary)
            (codex_home / "config.toml").write_text(
                """
model = "ignored"
model_provider = "gateway"

[model_providers.gateway]
name = "Gateway"
base_url = "https://gateway.invalid/v1"
env_key = "GATEWAY_API_KEY"
wire_api = "responses"
request_max_retries = 2

[model_providers.gateway.http_headers]
x-client = "benchmark"

[mcp_servers.unrelated]
command = "unrelated"
""".strip(),
                encoding="utf-8",
            )

            lines, environment_key = compare_drivers._codex_provider_config(codex_home)
            rendered = compare_drivers.tomllib.loads("\n".join(lines))

        self.assertEqual(environment_key, "GATEWAY_API_KEY")
        self.assertEqual(rendered["model_provider"], "gateway")
        self.assertEqual(
            rendered["model_providers"]["gateway"]["base_url"],
            "https://gateway.invalid/v1",
        )
        self.assertNotIn("mcp_servers", rendered)

    def test_discovers_required_evaluator_node_contract(self) -> None:
        bundle = MODULE_PATH.parents[1] / "tasks" / "shared" / "cdb-s01"
        completed = compare_drivers.subprocess.CompletedProcess(
            [sys.executable, "--version"], 0, "v22.12.0\n", ""
        )
        descriptor = {
            "semantics": {"evaluate": ["${evaluator_node}", "evaluator/check.js"]},
            "prerequisites": [
                {
                    "id": "node",
                    "check": ["node", "--version"],
                    "min_version": "22.0.0",
                }
            ],
        }
        with (
            patch.object(
                compare_drivers,
                "load_launch_descriptor",
                return_value=descriptor,
            ),
            patch.object(compare_drivers.shutil, "which", return_value=sys.executable),
            patch.object(compare_drivers, "_run_preflight_command", return_value=completed),
            patch.object(
                compare_drivers,
                "digest_file",
                return_value="sha256:" + "a" * 64,
            ),
        ):
            options = compare_drivers._evaluator_node_options(bundle, "windows")

        self.assertEqual(options["evaluator_node_path"], Path(sys.executable).resolve())
        self.assertEqual(options["evaluator_node_sha256"], "a" * 64)
        self.assertEqual(options["evaluator_node_version"], "v22.12.0")

    def test_codex_preflight_does_not_require_login_status(self) -> None:
        config = SimpleNamespace(
            codex=Path("codex"),
            codex_home=Path(".codex"),
            repo_root=Path.cwd(),
        )
        completed = compare_drivers.subprocess.CompletedProcess(
            ["codex", "--version"], 0, "codex-cli 1.0\n", ""
        )
        with patch.object(compare_drivers, "_run_preflight_command", return_value=completed) as run:
            version = compare_drivers._preflight_codex(config)

        self.assertEqual(version, "codex-cli 1.0")
        run.assert_called_once()
        self.assertEqual(run.call_args.args[0], ["codex", "--version"])

    def test_python_prerequisite_uses_current_interpreter(self) -> None:
        completed = compare_drivers.subprocess.CompletedProcess(
            [sys.executable, "--version"], 0, "Python 3.12.7\n", ""
        )
        descriptor = {
            "prerequisites": [
                {
                    "id": "python",
                    "check": ["python", "--version"],
                    "min_version": "3.11",
                }
            ]
        }
        with (
            patch.object(compare_drivers, "load_launch_descriptor", return_value=descriptor),
            patch.object(compare_drivers, "_run_preflight_command", return_value=completed) as run,
        ):
            compare_drivers._preflight_task(Path.cwd(), "windows")

        self.assertEqual(run.call_args.args[0], [sys.executable, "--version"])

    def test_agent_harness_forwards_provider_environment(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            command = root / "agent.py"
            command.write_text("", encoding="utf-8")
            artifacts = root / "artifacts"
            artifacts.mkdir()
            delegate = SimpleNamespace(command=command, expected_digest="expected")
            context = SimpleNamespace(
                artifacts=artifacts,
                harness_workspace=root,
                emit=lambda *_args: None,
            )
            handle = SimpleNamespace(root=root / "workspace")
            process = SimpleNamespace(
                returncode=0,
                duration_ms=123,
                truncated=False,
                output_exceeded=False,
            )
            harness = compare_drivers.EnvironmentSubprocessHarness(
                delegate, {"LITELLM_API_KEY": "test-key"}
            )
            with (
                patch.object(compare_drivers, "digest_file", return_value="expected"),
                patch.object(compare_drivers, "run_process", return_value=process) as run,
            ):
                outcome = harness.run(context, handle, SimpleNamespace(), 30.0)

        self.assertTrue(outcome.completed)
        self.assertEqual(
            run.call_args.kwargs["extra_env"],
            {"LITELLM_API_KEY": "test-key"},
        )

    def test_socket_observer_degrades_after_daemon_disconnect(self) -> None:
        class DisconnectedClient:
            def call(self, _method: str, _arguments: dict[str, object]) -> object:
                raise compare_drivers.HarnessFailure("daemon disconnected")

            def close(self) -> None:
                return None

        observer = compare_drivers.SocketCuaRecordingObserver(
            Path("cua-driver.exe"), "endpoint", "0.23.2", {}
        )
        observer.client = DisconnectedClient()
        observer.initial_apps = {}
        observer.requirements = ()
        observer.recording_dir = Path.cwd() / "missing-recording-directory"

        report = observer.finish(SimpleNamespace(), SimpleNamespace(), SimpleNamespace())

        self.assertEqual(report.trust, "non_certifying")
        self.assertEqual(report.events, ())
        self.assertIn("final app inventory unavailable", report.detail)
        self.assertIsNone(observer.client)

    def test_wait_for_codex_stops_after_terminal_event_grace(self) -> None:
        process = SimpleNamespace(pid=123, returncode=None, poll=lambda: None)
        with (
            patch.object(compare_drivers, "_codex_turn_completed", return_value=True),
            patch.object(compare_drivers, "_terminate_process_group") as terminate,
            patch.object(
                compare_drivers.time,
                "monotonic",
                side_effect=(100.0, 100.0, 106.0),
            ),
            patch.object(compare_drivers.time, "sleep"),
        ):
            result = compare_drivers._wait_for_codex(
                process, Path("events.jsonl"), 30.0, terminal_grace_seconds=5.0
            )

        self.assertEqual(result, (0, False, True))
        terminate.assert_called_once_with(123)

    def test_cleanup_terminates_reparented_trial_processes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            artifacts = trial / "artifacts"
            artifacts.mkdir()
            (artifacts / "app-processes.json").write_text(
                json.dumps([{"pid": 101, "process_group": 101}]),
                encoding="utf-8",
            )
            with (
                patch.object(
                    compare_drivers,
                    "_windows_processes_for_path",
                    return_value=(202,),
                ),
                patch.object(compare_drivers, "_terminate_process_group") as terminate,
            ):
                compare_drivers._cleanup_trial_processes(trial)

        self.assertEqual(
            [call.args[0] for call in terminate.call_args_list],
            [101, 202],
        )

    def test_discovers_semver_releases_and_matching_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for version in ("0.23.2", "0.22.2"):
                release = root / version
                (release / "binary").mkdir(parents=True)
                (release / "release-manifest.json").write_text(
                    json.dumps({"version": version}), encoding="utf-8"
                )
                (release / "binary" / "cua-driver").write_text("", encoding="utf-8")
            releases = compare_drivers.discover_driver_releases(root, "linux")
            self.assertEqual(list(releases), ["0.22.2", "0.23.2"])
            self.assertEqual(releases["0.23.2"].version, "0.23.2")

    def test_rejects_manifest_version_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release = root / "0.23.2"
            release.mkdir()
            (release / "release-manifest.json").write_text(
                json.dumps({"version": "0.23.1"}), encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "does not match"):
                compare_drivers.discover_driver_releases(root, "linux")

    def test_expands_nested_descriptor_placeholders(self) -> None:
        value = {
            "command": ["tool", "${workspace}/file"],
            "env": {"HOME": "${HOME}"},
        }
        expanded = compare_drivers.expand_placeholders(
            value, {"workspace": "/tmp/work", "HOME": "/home/test"}
        )
        self.assertEqual(expanded["command"][1], "/tmp/work/file")
        self.assertEqual(expanded["env"]["HOME"], "/home/test")

    def test_compares_prerequisite_versions_numerically(self) -> None:
        actual = compare_drivers._parsed_version("LibreOffice 25.2.1.2")
        minimum = compare_drivers._parsed_version("25")
        self.assertTrue(compare_drivers._version_at_least(actual, minimum))
        self.assertFalse(compare_drivers._version_at_least((5, 15), (5, 16)))

    def test_valid_focus_transfer_is_not_a_focus_drop(self) -> None:
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(focus_window=10),
                _foreground_sample(focus_window=20),
            )
        )

        self.assertEqual(summary["foreground_keyboard_focus_drops"], 0)
        self.assertEqual(summary["focus_window_transitions"], 1)
        self.assertEqual(summary["foreground_disturbances"], 0)

    def test_focus_drop_debounces_until_valid_focus_returns(self) -> None:
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(),
                _foreground_sample(focus_window=0, focus_viewable=False),
                _foreground_sample(focus_window=0, focus_viewable=False),
                _foreground_sample(),
                _foreground_sample(focus_window=0, focus_viewable=False),
            )
        )

        self.assertEqual(summary["foreground_keyboard_focus_drops"], 2)
        self.assertEqual(summary["foreground_disturbances"], 2)

    def test_cursor_deviation_requires_return_before_rearming(self) -> None:
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(cursor_x=100),
                _foreground_sample(cursor_x=111),
                _foreground_sample(cursor_x=130),
                _foreground_sample(cursor_x=110),
                _foreground_sample(cursor_x=111),
            )
        )

        self.assertEqual(summary["foreground_cursor_trajectory_deviations"], 2)
        self.assertEqual(summary["cursor_deviation_episodes"], 2)
        self.assertEqual(summary["foreground_disturbances"], 2)

    def test_normal_drag_release_is_excluded(self) -> None:
        button = compare_drivers._x11_button_bit(1)
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(),
                _foreground_sample(
                    button_mask=button,
                    events=(_foreground_event("button_press", window=20, button=1),),
                ),
                _foreground_sample(
                    events=(_foreground_event("button_release", window=20, button=1),)
                ),
            )
        )

        self.assertEqual(summary["foreground_drag_observations"], 1)
        self.assertEqual(summary["foreground_drag_interruptions"], 0)
        self.assertEqual(summary["foreground_disturbances"], 0)

    def test_focus_loss_interrupts_an_active_drag(self) -> None:
        button = compare_drivers._x11_button_bit(1)
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(),
                _foreground_sample(
                    button_mask=button,
                    events=(_foreground_event("button_press", window=20, button=1),),
                ),
                _foreground_sample(
                    focus_window=0,
                    focus_viewable=False,
                    button_mask=button,
                ),
            )
        )

        self.assertEqual(summary["foreground_keyboard_focus_drops"], 1)
        self.assertEqual(summary["foreground_drag_interruptions"], 1)
        self.assertEqual(summary["foreground_disturbances"], 2)

    def test_pointer_ungrab_interrupts_an_active_drag(self) -> None:
        button = compare_drivers._x11_button_bit(1)
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(),
                _foreground_sample(
                    button_mask=button,
                    events=(_foreground_event("button_press", window=20, button=1),),
                ),
                _foreground_sample(
                    button_mask=button,
                    events=(_foreground_event("pointer_ungrab"),),
                ),
            )
        )

        self.assertEqual(summary["foreground_drag_interruptions"], 1)

    def test_unavailable_drag_window_interrupts_an_active_drag(self) -> None:
        button = compare_drivers._x11_button_bit(1)
        for event_kind in ("window_unmap", "window_destroy"):
            with self.subTest(event_kind=event_kind):
                summary = compare_drivers.summarize_foreground_samples(
                    (
                        _foreground_sample(),
                        _foreground_sample(
                            button_mask=button,
                            events=(_foreground_event("button_press", window=20, button=1),),
                        ),
                        _foreground_sample(
                            button_mask=button,
                            events=(_foreground_event(event_kind, window=20),),
                        ),
                    )
                )

                self.assertEqual(summary["foreground_drag_interruptions"], 1)

    def test_unexpected_button_state_change_interrupts_drag(self) -> None:
        button = compare_drivers._x11_button_bit(1)
        summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(),
                _foreground_sample(
                    button_mask=button,
                    events=(_foreground_event("button_press", window=20, button=1),),
                ),
                _foreground_sample(button_mask=0),
            )
        )

        self.assertEqual(summary["foreground_drag_interruptions"], 1)

    def test_drag_setup_and_observer_teardown_are_excluded(self) -> None:
        button = compare_drivers._x11_button_bit(1)
        setup_summary = compare_drivers.summarize_foreground_samples(
            (_foreground_sample(button_mask=button),)
        )
        active_summary = compare_drivers.summarize_foreground_samples(
            (
                _foreground_sample(),
                _foreground_sample(
                    button_mask=button,
                    events=(_foreground_event("button_press", window=20, button=1),),
                ),
            )
        )

        self.assertEqual(setup_summary["foreground_drag_observations"], 0)
        self.assertEqual(setup_summary["foreground_drag_interruptions"], 0)
        self.assertEqual(active_summary["foreground_drag_observations"], 1)
        self.assertEqual(active_summary["foreground_drag_interruptions"], 0)

    def test_missing_samples_and_drag_events_report_component_availability(self) -> None:
        missing = compare_drivers.summarize_foreground_samples(())
        no_drag_events = compare_drivers.summarize_foreground_samples(
            (_foreground_sample(),), drag_events_available=False
        )

        self.assertFalse(missing["available"])
        self.assertIsNone(missing["foreground_disturbances"])
        self.assertFalse(no_drag_events["available"])
        self.assertTrue(no_drag_events["foreground_keyboard_focus_available"])
        self.assertFalse(no_drag_events["foreground_drag_available"])
        self.assertTrue(no_drag_events["foreground_cursor_trajectory_available"])
        self.assertIsNone(no_drag_events["foreground_disturbances"])

    def test_foreground_observer_writes_timestamped_raw_evidence(self) -> None:
        class StaticSource:
            drag_events_available = True
            drag_error = None

            def sample(self):
                return compare_drivers.ForegroundSample(42, 50, 60)

            def close(self):
                return None

        displays = []

        def source_factory(display_name):
            displays.append(display_name)
            return StaticSource()

        with tempfile.TemporaryDirectory() as temporary:
            trial_dir = Path(temporary)
            context = SimpleNamespace(trial_dir=trial_dir)
            observer = compare_drivers.ForegroundDisturbanceObserver(
                {"DISPLAY": ":test"}, source_factory=source_factory
            )
            with patch.object(compare_drivers, "detect_platform", return_value="linux"):
                observer.start(context)
                observer.finish()
            summary = json.loads(
                (trial_dir / "observer" / "foreground-disturbance.json").read_text(encoding="utf-8")
            )
            evidence = [
                json.loads(line)
                for line in (trial_dir / "observer" / "foreground-disturbance.ndjson")
                .read_text(encoding="utf-8")
                .splitlines()
            ]

        self.assertEqual(displays, [":test"])
        self.assertTrue(summary["available"])
        self.assertEqual(summary["schema_version"], "2")
        self.assertEqual(summary["sample_count"], 1)
        self.assertEqual(summary["foreground_keyboard_focus_drops"], 0)
        self.assertEqual(summary["foreground_drag_interruptions"], 0)
        self.assertEqual(summary["foreground_cursor_trajectory_deviations"], 0)
        self.assertEqual(summary["foreground_disturbances"], 0)
        self.assertEqual(len(evidence), 1)
        self.assertEqual(evidence[0]["type"], "sample")
        self.assertIsInstance(evidence[0]["observed_at"], str)
        self.assertIsInstance(evidence[0]["monotonic_ns"], int)

    def test_foreground_observer_marks_missing_display_unavailable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            context = SimpleNamespace(trial_dir=Path(temporary))
            observer = compare_drivers.ForegroundDisturbanceObserver({})

            with patch.object(compare_drivers.sys, "platform", "linux"):
                observer.start(context)
                observer.finish()

            summary = json.loads(
                (Path(temporary) / "observer" / "foreground-disturbance.json").read_text(
                    encoding="utf-8"
                )
            )
        self.assertFalse(summary["available"])
        self.assertFalse(summary["foreground_keyboard_focus_available"])
        self.assertFalse(summary["foreground_drag_available"])
        self.assertFalse(summary["foreground_cursor_trajectory_available"])
        self.assertIsNone(summary["foreground_keyboard_focus_drops"])
        self.assertIsNone(summary["foreground_drag_interruptions"])
        self.assertIsNone(summary["foreground_cursor_trajectory_deviations"])
        self.assertIsNone(summary["foreground_disturbances"])
        self.assertEqual(summary["error"], "DISPLAY is not set")

    def test_counts_cua_calls_and_successful_inputs_from_codex_events(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            artifacts = trial / "artifacts"
            artifacts.mkdir()

            def call(
                tool: str, status: str = "completed", server: str = "cua"
            ) -> dict[str, object]:
                return {
                    "type": "item.completed",
                    "item": {
                        "type": "mcp_tool_call",
                        "server": server,
                        "tool": tool,
                        "status": status,
                    },
                }

            events = [
                call("get_window_state"),
                call("click"),
                call("browser_click"),
                call("scroll", status="failed"),
                call("click", server="other"),
            ]
            (artifacts / "codex-events.jsonl").write_text(
                "\n".join(json.dumps(event) for event in events) + "\n",
                encoding="utf-8",
            )

            self.assertEqual(compare_drivers._codex_cua_metrics(trial), (4, 2))

    def test_extracts_metrics_from_raw_trial(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            (trial / "observer" / "cua-driver-recording" / "turn-00001").mkdir(parents=True)
            (trial / "observer" / "cua-driver-recording" / "turn-00002").mkdir()
            (trial / "observer" / "cua-driver-recording" / "turn-00003").mkdir()
            actions = (
                {"tool": "click", "result_error": False},
                {"tool": "screenshot", "result_error": False},
                {"tool": "type_text", "result_error": True},
            )
            for index, action in enumerate(actions, start=1):
                path = (
                    trial
                    / "observer"
                    / "cua-driver-recording"
                    / f"turn-{index:05d}"
                    / "action.json"
                )
                path.write_text(json.dumps(action), encoding="utf-8")
            (trial / "observer" / "foreground-disturbance.json").write_text(
                json.dumps(
                    {
                        "available": True,
                        "measurement": (compare_drivers.FOREGROUND_DISTURBANCE_MEASUREMENT),
                        "evidence_path": "foreground-disturbance.ndjson",
                        "foreground_keyboard_focus_available": True,
                        "foreground_keyboard_focus_error": None,
                        "foreground_keyboard_focus_drops": 1,
                        "foreground_drag_available": True,
                        "foreground_drag_error": None,
                        "foreground_drag_interruptions": 2,
                        "foreground_drag_observations": 3,
                        "foreground_cursor_trajectory_available": True,
                        "foreground_cursor_trajectory_error": None,
                        "foreground_cursor_trajectory_deviations": 2,
                        "focus_window_transitions": 3,
                        "cursor_deviation_episodes": 2,
                        "foreground_disturbances": 5,
                    }
                ),
                encoding="utf-8",
            )
            _write_foreground_evidence(trial)
            (trial / "artifacts").mkdir()
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "status": "completed",
                        "elapsed_ms": 1234,
                        "evaluation": {"passed": True, "score": 0.75},
                    }
                ),
                encoding="utf-8",
            )
            (trial / "artifacts" / "codex-run.json").write_text(
                json.dumps({"exit_code": 0, "timed_out": False}), encoding="utf-8"
            )
            (trial / "artifacts" / "codex-telemetry.json").write_text(
                json.dumps(
                    {
                        "usage": {
                            "input_tokens": 12,
                            "cache_read_tokens": 7,
                            "output_tokens": 5,
                        }
                    }
                ),
                encoding="utf-8",
            )
            metrics = compare_drivers.extract_trial_metrics(trial, "CDB-S01", "0.23.2", "trial")
            self.assertTrue(metrics.passed)
            self.assertEqual(metrics.score, 0.75)
            self.assertEqual(metrics.cua_calls, 3)
            self.assertEqual(metrics.input_actions, 1)
            self.assertEqual(metrics.codex_tokens["input_tokens"], 12)
            self.assertEqual(metrics.codex_tokens["cache_read_tokens"], 7)
            self.assertEqual(metrics.codex_tokens["output_tokens"], 5)
            self.assertTrue(metrics.foreground_disturbance_available)
            self.assertTrue(metrics.foreground_keyboard_focus_available)
            self.assertEqual(metrics.foreground_keyboard_focus_drops, 1)
            self.assertTrue(metrics.foreground_drag_available)
            self.assertEqual(metrics.foreground_drag_interruptions, 2)
            self.assertEqual(metrics.foreground_drag_observations, 3)
            self.assertTrue(metrics.foreground_cursor_trajectory_available)
            self.assertEqual(metrics.foreground_cursor_trajectory_deviations, 2)
            self.assertEqual(metrics.focus_window_transitions, 3)
            self.assertEqual(metrics.cursor_deviation_episodes, 2)
            self.assertEqual(metrics.foreground_disturbances, 5)

    def test_missing_component_evidence_preserves_available_components(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            (trial / "observer").mkdir()
            (trial / "observer" / "foreground-disturbance.json").write_text(
                json.dumps(
                    {
                        "available": False,
                        "measurement": (compare_drivers.FOREGROUND_DISTURBANCE_MEASUREMENT),
                        "evidence_path": "foreground-disturbance.ndjson",
                        "foreground_keyboard_focus_available": True,
                        "foreground_keyboard_focus_drops": 1,
                        "foreground_drag_available": False,
                        "foreground_drag_error": "XRecord unavailable",
                        "foreground_drag_interruptions": None,
                        "foreground_drag_observations": None,
                        "foreground_cursor_trajectory_available": True,
                        "foreground_cursor_trajectory_deviations": 2,
                        "foreground_disturbances": None,
                        "error": "one or more components unavailable",
                    }
                ),
                encoding="utf-8",
            )
            _write_foreground_evidence(trial)

            metrics = compare_drivers._foreground_disturbance_metrics(trial)

        self.assertFalse(metrics["foreground_disturbance_available"])
        self.assertTrue(metrics["foreground_keyboard_focus_available"])
        self.assertEqual(metrics["foreground_keyboard_focus_drops"], 1)
        self.assertFalse(metrics["foreground_drag_available"])
        self.assertEqual(metrics["foreground_drag_error"], "XRecord unavailable")
        self.assertTrue(metrics["foreground_cursor_trajectory_available"])
        self.assertEqual(metrics["foreground_cursor_trajectory_deviations"], 2)
        self.assertIsNone(metrics["foreground_disturbances"])
        trial_metrics = compare_drivers.TrialMetrics(
            "CDB-S01",
            "0.23.2",
            "trial",
            None,
            True,
            1.0,
            100,
            1,
            1,
            "completed",
            **metrics,
        )
        markdown = compare_drivers.render_markdown(
            {"trials": [asdict(trial_metrics)], "comparisons": []}
        )
        self.assertIn("XRecord unavailable", markdown)

    def test_missing_raw_foreground_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            (trial / "observer").mkdir()
            (trial / "observer" / "foreground-disturbance.json").write_text(
                json.dumps(
                    {
                        "available": True,
                        "measurement": (compare_drivers.FOREGROUND_DISTURBANCE_MEASUREMENT),
                        "evidence_path": "foreground-disturbance.ndjson",
                        "foreground_keyboard_focus_available": True,
                        "foreground_keyboard_focus_drops": 0,
                        "foreground_drag_available": True,
                        "foreground_drag_interruptions": 0,
                        "foreground_drag_observations": 0,
                        "foreground_cursor_trajectory_available": True,
                        "foreground_cursor_trajectory_deviations": 0,
                        "foreground_disturbances": 0,
                    }
                ),
                encoding="utf-8",
            )

            metrics = compare_drivers._foreground_disturbance_metrics(trial)

        self.assertFalse(metrics["foreground_disturbance_available"])
        self.assertFalse(metrics["foreground_keyboard_focus_available"])
        self.assertFalse(metrics["foreground_drag_available"])
        self.assertFalse(metrics["foreground_cursor_trajectory_available"])
        self.assertIn("evidence unavailable", metrics["foreground_disturbance_error"])

    def test_invalid_foreground_disturbance_aggregate_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            (trial / "observer").mkdir()
            (trial / "observer" / "foreground-disturbance.json").write_text(
                json.dumps(
                    {
                        "available": True,
                        "measurement": (compare_drivers.FOREGROUND_DISTURBANCE_MEASUREMENT),
                        "evidence_path": "foreground-disturbance.ndjson",
                        "foreground_keyboard_focus_available": True,
                        "foreground_keyboard_focus_drops": 1,
                        "foreground_drag_available": True,
                        "foreground_drag_interruptions": 2,
                        "foreground_drag_observations": 1,
                        "foreground_cursor_trajectory_available": True,
                        "foreground_cursor_trajectory_deviations": 3,
                        "foreground_disturbances": 99,
                    }
                ),
                encoding="utf-8",
            )
            _write_foreground_evidence(trial)

            metrics = compare_drivers._foreground_disturbance_metrics(trial)

        self.assertFalse(metrics["foreground_disturbance_available"])
        self.assertIsNone(metrics["foreground_disturbances"])
        self.assertIn(
            "does not equal the component sum",
            metrics["foreground_disturbance_error"],
        )

    def test_extracts_agent_exit_when_codex_run_is_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            (trial / "artifacts").mkdir()
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "status": "completed",
                        "elapsed_ms": 1234,
                        "evaluation": {
                            "passed": False,
                            "score": 0.8,
                            "detail": {"agent_exit_code": -15},
                        },
                    }
                ),
                encoding="utf-8",
            )

            metrics = compare_drivers.extract_trial_metrics(trial, "CDB-S01", "0.23.2", "trial")

            self.assertEqual(metrics.termination, "agent_exit_-15")
            self.assertFalse(metrics.foreground_disturbance_available)
            self.assertIsNone(metrics.foreground_disturbances)

    def test_compares_candidate_disturbance_component_deltas(self) -> None:
        baseline = compare_drivers.TrialMetrics(
            "CDB-S01",
            "0.22.2",
            "a",
            None,
            False,
            0.5,
            100,
            10,
            4,
            "completed",
            codex_tokens={
                "input_tokens": 100,
                "cache_read_tokens": 60,
                "output_tokens": 20,
            },
            foreground_disturbance_available=True,
            foreground_disturbance_measurement=(compare_drivers.FOREGROUND_DISTURBANCE_MEASUREMENT),
            foreground_keyboard_focus_available=True,
            foreground_keyboard_focus_drops=1,
            foreground_drag_available=True,
            foreground_drag_interruptions=1,
            foreground_drag_observations=2,
            foreground_cursor_trajectory_available=True,
            foreground_cursor_trajectory_deviations=1,
            foreground_disturbances=3,
            focus_window_transitions=2,
            cursor_deviation_episodes=1,
        )
        candidate = compare_drivers.TrialMetrics(
            "CDB-S01",
            "0.23.2",
            "b",
            None,
            True,
            0.8,
            80,
            8,
            3,
            "completed",
            codex_tokens={
                "input_tokens": 90,
                "cache_read_tokens": 70,
                "output_tokens": 18,
            },
            foreground_disturbance_available=True,
            foreground_disturbance_measurement=(compare_drivers.FOREGROUND_DISTURBANCE_MEASUREMENT),
            foreground_keyboard_focus_available=True,
            foreground_keyboard_focus_drops=2,
            foreground_drag_available=True,
            foreground_drag_interruptions=2,
            foreground_drag_observations=3,
            foreground_cursor_trajectory_available=True,
            foreground_cursor_trajectory_deviations=2,
            foreground_disturbances=6,
            focus_window_transitions=3,
            cursor_deviation_episodes=2,
        )
        comparison = compare_drivers.compare_pair(baseline, candidate)
        self.assertAlmostEqual(comparison["delta"]["score"], 0.3)
        self.assertEqual(comparison["delta"]["total_ms"], -20)
        self.assertEqual(comparison["delta"]["input_tokens"], -10)
        self.assertEqual(comparison["delta"]["cached_tokens"], 10)
        self.assertEqual(comparison["delta"]["output_tokens"], -2)
        self.assertEqual(comparison["delta"]["foreground_keyboard_focus_drops"], 1)
        self.assertEqual(comparison["delta"]["foreground_drag_interruptions"], 1)
        self.assertEqual(comparison["delta"]["foreground_cursor_trajectory_deviations"], 1)
        self.assertEqual(comparison["delta"]["foreground_disturbances"], 3)
        self.assertEqual(comparison["signal"], "improved")
        markdown = compare_drivers.render_markdown(
            {
                "trials": [asdict(baseline), asdict(candidate)],
                "comparisons": [comparison],
            }
        )

        self.assertIn("Focus drops | Drag interruptions | Cursor deviations", markdown)
        self.assertIn("Focus-drop Δ | Drag Δ | Cursor Δ | Disturbance Δ", markdown)
        self.assertIn("## Foreground Disturbance Availability", markdown)
        self.assertIn(
            "| CDB-S01 | 0.22.2 | no | 0.5 | 100 | 10 | 4 | 1 | 1 | 1 | 3 | 100 | 60 | 20 | completed |",
            markdown,
        )
        self.assertIn("| 1 | 1 | 1 | 3 | -10 | 10 | -2 | improved |", markdown)

    def test_foreground_disturbances_do_not_affect_comparison_signal(self) -> None:
        baseline = compare_drivers.TrialMetrics(
            "CDB-S01",
            "0.22.2",
            "a",
            None,
            True,
            1.0,
            100,
            10,
            4,
            "completed",
            foreground_disturbance_available=True,
            foreground_keyboard_focus_available=True,
            foreground_keyboard_focus_drops=0,
            foreground_drag_available=True,
            foreground_drag_interruptions=0,
            foreground_cursor_trajectory_available=True,
            foreground_cursor_trajectory_deviations=0,
            foreground_disturbances=0,
        )
        candidate = compare_drivers.TrialMetrics(
            "CDB-S01",
            "0.23.2",
            "b",
            None,
            True,
            1.0,
            100,
            10,
            4,
            "completed",
            foreground_disturbance_available=True,
            foreground_keyboard_focus_available=True,
            foreground_keyboard_focus_drops=30,
            foreground_drag_available=True,
            foreground_drag_interruptions=30,
            foreground_cursor_trajectory_available=True,
            foreground_cursor_trajectory_deviations=40,
            foreground_disturbances=100,
        )

        comparison = compare_drivers.compare_pair(baseline, candidate)

        self.assertEqual(comparison["delta"]["foreground_disturbances"], 100)
        self.assertEqual(comparison["signal"], "unchanged")

    def test_normalizes_single_and_comparison_release_selection(self) -> None:
        self.assertEqual(
            compare_drivers.normalize_release_selection(None, None),
            (
                compare_drivers.DEFAULT_BASELINE,
                compare_drivers.DEFAULT_CANDIDATE,
            ),
        )
        self.assertEqual(
            compare_drivers.normalize_release_selection("0.23.2", None),
            ("0.23.2", None),
        )
        self.assertEqual(
            compare_drivers.normalize_release_selection(None, "0.23.2"),
            ("0.23.2", None),
        )
        self.assertEqual(
            compare_drivers.normalize_release_selection("0.22.2", "0.23.2"),
            ("0.22.2", "0.23.2"),
        )
        self.assertEqual(
            compare_drivers.normalize_release_selection("0.23.2", "0.23.2"),
            ("0.23.2", None),
        )

    def test_single_release_plan_has_one_trial_and_no_candidate(self) -> None:
        release = compare_drivers.DriverRelease(
            version="0.23.2",
            root=Path("/release/0.23.2"),
            binary=Path("/release/0.23.2/cua-driver"),
            manifest=Path("/release/0.23.2/release-manifest.json"),
        )
        config = compare_drivers.ComparisonConfig(
            repo_root=MODULE_PATH.parents[1],
            tasks_root=MODULE_PATH.parents[1] / "tasks",
            drivers_root=Path("/release"),
            baseline="0.23.2",
            candidate=None,
            tasks=("CDB-S01",),
            output=Path("/output"),
            platform="linux",
            codex=Path("/codex"),
            codex_home=Path("/codex-home"),
            model="large",
            reasoning_effort="high",
            timeout_seconds=1800,
        )
        with (
            patch.object(
                compare_drivers,
                "discover_driver_releases",
                return_value={"0.23.2": release},
            ),
            patch.object(compare_drivers, "require_release", return_value=release),
            patch.object(
                compare_drivers,
                "task_path",
                return_value=Path("/tasks/shared/cdb-s01/task.cuabench.json"),
            ),
            patch.object(compare_drivers, "load_launch_descriptor"),
        ):
            plan = compare_drivers.build_plan(config)

        self.assertEqual(plan["baseline"]["version"], "0.23.2")
        self.assertIsNone(plan["candidate"])
        self.assertEqual(plan["trials"], [{"task": "CDB-S01", "version": "0.23.2"}])

    def test_single_release_report_keeps_format_without_comparisons(self) -> None:
        release = compare_drivers.DriverRelease(
            version="0.23.2",
            root=Path("/release/0.23.2"),
            binary=Path("/release/0.23.2/cua-driver"),
            manifest=Path("/release/0.23.2/release-manifest.json"),
        )
        trial = compare_drivers.TrialMetrics(
            "CDB-S01", "0.23.2", "trial", None, True, 1.0, 100, 2, 1, "completed"
        )
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "report"
            config = compare_drivers.ComparisonConfig(
                repo_root=MODULE_PATH.parents[1],
                tasks_root=MODULE_PATH.parents[1] / "tasks",
                drivers_root=Path("/release"),
                baseline="0.23.2",
                candidate=None,
                tasks=("CDB-S01",),
                output=output,
                platform="linux",
                codex=Path("/codex"),
                codex_home=Path("/codex-home"),
                model="large",
                reasoning_effort="high",
                timeout_seconds=1800,
            )
            with (
                patch.object(
                    compare_drivers,
                    "discover_driver_releases",
                    return_value={"0.23.2": release},
                ),
                patch.object(compare_drivers, "require_release", return_value=release),
                patch.object(
                    compare_drivers,
                    "task_path",
                    return_value=Path("/tasks/shared/cdb-s01/task.cuabench.json"),
                ),
                patch.object(compare_drivers, "preflight", return_value="codex-cli"),
                patch.object(compare_drivers, "_run_trial", return_value=trial),
            ):
                report, json_path, markdown_path = compare_drivers.run_comparison(config)

            persisted = json.loads(json_path.read_text(encoding="utf-8"))
            markdown = markdown_path.read_text(encoding="utf-8")

        self.assertEqual(report, persisted)
        self.assertEqual(report["baseline"], "0.23.2")
        self.assertIsNone(report["candidate"])
        self.assertEqual(report["drivers"]["baseline"]["version"], "0.23.2")
        self.assertIsNone(report["drivers"]["candidate"])
        self.assertEqual(len(report["trials"]), 1)
        self.assertIn("foreground_keyboard_focus_drops", report["trials"][0])
        self.assertIn("foreground_drag_interruptions", report["trials"][0])
        self.assertIn("foreground_cursor_trajectory_deviations", report["trials"][0])
        self.assertEqual(
            set(report["foreground_disturbance"]["components"]),
            {
                "foreground_keyboard_focus_drops",
                "foreground_drag_interruptions",
                "foreground_cursor_trajectory_deviations",
            },
        )
        self.assertFalse(report["foreground_disturbance"]["affects_signal"])
        self.assertEqual(report["comparisons"], [])
        self.assertIn("## Trials", markdown)
        comparison_section = markdown.split("## Baseline vs Candidate", 1)[1]
        self.assertIn("| Task | Baseline score | Candidate score |", comparison_section)
        self.assertNotIn("| CDB-S01 |", comparison_section)

    def test_noncompleted_trial_comparison_is_incomplete(self) -> None:
        baseline = compare_drivers.TrialMetrics(
            "CDB-S01", "0.22.2", "a", None, True, 1.0, 100, 10, 4, "completed"
        )
        candidate = compare_drivers.TrialMetrics(
            "CDB-S01",
            "0.23.2",
            "b",
            None,
            False,
            0.8,
            80,
            1,
            1,
            "agent_exit_-15",
        )

        comparison = compare_drivers.compare_pair(baseline, candidate)

        self.assertEqual(comparison["signal"], "incomplete")


if __name__ == "__main__":
    unittest.main()
