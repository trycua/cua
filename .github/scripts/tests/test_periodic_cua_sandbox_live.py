"""Contract tests for the periodic Cua Sandbox live Fleet workflow."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github/workflows/periodic-cua-sandbox-live.yml"
LIVE_TEST = REPO_ROOT / "libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py"


class TestPeriodicCuaSandboxLive(unittest.TestCase):
    def test_trigger_and_lane_contract(self) -> None:
        workflow = WORKFLOW.read_text()

        self.assertIn('cron: "7/15 * * * *"', workflow)
        self.assertIn("published-package", workflow)
        self.assertIn("main-source", workflow)
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("force_failure:", workflow)
        self.assertIn('[[ "$EVENT_NAME" == "push" ]]', workflow)
        self.assertIn("fromJSON(needs.prepare.outputs.matrix)", workflow)
        self.assertIn("timeout-minutes: 25", workflow)
        self.assertIn("cancel-in-progress:", workflow)

    def test_security_cleanup_and_failure_contract(self) -> None:
        workflow = WORKFLOW.read_text()
        live_test = LIVE_TEST.read_text()

        self.assertIn("CUA_CLIENT_ID", workflow)
        self.assertIn("CUA_CLIENT_SECRET", workflow)
        self.assertNotIn("CUA_API_KEY", workflow)
        self.assertNotIn("cleanup_namespace", workflow)
        self.assertNotIn("delete_namespace", workflow)
        self.assertIn("Sandbox.ephemeral", live_test)
        self.assertIn("wait_namespace_absent", live_test)
        self.assertIn("namespace_leak", live_test)
        self.assertIn("remaining_resources", live_test)
        self.assertIn("if: failure()", workflow)
        self.assertIn("PeriodicCuaSandboxLiveE2EFailed", workflow)
        self.assertIn('lane": "${{ matrix.lane }}"', workflow)
        self.assertIn("retention-days: 7", workflow)
        self.assertIn(
            "github.event_name == 'workflow_dispatch' && inputs.force_failure", workflow
        )

    def test_actions_are_pinned(self) -> None:
        workflow = WORKFLOW.read_text()

        self.assertIn(
            "actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5",
            workflow,
        )
        self.assertIn(
            "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065",
            workflow,
        )
        self.assertIn(
            "actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08",
            workflow,
        )

    def test_live_parameters_are_pinned(self) -> None:
        live_test = LIVE_TEST.read_text()

        self.assertIn("desktop-workspace-duo", live_test)
        self.assertIn(
            "5b9cb82f482834f7541901b87be956e7544d0db13fabc0b372cbc5eca5a74180",
            live_test,
        )
        for expected in (
            "cpu=4",
            "memory_mb=4096",
            "server_port=8000",
            "time_to_start=900",
            "request_timeout=60",
            "telemetry_enabled=False",
        ):
            self.assertIn(expected, live_test)


if __name__ == "__main__":
    unittest.main()
