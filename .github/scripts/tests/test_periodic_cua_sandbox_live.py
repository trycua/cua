"""Contract tests for the periodic Cua Sandbox live Fleet workflow."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import yaml


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github/workflows/periodic-cua-sandbox-live.yml"
SANDBOX_ROOT = REPO_ROOT / "libs/python/cua-sandbox"
LIVE_TEST = SANDBOX_ROOT / "tests/live/test_fleet_ephemeral.py"


class TestPeriodicCuaSandboxLive(unittest.TestCase):
    @staticmethod
    def workflow() -> dict[str, object]:
        # BaseLoader keeps GitHub Actions' `on` key as a string.
        return yaml.load(WORKFLOW.read_text(), Loader=yaml.BaseLoader)

    @staticmethod
    def steps_by_name(job: dict[str, object]) -> dict[str, dict[str, object]]:
        return {
            step["name"]: step for step in job["steps"] if isinstance(step, dict) and "name" in step
        }

    def run_prepare_matrix(self, event_name: str, requested_lane: str) -> dict[str, object]:
        prepare_script = self.workflow()["jobs"]["prepare"]["steps"][0]["run"]
        with tempfile.TemporaryDirectory() as temporary_directory:
            github_output = Path(temporary_directory) / "github-output"
            environment = os.environ | {
                "EVENT_NAME": event_name,
                "REQUESTED_LANE": requested_lane,
                "GITHUB_OUTPUT": str(github_output),
            }
            subprocess.run(
                ["bash", "-c", prepare_script],
                env=environment,
                check=True,
                capture_output=True,
                text=True,
            )
            outputs = dict(
                line.split("=", 1) for line in github_output.read_text().splitlines() if "=" in line
            )
        return json.loads(outputs["matrix"])

    def test_trigger_and_lane_structure(self) -> None:
        workflow = self.workflow()
        triggers = workflow["on"]
        self.assertEqual(triggers["schedule"], [{"cron": "7/15 * * * *"}])
        self.assertEqual(triggers["push"]["branches"], ["main"])
        self.assertEqual(
            triggers["push"]["paths"],
            [
                "libs/python/cua-sandbox/**",
                "libs/python/cua-fleet/**",
                ".github/workflows/periodic-cua-sandbox-live.yml",
                ".github/scripts/tests/test_periodic_cua_sandbox_live.py",
            ],
        )
        inputs = triggers["workflow_dispatch"]["inputs"]
        self.assertEqual(
            inputs["lane"]["options"],
            ["both", "main-source", "published-package"],
        )
        self.assertEqual(inputs["force_failure"]["type"], "boolean")

        prepare = workflow["jobs"]["prepare"]
        self.assertEqual(prepare["outputs"]["matrix"], "${{ steps.matrix.outputs.matrix }}")
        prepare_script = prepare["steps"][0]["run"]
        self.assertIn('[[ "$EVENT_NAME" == "push" ]]', prepare_script)
        self.assertIn('"lane":"main-source"', prepare_script)
        self.assertIn('"lane":"published-package"', prepare_script)

    def test_prepare_matrix_selects_lanes_for_each_trigger(self) -> None:
        expected_matrices = {
            ("push", ""): {"include": [{"lane": "main-source"}]},
            ("push", "both"): {"include": [{"lane": "main-source"}]},
            ("push", "published-package"): {"include": [{"lane": "main-source"}]},
            ("schedule", ""): {
                "include": [
                    {"lane": "main-source"},
                    {"lane": "published-package"},
                ]
            },
            ("schedule", "main-source"): {
                "include": [
                    {"lane": "main-source"},
                    {"lane": "published-package"},
                ]
            },
            ("workflow_dispatch", "both"): {
                "include": [
                    {"lane": "main-source"},
                    {"lane": "published-package"},
                ]
            },
            ("workflow_dispatch", "main-source"): {"include": [{"lane": "main-source"}]},
            ("workflow_dispatch", "published-package"): {
                "include": [{"lane": "published-package"}]
            },
        }

        for inputs, expected_matrix in expected_matrices.items():
            with self.subTest(event_name=inputs[0], requested_lane=inputs[1]):
                self.assertEqual(self.run_prepare_matrix(*inputs), expected_matrix)

    def test_docs_describe_the_remediated_workflow(self) -> None:
        docs = (
            REPO_ROOT / "docs/superpowers/specs/2026-08-09-periodic-cua-sandbox-live-e2e-design.md",
            REPO_ROOT / "docs/superpowers/plans/2026-08-09-periodic-cua-sandbox-live-e2e.md",
        )
        required_contract = (
            "libs/python/cua-fleet/**",
            "Check Fleet OAuth credentials",
            "periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}",
            "Prepare isolated live test suite",
            "CUA_LIVE_E2E_TEST_ROOT",
            'tee -a "$GITHUB_OUTPUT"',
            "Write controlled failure diagnostics",
            "persistent reconciled resources",
            "claim-only cleanup",
            "cua-live-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}",
        )
        stale_contract = (
            "Concurrency is scoped\nper lane",
            "python - <<'PY' >> \"$GITHUB_OUTPUT\"",
            "Emergency namespace cleanup",
            "namespace leak",
            "automatic namespace cleanup",
            "leave no test namespace behind",
            "no remaining namespace",
            "namespace is absent",
        )

        for document in docs:
            content = document.read_text()
            with self.subTest(document=document):
                for expected in required_contract:
                    self.assertIn(expected, content)
                for stale in stale_contract:
                    self.assertNotIn(stale, content)

    def test_live_job_security_and_execution_structure(self) -> None:
        workflow = self.workflow()
        live = workflow["jobs"]["live"]
        steps = self.steps_by_name(live)

        self.assertEqual(live["timeout-minutes"], "25")
        self.assertEqual(
            live["strategy"]["matrix"],
            "${{ fromJSON(needs.prepare.outputs.matrix) }}",
        )
        self.assertEqual(
            live["concurrency"]["group"],
            "periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}",
        )
        self.assertEqual(
            live["concurrency"]["cancel-in-progress"],
            "${{ github.event_name == 'schedule' }}",
        )
        self.assertEqual(live["env"]["CUA_CLIENT_ID"], "${{ secrets.CUA_CLIENT_ID }}")
        self.assertEqual(
            live["env"]["CUA_CLIENT_SECRET"],
            "${{ secrets.CUA_CLIENT_SECRET }}",
        )
        self.assertNotIn("CUA_API_KEY", live["env"])
        self.assertEqual(live["env"]["CUA_LIVE_E2E_EVENT"], "${{ github.event_name }}")
        self.assertEqual(
            live["env"]["CUA_LIVE_E2E_NAMESPACE"],
            "cua-live-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}",
        )
        self.assertNotIn("github.run_id", live["env"]["CUA_LIVE_E2E_NAMESPACE"])
        self.assertNotIn("github.run_attempt", live["env"]["CUA_LIVE_E2E_NAMESPACE"])

        checkout = steps["Checkout main"]
        self.assertEqual(
            checkout["uses"],
            "actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5",
        )
        self.assertEqual(
            checkout["with"]["ref"],
            "${{ github.event_name == 'push' && github.sha || 'main' }}",
        )
        self.assertEqual(
            steps["Set up Python"]["uses"],
            "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065",
        )
        self.assertIn("-e libs/python/cua-sandbox", steps["Install main source"]["run"])
        self.assertIn("--upgrade cua-sandbox", steps["Install published package"]["run"])

        preflight = steps["Check Fleet OAuth credentials"]["run"]
        self.assertIn("CUA_CLIENT_ID", preflight)
        self.assertIn("CUA_CLIENT_SECRET", preflight)
        self.assertIn("exit 1", preflight)

        isolated_suite = steps["Prepare isolated live test suite"]["run"]
        self.assertIn("CUA_LIVE_E2E_TEST_ROOT", isolated_suite)
        self.assertIn("tests/live", isolated_suite)
        live_run = steps["Run live Fleet smoke"]["run"]
        self.assertIn("$CUA_LIVE_E2E_TEST_ROOT/tests/live/test_fleet_ephemeral.py", live_run)
        self.assertNotIn("libs/python/cua-sandbox/tests/live", live_run)
        self.assertIn('PYTHONPATH="$CUA_LIVE_E2E_TEST_ROOT', live_run)

    def test_failure_diagnostics_alerting_and_pinned_actions(self) -> None:
        workflow = self.workflow()
        live = workflow["jobs"]["live"]
        steps = self.steps_by_name(live)

        for step in live["steps"]:
            if "uses" in step:
                self.assertRegex(step["uses"], r"^[^@]+@[0-9a-f]{40}$")
        self.assertEqual(
            steps["Upload failure diagnostics"]["uses"],
            "actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08",
        )
        self.assertEqual(steps["Upload failure diagnostics"]["if"], "failure()")
        self.assertEqual(steps["Upload failure diagnostics"]["with"]["retention-days"], "7")

        controlled_summary = steps["Write controlled failure diagnostics"]
        self.assertEqual(
            controlled_summary["if"],
            "github.event_name == 'workflow_dispatch' && inputs.force_failure",
        )
        self.assertIn("summary.json", controlled_summary["run"])
        self.assertIn("ControlledFailure", controlled_summary["run"])
        self.assertEqual(
            steps["Controlled alert test failure"]["if"],
            "github.event_name == 'workflow_dispatch' && inputs.force_failure",
        )

        versions = steps["Record installed versions"]
        self.assertIn("GITHUB_OUTPUT", versions["run"])
        self.assertNotIn("steps.versions.outputs", versions["run"])

        alert = steps["Alert Alertmanager"]
        self.assertEqual(alert["if"], "failure()")
        self.assertIn("https://am.cua.ai/api/v2/alerts", alert["run"])
        self.assertIn("PeriodicCuaSandboxLiveE2EFailed", alert["run"])
        self.assertIn('"lane": "${{ matrix.lane }}"', alert["run"])
        self.assertIn("${{ steps.versions.outputs.sandbox }}", alert["run"])

    def test_cleanup_is_claim_only_and_inventory_is_diagnostic(self) -> None:
        workflow = WORKFLOW.read_text()
        live_test = LIVE_TEST.read_text()

        self.assertNotIn("cleanup_namespace", workflow)
        self.assertNotIn("delete_namespace", workflow)
        self.assertNotIn("Emergency namespace cleanup", workflow)
        self.assertIn("Sandbox.ephemeral", live_test)
        self.assertNotIn("wait_namespace_absent", live_test)
        self.assertNotIn("namespace_leak", live_test)
        self.assertIn("wait_claims_absent", live_test)
        self.assertIn("claim_leak", live_test)
        self.assertIn("persistent_resources", live_test)
        self.assertIn("unexpected_inventory", live_test)
        self.assertIn("module_origins", live_test)
        self.assertIn("cua_sandbox", live_test)

    def test_isolated_suite_cannot_import_checkout_cua_sandbox(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary_root = Path(temporary_directory)
            suite_root = temporary_root / "live-suite"
            copied_live = suite_root / "tests/live"
            copied_live.mkdir(parents=True)
            shutil.copy2(SANDBOX_ROOT / "tests/__init__.py", suite_root / "tests")
            shutil.copy2(SANDBOX_ROOT / "tests/live/__init__.py", copied_live)

            installed_root = temporary_root / "installed"
            installed_package = installed_root / "cua_sandbox"
            installed_package.mkdir(parents=True)
            (installed_package / "__init__.py").write_text("MARKER = 'published-wheel'\n")
            probe = copied_live / "import_origin_probe.py"
            probe.write_text(
                "import cua_sandbox\n"
                "from pathlib import Path\n"
                "print(Path(cua_sandbox.__file__).resolve())\n"
            )

            environment = os.environ | {
                "PYTHONPATH": os.pathsep.join((str(suite_root), str(installed_root)))
            }
            result = subprocess.run(
                [sys.executable, str(probe)],
                cwd=REPO_ROOT,
                env=environment,
                check=True,
                capture_output=True,
                text=True,
            )

            origin = Path(result.stdout.strip()).resolve()
            self.assertEqual(origin, (installed_package / "__init__.py").resolve())
            self.assertNotIn(SANDBOX_ROOT.resolve(), origin.parents)


if __name__ == "__main__":
    unittest.main()
