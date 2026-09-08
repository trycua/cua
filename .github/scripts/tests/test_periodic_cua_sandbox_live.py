"""Contract tests for the periodic Cua Sandbox live Fleet workflow."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github/workflows/periodic-cua-sandbox-live.yml"
SANDBOX_ROOT = REPO_ROOT / "libs/python/cua-sandbox"
LIVE_TEST = SANDBOX_ROOT / "tests/live/test_fleet_ephemeral.py"
POOL_LIVE_TEST = SANDBOX_ROOT / "tests/live/test_fleet_pool_persistent.py"


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

    def run_prepare_matrix(
        self, event_name: str, requested_lane: str, requested_suite: str = ""
    ) -> dict[str, object]:
        prepare_script = self.workflow()["jobs"]["prepare"]["steps"][0]["run"]
        with tempfile.TemporaryDirectory() as temporary_directory:
            github_output = Path(temporary_directory) / "github-output"
            environment = os.environ | {
                "EVENT_NAME": event_name,
                "REQUESTED_LANE": requested_lane,
                "REQUESTED_SUITE": requested_suite,
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
        self.assertEqual(inputs["suite"]["options"], ["both", "ephemeral", "pool"])
        self.assertEqual(inputs["suite"]["default"], "both")
        self.assertEqual(inputs["force_failure"]["type"], "boolean")

        prepare = workflow["jobs"]["prepare"]
        self.assertEqual(prepare["outputs"]["matrix"], "${{ steps.matrix.outputs.matrix }}")
        prepare_script = prepare["steps"][0]["run"]
        self.assertIn('[[ "$EVENT_NAME" == "push" ]]', prepare_script)
        self.assertIn('{"lane":"main-source","suite":"ephemeral"}', prepare_script)
        self.assertIn('lanes=("main-source" "published-package")', prepare_script)
        self.assertIn('suites=("ephemeral" "pool")', prepare_script)

    def test_jobs_only_run_in_the_upstream_repository(self) -> None:
        workflow = self.workflow()
        for job_name in ("prepare", "live"):
            with self.subTest(job=job_name):
                self.assertEqual(
                    workflow["jobs"][job_name]["if"],
                    "github.repository == 'trycua/cua'",
                )

    def test_prepare_matrix_selects_lanes_and_suites_for_each_trigger(self) -> None:
        main_ephemeral = {"lane": "main-source", "suite": "ephemeral"}
        main_pool = {"lane": "main-source", "suite": "pool"}
        published_ephemeral = {"lane": "published-package", "suite": "ephemeral"}
        published_pool = {"lane": "published-package", "suite": "pool"}
        all_lanes_and_suites = {
            "include": [main_ephemeral, main_pool, published_ephemeral, published_pool]
        }
        expected_matrices = {
            ("push", "", ""): {"include": [main_ephemeral]},
            ("push", "both", "both"): {"include": [main_ephemeral]},
            ("push", "published-package", "pool"): {"include": [main_ephemeral]},
            ("schedule", "", ""): all_lanes_and_suites,
            ("schedule", "main-source", "pool"): all_lanes_and_suites,
            ("workflow_dispatch", "both", "both"): all_lanes_and_suites,
            ("workflow_dispatch", "main-source", ""): {"include": [main_ephemeral, main_pool]},
            ("workflow_dispatch", "main-source", "both"): {"include": [main_ephemeral, main_pool]},
            ("workflow_dispatch", "main-source", "pool"): {"include": [main_pool]},
            ("workflow_dispatch", "both", "pool"): {"include": [main_pool, published_pool]},
            ("workflow_dispatch", "published-package", "ephemeral"): {
                "include": [published_ephemeral]
            },
        }

        for inputs, expected_matrix in expected_matrices.items():
            with self.subTest(
                event_name=inputs[0], requested_lane=inputs[1], requested_suite=inputs[2]
            ):
                self.assertEqual(self.run_prepare_matrix(*inputs), expected_matrix)

    def test_docs_describe_the_remediated_workflow(self) -> None:
        docs = (
            REPO_ROOT / "docs/superpowers/specs/2026-08-09-periodic-cua-sandbox-live-e2e-design.md",
            REPO_ROOT / "docs/superpowers/plans/2026-08-09-periodic-cua-sandbox-live-e2e.md",
        )
        required_contract = (
            "libs/python/cua-fleet/**",
            "github.repository == 'trycua/cua'",
            "Check Fleet OAuth credentials",
            "step-scoped OAuth credentials",
            "CUA_LIVE_E2E_SOURCE_SHA",
            "periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}",
            "Prepare isolated live test suite",
            "CUA_LIVE_E2E_TEST_ROOT",
            'tee -a "$GITHUB_OUTPUT"',
            "Write controlled failure diagnostics",
            "persistent reconciled resources",
            "claim-only cleanup",
            "cua-live-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && github.run_id || github.event_name }}",
            "periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}-${{ matrix.suite }}",
            "cua-live-pool-warm-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}",
            "cua-live-pool-cold-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}",
            "Run live Fleet pool smoke",
            "test_fleet_pool_persistent.py",
            "WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=0, max_pool_size=1)",
            "pool_pre_existed",
            "claim-only release",
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
            "periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}-${{ matrix.suite }}",
        )
        self.assertEqual(
            live["concurrency"]["cancel-in-progress"],
            "${{ github.event_name == 'schedule' }}",
        )
        expected_oauth_env = {
            "CUA_CLIENT_ID": "${{ secrets.CUA_CLIENT_ID }}",
            "CUA_CLIENT_SECRET": "${{ secrets.CUA_CLIENT_SECRET }}",
        }
        self.assertNotIn("CUA_CLIENT_ID", live["env"])
        self.assertNotIn("CUA_CLIENT_SECRET", live["env"])
        self.assertNotIn("CUA_API_KEY", live["env"])
        oauth_steps = {
            step["name"]: step["env"]
            for step in live["steps"]
            if "CUA_CLIENT_ID" in step.get("env", {}) or "CUA_CLIENT_SECRET" in step.get("env", {})
        }
        self.assertEqual(
            oauth_steps,
            {
                "Check Fleet OAuth credentials": expected_oauth_env,
                "Run live Fleet smoke": expected_oauth_env,
                "Run live Fleet pool smoke": expected_oauth_env,
            },
        )
        self.assertEqual(live["env"]["CUA_LIVE_E2E_EVENT"], "${{ github.event_name }}")
        self.assertEqual(live["env"]["CUA_LIVE_E2E_SUITE"], "${{ matrix.suite }}")
        self.assertEqual(live["env"]["CUA_LIVE_E2E_SIGNED_URLS"], "true")
        self.assertEqual(
            live["env"]["CUA_LIVE_E2E_NAMESPACE"],
            "cua-live-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && github.run_id || github.event_name }}",
        )
        self.assertEqual(
            live["env"]["CUA_LIVE_E2E_POOL_WARM_NAMESPACE"],
            "cua-live-pool-warm-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}",
        )
        self.assertEqual(
            live["env"]["CUA_LIVE_E2E_POOL_COLD_NAMESPACE"],
            "cua-live-pool-cold-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}",
        )
        self.assertIn("github.run_id", live["env"]["CUA_LIVE_E2E_NAMESPACE"])
        for namespace_env in (
            "CUA_LIVE_E2E_POOL_WARM_NAMESPACE",
            "CUA_LIVE_E2E_POOL_COLD_NAMESPACE",
        ):
            self.assertNotIn("github.run_id", live["env"][namespace_env])
        for namespace_env in (
            "CUA_LIVE_E2E_NAMESPACE",
            "CUA_LIVE_E2E_POOL_WARM_NAMESPACE",
            "CUA_LIVE_E2E_POOL_COLD_NAMESPACE",
        ):
            self.assertNotIn("github.run_attempt", live["env"][namespace_env])

        step_names = [step["name"] for step in live["steps"]]
        self.assertLess(
            step_names.index("Checkout main"),
            step_names.index("Record checked out source SHA"),
        )

        checkout = steps["Checkout main"]
        self.assertEqual(
            checkout["uses"],
            "actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5",
        )
        self.assertEqual(
            checkout["with"]["ref"],
            "${{ github.event_name == 'push' && github.sha || github.ref }}",
        )
        self.assertEqual(
            steps["Set up Python"]["uses"],
            "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065",
        )
        self.assertIn("-e libs/python/cua-sandbox", steps["Install main source"]["run"])
        self.assertIn("--upgrade cua-sandbox", steps["Install published package"]["run"])

        preflight_step = steps["Check Fleet OAuth credentials"]
        self.assertEqual(preflight_step["env"], expected_oauth_env)
        preflight = preflight_step["run"]
        self.assertIn("CUA_CLIENT_ID", preflight)
        self.assertIn("CUA_CLIENT_SECRET", preflight)
        self.assertIn("exit 1", preflight)

        source_sha = steps["Record checked out source SHA"]
        self.assertEqual(source_sha["id"], "source_sha")
        self.assertIn("git rev-parse HEAD", source_sha["run"])
        self.assertIn("source_sha=", source_sha["run"])
        self.assertIn("GITHUB_OUTPUT", source_sha["run"])
        self.assertIn("CUA_LIVE_E2E_SOURCE_SHA", source_sha["run"])
        self.assertIn("GITHUB_ENV", source_sha["run"])

        isolated_suite = steps["Prepare isolated live test suite"]["run"]
        self.assertIn("CUA_LIVE_E2E_TEST_ROOT", isolated_suite)
        self.assertIn("tests/live", isolated_suite)
        live_step = steps["Run live Fleet smoke"]
        self.assertEqual(live_step["env"], expected_oauth_env)
        self.assertIn("matrix.suite == 'ephemeral'", live_step["if"])
        live_run = live_step["run"]
        self.assertIn("$CUA_LIVE_E2E_TEST_ROOT/tests/live/test_fleet_ephemeral.py", live_run)
        self.assertNotIn("libs/python/cua-sandbox/tests/live", live_run)
        self.assertIn('PYTHONPATH="$CUA_LIVE_E2E_TEST_ROOT', live_run)
        pool_step = steps["Run live Fleet pool smoke"]
        self.assertEqual(pool_step["env"], expected_oauth_env)
        self.assertIn("matrix.suite == 'pool'", pool_step["if"])
        self.assertIn("force_failure", pool_step["if"])
        pool_run = pool_step["run"]
        self.assertIn("$CUA_LIVE_E2E_TEST_ROOT/tests/live/test_fleet_pool_persistent.py", pool_run)
        self.assertNotIn("libs/python/cua-sandbox/tests/live", pool_run)
        self.assertIn('PYTHONPATH="$CUA_LIVE_E2E_TEST_ROOT', pool_run)

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
        self.assertEqual(
            steps["Upload failure diagnostics"]["with"]["name"],
            "cua-sandbox-live-${{ matrix.lane }}-${{ matrix.suite }}"
            "-${{ github.run_id }}-${{ github.run_attempt }}",
        )

        controlled_summary = steps["Write controlled failure diagnostics"]
        self.assertEqual(
            controlled_summary["if"],
            "github.event_name == 'workflow_dispatch' && inputs.force_failure",
        )
        self.assertIn("summary.json", controlled_summary["run"])
        self.assertIn("ControlledFailure", controlled_summary["run"])
        self.assertIn('os.environ["CUA_LIVE_E2E_SOURCE_SHA"]', controlled_summary["run"])
        self.assertIn('os.environ["CUA_LIVE_E2E_SUITE"]', controlled_summary["run"])
        self.assertNotIn('os.environ.get("GITHUB_SHA")', controlled_summary["run"])
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
        self.assertIn('"suite": "${{ matrix.suite }}"', alert["run"])
        self.assertIn("${{ steps.versions.outputs.sandbox }}", alert["run"])
        self.assertIn("${{ steps.source_sha.outputs.source_sha }}", alert["run"])
        self.assertNotIn('"source_sha": "${{ github.sha }}"', alert["run"])

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

    def test_pool_suite_is_claim_only_and_pool_is_persistent(self) -> None:
        workflow = WORKFLOW.read_text()
        pool_test = POOL_LIVE_TEST.read_text()

        self.assertIn("Run live Fleet pool smoke", workflow)
        self.assertIn("Sandbox.ephemeral", pool_test)
        self.assertIn("Pool.apply", pool_test)
        self.assertIn(
            "WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=0, max_pool_size=1)",
            pool_test,
        )
        self.assertIn("wait_claims_absent", pool_test)
        self.assertIn("claim_leak", pool_test)
        self.assertIn("persistent_resources", pool_test)
        self.assertIn("unexpected_inventory", pool_test)
        self.assertIn("module_origins", pool_test)
        self.assertIn("pool_pre_existed", pool_test)
        self.assertIn("is_pool_missing_error", pool_test)
        self.assertNotIn("pool.delete(", pool_test)
        self.assertNotIn("delete_pool", pool_test)
        self.assertNotIn("delete_namespace", pool_test)
        self.assertNotIn("keep_pool", pool_test)

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
