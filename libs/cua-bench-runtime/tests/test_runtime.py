from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from cua_bench_runtime import exit_codes
from cua_bench_runtime.adapters.local import adapters as local_adapters
from cua_bench_runtime.canon import digest_file, digest_json
from cua_bench_runtime.engine import (
    _bounded_emergency_cleanup,
    _certification_apparatus,
    _destroy_credential_lease_on_exit,
    _has_certifying_environment_evidence,
    run_trial,
)
from cua_bench_runtime.errors import (
    BudgetExceeded,
    HardAbort,
    TrialInterrupted,
    UsageFailure,
    ValidationFailure,
)
from cua_bench_runtime.events import EventLog
from cua_bench_runtime.explain import inspect_trial, narrative
from cua_bench_runtime.model import AgentOutcome, CleanupReport, EnvironmentHandle, ObserverReport
from cua_bench_runtime.schemas import REPO_ROOT

TASK = REPO_ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
AGENTS = REPO_ROOT / "conformance/agents"
NODE = Path(shutil.which("node") or "").resolve()
NODE_VERSION = (
    subprocess.run(
        [str(NODE), "--version"], capture_output=True, text=True, check=True
    ).stdout.strip()
    if NODE.is_file()
    else ""
)


def generate_signing_key(path: Path) -> None:
    ssh_keygen = r"C:\Windows\System32\OpenSSH\ssh-keygen.exe" if os.name == "nt" else "ssh-keygen"
    subprocess.run(
        [ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(path)],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if os.name == "nt":
        subprocess.run(
            [
                "icacls",
                str(path),
                "/inheritance:r",
                "/grant:r",
                f"{os.environ['USERNAME']}:F",
            ],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )


class RuntimeTests(unittest.TestCase):
    def test_certification_apparatus_maps_protected_tool_contract_evidence(
        self,
    ) -> None:
        digest = "sha256:" + "a" * 64
        facts = {
            "protected_report_digest": digest,
            "protected_tool_contract_validated": True,
            "observed_daemon_tool_schemas_sha256": digest,
        }
        apparatus = _certification_apparatus(
            environment_name="lume-macos-certifying",
            environment_declared_certifying=True,
            apparatus_check=False,
            handle=EnvironmentHandle("test", Path("."), facts),
            cleanup=CleanupReport(ok=True),
            inputs_unchanged=True,
            evaluation=None,
            participation={},
            participation_signature=None,
            policy_receipt=None,
        )
        self.assertEqual(apparatus["protected_report_digest"], digest)
        self.assertIs(apparatus["protected_tool_contract_validated"], True)
        self.assertEqual(apparatus["observed_daemon_tool_schemas_sha256"], digest)

        facts["protected_tool_contract_validated"] = 1
        apparatus = _certification_apparatus(
            environment_name="lume-macos-certifying",
            environment_declared_certifying=True,
            apparatus_check=False,
            handle=EnvironmentHandle("test", Path("."), facts),
            cleanup=CleanupReport(ok=True),
            inputs_unchanged=True,
            evaluation=None,
            participation={},
            participation_signature=None,
            policy_receipt=None,
        )
        self.assertIs(apparatus["protected_tool_contract_validated"], False)

    def test_certifying_macos_uses_canonical_log_not_optional_report(self) -> None:
        facts = {
            "certifying": True,
            "human_input_channel": "closed-no-vnc",
            "task_store_mode": "protected-console-only",
            "mediator_enforcer": "guest-root-cdb-helper",
            "vm_stopped_before_collection": True,
            "protected_collection_read_only": True,
            "protected_log_digest": "sha256:" + "a" * 64,
            "protected_report_digest": None,
        }
        handle = EnvironmentHandle("lume-macos-certifying", Path("."), facts)
        self.assertTrue(
            _has_certifying_environment_evidence(
                "lume-macos-certifying", handle, CleanupReport(ok=True)
            )
        )
        facts["protected_log_digest"] = None
        facts["protected_report_digest"] = "sha256:" + "b" * 64
        self.assertFalse(
            _has_certifying_environment_evidence(
                "lume-macos-certifying", handle, CleanupReport(ok=True)
            )
        )

    def participation_task(self, root: Path) -> Path:
        task_root = root / "participation-task"
        shutil.copytree(TASK.parent, task_root)
        manifest = task_root / "task.cuabench.json"
        task = json.loads(manifest.read_text(encoding="utf-8"))
        task["schema_version"] = "0.2.0"
        task["version"] = "1.1.0"
        task["participation_requirements"] = [
            {
                "id": "participation.synthetic.confirm",
                "target": {
                    "application_id": "application.synthetic-ui",
                    "surface_id": "surface.synthetic.confirmation",
                },
                "sequence": [
                    {
                        "kind": "act",
                        "required_facts": {"operation": "confirm"},
                    },
                    {
                        "kind": "readback",
                        "required_facts": {"state": "confirmed"},
                    },
                ],
            }
        ]
        manifest.write_text(json.dumps(task), encoding="utf-8")
        return manifest

    def run_agent(
        self,
        directory: str,
        name: str,
        *,
        trial_id: str,
        timeout: float | None = None,
        environment: str = "local",
    ) -> tuple[int, Path, dict]:
        return run_trial(
            task_path=TASK,
            agent_command=AGENTS / name,
            out=Path(directory),
            trial_id=trial_id,
            timeout_seconds=timeout,
            environment_name=environment,
        )

    def test_success_is_explainable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = self.run_agent(directory, "reference_ok.py", trial_id="ok")
            self.assertEqual(code, exit_codes.OK)
            self.assertTrue(result["evaluation"]["passed"])
            report = inspect_trial(trial)
            self.assertTrue(report["verified"])
            self.assertEqual(report["states"][-1], "done")

    def test_apparatus_check_is_bound_and_explainable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = run_trial(
                task_path=TASK,
                agent_command=AGENTS / "reference_ok.py",
                out=Path(directory),
                trial_id="apparatus-check",
                apparatus_check=True,
            )
            self.assertEqual(code, exit_codes.OK)
            self.assertTrue(result["apparatus_check"])
            self.assertFalse(result["certifying"])
            report = inspect_trial(trial)
            self.assertTrue(report["apparatus_check"])
            self.assertIn("apparatus check: yes", narrative(report))

            result["apparatus_check"] = False
            (trial / "result.json").write_text(json.dumps(result), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "apparatus-check"):
                inspect_trial(trial)

    def test_agent_failure_is_a_graded_result(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, _trial, result = self.run_agent(directory, "reference_fail.py", trial_id="fail")
            self.assertEqual(code, exit_codes.OK)
            self.assertFalse(result["evaluation"]["passed"])
            self.assertTrue(result["certifying"])
            self.assertEqual(result["status"], "completed")

    def test_terminal_provider_failure_is_an_infrastructure_result(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            environment, _agent, observer, evaluator = local_adapters(
                "local",
                AGENTS / "reference_ok.py",
                digest_file(AGENTS / "reference_ok.py"),
                TASK.parent / "evaluator/check.py",
                digest_file(TASK.parent / "evaluator/check.py"),
            )

            class ProviderFailureAgent:
                name = "provider-failure-agent"

                def run(self, context, handle, interrupt, timeout_seconds):
                    successful = _agent.run(context, handle, interrupt, timeout_seconds)
                    return AgentOutcome(
                        completed=False,
                        exit_code=1,
                        duration_ms=successful.duration_ms,
                        artifacts=successful.artifacts,
                        terminal_failure="authentication_unavailable",
                    )

            with mock.patch(
                "cua_bench_runtime.engine.adapters",
                return_value=(
                    environment,
                    ProviderFailureAgent(),
                    observer,
                    evaluator,
                ),
            ):
                code, trial, result = run_trial(
                    task_path=TASK,
                    agent_command=AGENTS / "reference_ok.py",
                    out=Path(directory),
                    trial_id="provider-failure",
                )

            self.assertEqual(code, exit_codes.HARNESS)
            self.assertEqual(result["status"], "infrastructure_error", result)
            self.assertFalse(result["evaluation"]["passed"])
            self.assertTrue(result["cleanup_ok"])
            self.assertTrue(inspect_trial(trial)["verified"])

    def test_malformed_agent_output_is_a_graded_result(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, _trial, result = self.run_agent(
                directory, "reference_malformed.py", trial_id="malformed"
            )
            self.assertEqual(code, exit_codes.OK)
            self.assertFalse(result["evaluation"]["passed"])

    def test_timeout_still_cleans_up(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = self.run_agent(
                directory, "reference_hang.py", trial_id="timeout", timeout=0.2
            )
            self.assertEqual(code, exit_codes.TIMEOUT)
            self.assertTrue(result["cleanup_ok"])
            self.assertTrue((trial / "artifacts/cleanup.json").is_file())
            self.assertTrue(inspect_trial(trial)["verified"])

    def test_hard_abort_cleans_pending_and_ready_environments_once(self) -> None:
        for phase in ("setup", "agent"):
            with (
                self.subTest(phase=phase),
                tempfile.TemporaryDirectory() as directory,
            ):
                environment, agent, observer, evaluator = local_adapters(
                    "local",
                    AGENTS / "reference_ok.py",
                    digest_json({"unused": True}),
                    AGENTS / "reference_ok.py",
                    digest_json({"unused": False}),
                )
                cleanup = mock.Mock(return_value=CleanupReport(ok=True))
                environment.cleanup = cleanup
                environment.export = mock.Mock(wraps=environment.export)
                evaluator.evaluate = mock.Mock(wraps=evaluator.evaluate)

                if phase == "setup":
                    environment.setup = mock.Mock(side_effect=HardAbort("hard abort during setup"))
                else:

                    class AbortingAgent:
                        name = "aborting-agent"

                        def run(self, context, handle, interrupt, timeout_seconds):
                            del context, handle, interrupt, timeout_seconds
                            raise HardAbort("hard abort after setup")

                    agent = AbortingAgent()

                created_events: list[EventLog] = []

                def event_log(*args: object, **kwargs: object) -> EventLog:
                    log = EventLog(*args, **kwargs)
                    created_events.append(log)
                    return log

                with (
                    mock.patch(
                        "cua_bench_runtime.engine.adapters",
                        return_value=(environment, agent, observer, evaluator),
                    ),
                    mock.patch("cua_bench_runtime.engine.EventLog", side_effect=event_log),
                    self.assertRaisesRegex(HardAbort, "hard abort"),
                ):
                    run_trial(
                        task_path=TASK,
                        agent_command=AGENTS / "reference_ok.py",
                        out=Path(directory),
                        trial_id=f"hard-abort-{phase}",
                    )

                cleanup.assert_called_once()
                if phase == "setup":
                    self.assertIsNone(cleanup.call_args.args[1])
                else:
                    self.assertIsNotNone(cleanup.call_args.args[1])
                self.assertTrue(created_events[0].closed)
                environment.export.assert_not_called()
                evaluator.evaluate.assert_not_called()

    def test_hard_abort_cleanup_failure_never_masks_abort(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            environment, agent, observer, evaluator = local_adapters(
                "local",
                AGENTS / "reference_ok.py",
                digest_json({"unused": True}),
                AGENTS / "reference_ok.py",
                digest_json({"unused": False}),
            )
            environment.setup = mock.Mock(side_effect=HardAbort("primary abort"))
            environment.cleanup = mock.Mock(side_effect=RuntimeError("cleanup failed"))
            with (
                mock.patch(
                    "cua_bench_runtime.engine.adapters",
                    return_value=(environment, agent, observer, evaluator),
                ),
                self.assertRaisesRegex(HardAbort, "primary abort"),
            ):
                run_trial(
                    task_path=TASK,
                    agent_command=AGENTS / "reference_ok.py",
                    out=Path(directory),
                    trial_id="hard-abort-cleanup-failure",
                )
            environment.cleanup.assert_called_once()

    def test_hard_abort_destroys_credential_lease_on_scope_exit(self) -> None:
        lease = mock.Mock()
        with self.assertRaisesRegex(HardAbort, "primary abort"):
            with _destroy_credential_lease_on_exit(lease) as destroy:
                destroy()
                raise HardAbort("primary abort")
        lease.destroy.assert_called_once_with()

    @unittest.skipUnless(
        hasattr(signal, "setitimer") and hasattr(signal, "SIGALRM"),
        "POSIX interval timers are required",
    )
    def test_hard_abort_cleanup_is_bounded_without_a_background_thread(self) -> None:
        environment = mock.Mock(cleanup_timeout_seconds=10.0)
        environment.cleanup.side_effect = lambda *args, **kwargs: time.sleep(1.0)
        started = time.monotonic()
        with (
            mock.patch("cua_bench_runtime.engine.HARD_ABORT_CLEANUP_TIMEOUT_SECONDS", 0.03),
            self.assertRaises(BaseException) as raised,
        ):
            _bounded_emergency_cleanup(environment, mock.sentinel.context, None)
        elapsed = time.monotonic() - started
        self.assertEqual(type(raised.exception).__name__, "_EmergencyCleanupTimeout")
        self.assertLess(elapsed, 0.3)
        self.assertEqual(environment.cleanup.call_args.kwargs["timeout_seconds"], 0.03)

    def test_hard_abort_cleans_up_with_bounded_daemon_thread_without_posix_timer(
        self,
    ) -> None:
        environment = mock.Mock(cleanup_timeout_seconds=1.0)
        with mock.patch("cua_bench_runtime.engine.signal.setitimer", None, create=True):
            _bounded_emergency_cleanup(environment, mock.sentinel.context, None)
        environment.cleanup.assert_called_once_with(
            mock.sentinel.context,
            None,
            timeout_seconds=1.0,
        )

    def test_hard_abort_cleanup_without_posix_timer_is_bounded(self) -> None:
        environment = mock.Mock(cleanup_timeout_seconds=10.0)
        environment.cleanup.side_effect = lambda *args, **kwargs: time.sleep(1.0)
        started = time.monotonic()
        with (
            mock.patch("cua_bench_runtime.engine.HARD_ABORT_CLEANUP_TIMEOUT_SECONDS", 0.03),
            mock.patch("cua_bench_runtime.engine.signal.setitimer", None, create=True),
        ):
            _bounded_emergency_cleanup(environment, mock.sentinel.context, None)
        elapsed = time.monotonic() - started
        environment.cleanup.assert_called_once()
        self.assertLess(elapsed, 0.3)

    def test_hard_abort_honors_bounded_environment_containment_window(self) -> None:
        environment = mock.Mock(
            cleanup_timeout_seconds=180.0,
            hard_abort_cleanup_timeout_seconds=20.0,
        )
        with mock.patch("cua_bench_runtime.engine.MAX_HARD_ABORT_CLEANUP_TIMEOUT_SECONDS", 0.07):
            _bounded_emergency_cleanup(environment, mock.sentinel.context, None)
        environment.cleanup.assert_called_once_with(
            mock.sentinel.context,
            None,
            timeout_seconds=0.07,
        )

    def test_agent_budget_and_interrupt_still_export_workspace(self) -> None:
        cases = (
            (BudgetExceeded("budget exhausted"), exit_codes.BUDGET, "cost_limit"),
            (TrialInterrupted("stop requested"), exit_codes.INTERRUPTED, "interrupted"),
        )
        for index, (failure, expected_code, expected_status) in enumerate(cases):
            with (
                self.subTest(status=expected_status),
                tempfile.TemporaryDirectory() as directory,
            ):
                environment, _agent, observer, evaluator = local_adapters(
                    "local",
                    AGENTS / "reference_ok.py",
                    digest_json({"unused": True}),
                    AGENTS / "reference_ok.py",
                    digest_json({"unused": False}),
                )
                exported: list[object] = []
                original_export = environment.export

                def export(context, handle, outcome):
                    exported.append(outcome)
                    return original_export(context, handle, outcome)

                environment.export = export

                class FailingAgent:
                    name = "failing-agent"

                    def run(self, context, handle, interrupt, timeout_seconds):
                        del context, handle, interrupt, timeout_seconds
                        raise failure

                with mock.patch(
                    "cua_bench_runtime.engine.adapters",
                    return_value=(environment, FailingAgent(), observer, evaluator),
                ):
                    code, trial, result = run_trial(
                        task_path=TASK,
                        agent_command=AGENTS / "reference_ok.py",
                        out=Path(directory),
                        trial_id=f"exceptional-export-{index}",
                    )

                self.assertEqual(code, expected_code)
                self.assertEqual(result["status"], expected_status)
                self.assertEqual(len(exported), 1)
                self.assertFalse(exported[0].completed)
                events = (trial / "events.ndjson").read_text(encoding="utf-8")
                self.assertIn('"after_exception":true', events)
                self.assertIn(f'"error_status":"{expected_status}"', events)

    def test_ineligible_cb_errors_do_not_export_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            environment, _agent, observer, evaluator = local_adapters(
                "local",
                AGENTS / "reference_ok.py",
                digest_json({"unused": True}),
                AGENTS / "reference_ok.py",
                digest_json({"unused": False}),
            )
            environment.export = mock.Mock(wraps=environment.export)

            class InvalidAgent:
                name = "invalid-agent"

                def run(self, context, handle, interrupt, timeout_seconds):
                    del context, handle, interrupt, timeout_seconds
                    raise ValidationFailure("invalid after setup")

            with mock.patch(
                "cua_bench_runtime.engine.adapters",
                return_value=(environment, InvalidAgent(), observer, evaluator),
            ):
                code, _trial, result = run_trial(
                    task_path=TASK,
                    agent_command=AGENTS / "reference_ok.py",
                    out=Path(directory),
                    trial_id="ineligible-export",
                )

            self.assertEqual(code, exit_codes.VALIDATION)
            self.assertEqual(result["status"], "validation_error")
            environment.export.assert_not_called()

    def test_pre_setup_budget_failure_does_not_export_or_mask_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            environment, agent, observer, evaluator = local_adapters(
                "local",
                AGENTS / "reference_ok.py",
                digest_json({"unused": True}),
                AGENTS / "reference_ok.py",
                digest_json({"unused": False}),
            )
            environment.export = mock.Mock(wraps=environment.export)

            def fail_setup(context):
                del context
                raise BudgetExceeded("budget exhausted before preparation")

            environment.setup = fail_setup
            with mock.patch(
                "cua_bench_runtime.engine.adapters",
                return_value=(environment, agent, observer, evaluator),
            ):
                code, _trial, result = run_trial(
                    task_path=TASK,
                    agent_command=AGENTS / "reference_ok.py",
                    out=Path(directory),
                    trial_id="pre-setup-budget",
                )

            self.assertEqual(code, exit_codes.BUDGET)
            self.assertEqual(result["original_status"], "cost_limit")
            self.assertTrue(result["cleanup_ok"])
            environment.export.assert_not_called()

    def test_exceptional_export_failure_preserves_budget_and_cleanup_precedence(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            environment, _agent, observer, evaluator = local_adapters(
                "local-fail-cleanup",
                AGENTS / "reference_ok.py",
                digest_json({"unused": True}),
                AGENTS / "reference_ok.py",
                digest_json({"unused": False}),
            )
            environment.export = mock.Mock(
                side_effect=ValidationFailure("exceptional export failed")
            )

            class BudgetAgent:
                name = "budget-agent"

                def run(self, context, handle, interrupt, timeout_seconds):
                    del context, handle, interrupt, timeout_seconds
                    raise BudgetExceeded("budget exhausted")

            with mock.patch(
                "cua_bench_runtime.engine.adapters",
                return_value=(environment, BudgetAgent(), observer, evaluator),
            ):
                code, trial, result = run_trial(
                    task_path=TASK,
                    agent_command=AGENTS / "reference_ok.py",
                    out=Path(directory),
                    trial_id="failed-exceptional-export",
                )

            self.assertEqual(code, exit_codes.CLEANUP)
            self.assertEqual(result["original_status"], "cost_limit")
            self.assertEqual(result["status"], "cleanup_error")
            self.assertFalse(result["cleanup_ok"])
            self.assertIn(
                '"workspace_export_failed"',
                (trial / "events.ndjson").read_text(encoding="utf-8"),
            )

    def test_malformed_participation_on_timeout_still_cleans_up(self) -> None:
        class MalformedObserver:
            name = "malformed-observer"

            def start(self, context, handle, requirements) -> None:
                del context, handle, requirements

            def finish(self, context, handle, outcome) -> ObserverReport:
                del context, handle, outcome
                return ObserverReport(
                    name=self.name,
                    trust="certifying",
                    events=({"unsupported": "observer-controlled"},),
                )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def with_observer(*args: object, **kwargs: object):
                environment, agent, _observer, evaluator = local_adapters(*args, **kwargs)
                return environment, agent, MalformedObserver(), evaluator

            with mock.patch("cua_bench_runtime.engine.adapters", side_effect=with_observer):
                code, trial, result = run_trial(
                    task_path=self.participation_task(root),
                    agent_command=AGENTS / "reference_hang.py",
                    out=root / "trials",
                    trial_id="malformed-timeout",
                    timeout_seconds=0.2,
                )
            self.assertEqual(code, exit_codes.TIMEOUT)
            self.assertEqual(result["status"], "timeout")
            self.assertTrue(result["cleanup_ok"])
            self.assertEqual(result["participation"]["status"], "unavailable")
            self.assertTrue((trial / "result.json").is_file())
            self.assertTrue((trial / "artifacts/cleanup.json").is_file())
            events = (trial / "events.ndjson").read_text(encoding="utf-8")
            self.assertIn('"phase":"verify"', events)
            self.assertTrue(inspect_trial(trial)["verified"])

    def test_cleanup_failure_has_precedence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, _trial, result = self.run_agent(
                directory,
                "reference_ok.py",
                trial_id="cleanup",
                environment="local-fail-cleanup",
            )
            self.assertEqual(code, exit_codes.CLEANUP)
            self.assertEqual(result["original_status"], "completed")
            self.assertFalse(result["cleanup_ok"])

    def test_trial_directory_is_never_reused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.run_agent(directory, "reference_ok.py", trial_id="same")
            with self.assertRaises(UsageFailure):
                self.run_agent(directory, "reference_ok.py", trial_id="same")

    def test_trial_id_cannot_escape_output_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            for trial_id in ("../escape", "/absolute", "nested/id", "x" * 65):
                with self.subTest(trial_id=trial_id), self.assertRaises(UsageFailure):
                    self.run_agent(directory, "reference_ok.py", trial_id=trial_id)

    def test_nonpositive_timeout_is_rejected(self) -> None:
        with (
            tempfile.TemporaryDirectory() as directory,
            self.assertRaises(UsageFailure),
        ):
            self.run_agent(
                directory,
                "reference_ok.py",
                trial_id="zero-timeout",
                timeout=0,
            )

    def test_evaluator_outputs_are_not_agent_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = self.run_agent(
                directory, "reference_plant_evaluator.py", trial_id="plant"
            )
            self.assertEqual(code, exit_codes.OK)
            self.assertFalse(result["evaluation"]["passed"])
            self.assertTrue((trial / "artifacts/evaluation.json").is_file())
            self.assertTrue((trial / "evaluator/evaluation.json").is_file())

    def test_noisy_agent_is_still_graded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = self.run_agent(directory, "reference_noisy.py", trial_id="noisy")
            self.assertEqual(code, exit_codes.OK)
            self.assertEqual(result["status"], "completed")
            self.assertFalse(result["evaluation"]["passed"])
            self.assertIn(
                '"output_exceeded":true',
                (trial / "events.ndjson").read_text(encoding="utf-8"),
            )

    def test_local_adapter_rejects_unenforced_staging(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            task_root = root / "task"
            shutil.copytree(TASK.parent, task_root)
            manifest = task_root / "task.cuabench.json"
            task = json.loads(manifest.read_text(encoding="utf-8"))
            task["evaluator"]["staging"] = "agent-inaccessible"
            manifest.write_text(json.dumps(task), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "agent-visible"):
                run_trial(
                    task_path=manifest,
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "trials",
                    trial_id="unsupported-staging",
                )

    def test_shell_only_success_scores_but_cannot_certify_gui_task(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code, trial, result = run_trial(
                task_path=self.participation_task(root),
                agent_command=AGENTS / "reference_ok.py",
                out=root / "trials",
                trial_id="shell-only",
            )
            self.assertEqual(code, exit_codes.OK)
            self.assertTrue(result["evaluation"]["passed"])
            self.assertEqual(result["evaluation"]["score"], 1.0)
            self.assertEqual(result["participation"]["status"], "unavailable")
            self.assertIsNone(result["participation"]["passed"])
            self.assertFalse(result["certifying"])
            self.assertTrue(inspect_trial(trial)["verified"])

    def test_participation_receipt_is_signed_by_pinned_host_key(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key = root / "participation-key"
            generate_signing_key(key)

            def protected_adapters(*args: object, **kwargs: object):
                local_args = ("local", *args[1:])
                return local_adapters(*local_args, **kwargs)

            with (
                mock.patch(
                    "cua_bench_runtime.engine.CERTIFYING_ENVIRONMENT_ADAPTERS",
                    frozenset({"protected-test"}),
                ),
                mock.patch("cua_bench_runtime.engine.adapters", side_effect=protected_adapters),
            ):
                _code, trial, result = run_trial(
                    task_path=self.participation_task(root),
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "trials",
                    trial_id="signed-participation",
                    environment_name="protected-test",
                    participation_signing_key=key,
                    participation_verifier_key=key.with_suffix(".pub"),
                )
            with (
                mock.patch(
                    "cua_bench_runtime.engine.CERTIFYING_ENVIRONMENT_ADAPTERS",
                    frozenset({"protected-test"}),
                ),
                self.assertRaisesRegex(UsageFailure, "schema 0.2.0"),
            ):
                run_trial(
                    task_path=TASK,
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "legacy-trials",
                    trial_id="signed-legacy",
                    environment_name="protected-test",
                    participation_signing_key=key,
                    participation_verifier_key=key.with_suffix(".pub"),
                )
            descriptor = result["participation_signature"]
            self.assertIsNotNone(descriptor)
            self.assertTrue((trial / descriptor["path"]).is_file())
            self.assertTrue(inspect_trial(trial)["verified"])
            self.assertFalse(any(path.name == key.name for path in (trial / "inputs").rglob("*")))

            artifact = trial / descriptor["path"]
            document = json.loads(artifact.read_text(encoding="utf-8"))
            document["body"]["receipt"]["status"] = "passed"
            artifact.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "artifact digest"):
                inspect_trial(trial)

    def test_signed_participation_is_rejected_for_local_and_legacy_runs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key = root / "participation-key"
            generate_signing_key(key)
            with self.assertRaisesRegex(UsageFailure, "certifying environment"):
                run_trial(
                    task_path=self.participation_task(root),
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "local-trials",
                    trial_id="signed-local",
                    participation_signing_key=key,
                    participation_verifier_key=key.with_suffix(".pub"),
                )

    def test_agent_cannot_redirect_signed_receipt_write_with_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key = root / "participation-key"
            generate_signing_key(key)
            target = root / "must-not-change"
            target.write_text("sentinel", encoding="utf-8")
            agent = root / "symlink-agent.py"
            agent.write_text(
                "\n".join(
                    (
                        "import argparse, json",
                        "from pathlib import Path",
                        "p=argparse.ArgumentParser()",
                        "p.add_argument('--workspace',type=Path,required=True)",
                        "p.add_argument('--artifacts',type=Path,required=True)",
                        "a=p.parse_args()",
                        f"(a.artifacts/'participation-receipt.sshsig.json').symlink_to({str(target)!r})",
                        "d=json.loads((a.workspace/'input.json').read_text())",
                        "(a.artifacts/'output.json').write_text(json.dumps({'value':d['parameters']['nonce'][::-1]}))",
                    )
                )
                + "\n",
                encoding="utf-8",
            )

            def protected_adapters(*args: object, **kwargs: object):
                local_args = ("local", *args[1:])
                return local_adapters(*local_args, **kwargs)

            with (
                mock.patch(
                    "cua_bench_runtime.engine.CERTIFYING_ENVIRONMENT_ADAPTERS",
                    frozenset({"protected-test"}),
                ),
                mock.patch("cua_bench_runtime.engine.adapters", side_effect=protected_adapters),
            ):
                _code, trial, result = run_trial(
                    task_path=self.participation_task(root),
                    agent_command=agent,
                    out=root / "trials",
                    trial_id="receipt-symlink",
                    environment_name="protected-test",
                    participation_signing_key=key,
                    participation_verifier_key=key.with_suffix(".pub"),
                )
            self.assertEqual(target.read_text(encoding="utf-8"), "sentinel")
            self.assertIsNone(result["participation_signature"])
            self.assertFalse(result["certifying"])
            self.assertTrue(inspect_trial(trial)["verified"])
            self.assertIn(
                '"type":"participation_signature_error"',
                (trial / "events.ndjson").read_text(encoding="utf-8"),
            )

    def test_output_schema_version_follows_task_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cases = (
                (TASK, "schema-v01", "0.1.0"),
                (self.participation_task(root), "schema-v02", "0.2.0"),
            )
            for task_path, trial_id, expected in cases:
                with self.subTest(schema_version=expected):
                    _code, trial, result = run_trial(
                        task_path=task_path,
                        agent_command=AGENTS / "reference_ok.py",
                        out=root / "trials",
                        trial_id=trial_id,
                    )
                    config = json.loads((trial / "config.json").read_text(encoding="utf-8"))
                    manifest = json.loads(
                        (trial / "inputs.manifest.json").read_text(encoding="utf-8")
                    )
                    self.assertEqual(config["schema_version"], expected)
                    self.assertEqual(manifest["schema_version"], expected)
                    self.assertEqual(result["schema_version"], expected)

    def test_explain_accepts_legacy_v01_result_without_participation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            _code, trial, result = self.run_agent(
                directory, "reference_ok.py", trial_id="legacy-v01"
            )
            result.pop("participation")
            result.pop("environment_certifying")
            (trial / "result.json").write_text(json.dumps(result), encoding="utf-8")

            report = inspect_trial(trial)

            self.assertTrue(report["verified"])
            self.assertIsNone(report["participation"])
            self.assertTrue(report["certifying"])
            self.assertIn(
                "driver participation: not recorded (schema 0.1.0)",
                narrative(report),
            )

    def test_runtime_owned_correlated_driver_events_certify(self) -> None:
        class Observer:
            name = "conformance-driver-proxy"

            def start(self, context, handle, requirements) -> None:
                del context, handle
                self.requirements = requirements

            def finish(self, context, handle, outcome) -> ObserverReport:
                del context, handle, outcome
                target = {
                    "application_id": "application.synthetic-ui",
                    "surface_id": "surface.synthetic.confirmation",
                    "platform_application_id": "com.example.synthetic",
                    "process_id": 91,
                    "window_id": "opaque-window",
                }
                common = {
                    "provider": {"id": "conformance.driver", "version": "1"},
                    "target": target,
                    "correlation_id": "runtime-correlation-1",
                }
                return ObserverReport(
                    name=self.name,
                    trust="certifying",
                    events=(
                        {
                            **common,
                            "capability_class": "pointer_input",
                            "kind": "act",
                            "fact_digests": {"operation": digest_json("confirm")},
                        },
                        {
                            **common,
                            "capability_class": "accessibility_observation",
                            "kind": "readback",
                            "fact_digests": {"state": digest_json("confirmed")},
                        },
                    ),
                )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            task = self.participation_task(root)

            def with_observer(*args: object, **kwargs: object):
                local_args = ("local", *args[1:])
                environment, agent, _observer, evaluator = local_adapters(*local_args, **kwargs)
                return environment, agent, Observer(), evaluator

            with (
                mock.patch(
                    "cua_bench_runtime.engine.CERTIFYING_ENVIRONMENT_ADAPTERS",
                    frozenset({"protected-test"}),
                ),
                mock.patch("cua_bench_runtime.engine.adapters", side_effect=with_observer),
            ):
                code, trial, result = run_trial(
                    task_path=task,
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "trials",
                    trial_id="driver-pass",
                    environment_name="protected-test",
                )
            self.assertEqual(code, exit_codes.OK)
            self.assertTrue(result["evaluation"]["passed"])
            self.assertEqual(result["participation"]["status"], "passed")
            self.assertTrue(result["participation"]["passed"])
            self.assertTrue(result["certifying"])
            self.assertTrue(inspect_trial(trial)["verified"])

            with mock.patch("cua_bench_runtime.engine.adapters", side_effect=with_observer):
                _code, local_trial, local_result = run_trial(
                    task_path=task,
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "trials",
                    trial_id="driver-pass-local",
                    environment_name="local",
                )
            self.assertTrue(local_result["participation"]["passed"])
            self.assertFalse(local_result["environment_certifying"])
            self.assertFalse(local_result["certifying"])
            self.assertTrue(inspect_trial(local_trial)["verified"])

    def test_unrelated_runtime_owned_gui_events_do_not_certify(self) -> None:
        class UnrelatedObserver:
            name = "conformance-driver-proxy"

            def start(self, context, handle, requirements) -> None:
                del context, handle, requirements

            def finish(self, context, handle, outcome) -> ObserverReport:
                del context, handle, outcome
                return ObserverReport(
                    name=self.name,
                    trust="certifying",
                    events=(
                        {
                            "provider": {"id": "conformance.driver"},
                            "capability_class": "pointer_input",
                            "target": {
                                "application_id": "application.calculator",
                                "surface_id": "surface.calculator.main",
                                "platform_application_id": "com.example.calculator",
                                "process_id": 92,
                                "window_id": "opaque-calculator-window",
                            },
                            "kind": "act",
                            "correlation_id": "unrelated-1",
                            "fact_digests": {"operation": digest_json("confirm")},
                        },
                    ),
                )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def with_observer(*args: object, **kwargs: object):
                environment, agent, _observer, evaluator = local_adapters(*args, **kwargs)
                return environment, agent, UnrelatedObserver(), evaluator

            with mock.patch("cua_bench_runtime.engine.adapters", side_effect=with_observer):
                _code, trial, result = run_trial(
                    task_path=self.participation_task(root),
                    agent_command=AGENTS / "reference_ok.py",
                    out=root / "trials",
                    trial_id="unrelated-gui",
                )
            self.assertTrue(result["evaluation"]["passed"])
            self.assertEqual(result["participation"]["status"], "failed")
            self.assertFalse(result["certifying"])
            self.assertTrue(inspect_trial(trial)["verified"])

    def test_materialized_commands_are_executed_after_sources_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            task_root = root / "task"
            shutil.copytree(TASK.parent, task_root)
            agent = root / "reference_ok.py"
            shutil.copyfile(AGENTS / "reference_ok.py", agent)
            evaluator = task_root / "evaluator/check.py"

            def mutate_sources(*args: object, **kwargs: object):
                agent.write_text("raise SystemExit(91)\n", encoding="utf-8")
                evaluator.write_text("raise SystemExit(92)\n", encoding="utf-8")
                return local_adapters(*args, **kwargs)

            with mock.patch("cua_bench_runtime.engine.adapters", side_effect=mutate_sources):
                code, _trial, result = run_trial(
                    task_path=task_root / "task.cuabench.json",
                    agent_command=agent,
                    out=root / "trials",
                    trial_id="snapshots",
                )
            self.assertEqual(code, exit_codes.OK)
            self.assertTrue(result["evaluation"]["passed"])

    def test_explain_rejects_corruption(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            _code, trial, _result = self.run_agent(directory, "reference_ok.py", trial_id="tamper")
            event_path = trial / "events.ndjson"
            event_path.write_bytes(event_path.read_bytes() + b"x")
            with self.assertRaises(ValidationFailure):
                inspect_trial(trial)

    def test_explain_rejects_environment_certification_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            _code, trial, result = self.run_agent(
                directory, "reference_ok.py", trial_id="certification-tamper"
            )
            result["environment_certifying"] = not result["environment_certifying"]
            (trial / "result.json").write_text(json.dumps(result), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "environment certification"):
                inspect_trial(trial)


if __name__ == "__main__":
    unittest.main()
