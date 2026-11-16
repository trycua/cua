"""The Cua Driver desktop E2E has a quick PR lane and a full lane after merge and at release.

Run: python3 .github/scripts/tests/test_cua_driver_e2e_quick_lane.py
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WF = ROOT / ".github/workflows"
GATE = (WF / "e2e-cua-driver-release-gate.yml").read_text()
CD = (WF / "cd-rust-cua-driver.yml").read_text()


def job(text: str, name: str) -> str:
    m = re.search(rf"^  {re.escape(name)}:\n(.*?)(?=^  [a-z0-9-]+:\n|\Z)", text, re.S | re.M)
    assert m, name
    return m.group(1)


class QuickLane(unittest.TestCase):
    def test_full_suite_runs_after_merge_nightly_and_on_demand(self):
        trigger = GATE.split("permissions:", 1)[0]
        self.assertIn("push:\n    branches: [main]", trigger)
        self.assertIn("schedule:", trigger)
        self.assertIn("workflow_dispatch:", trigger)
        self.assertIn("post-merge", job(GATE, "decide"))

    def test_pull_requests_get_the_quick_lane(self):
        decide = job(GATE, "decide")
        self.assertIn('emit true pr "label e2e-gate" quick', decide)
        self.assertRegex(decide, r'emit true pr "gated paths[^\n]*quick')
        # Every other trigger is a full run.
        self.assertIn('lanes=${4:-full}', decide)

    def test_quick_lane_is_the_shared_electron_lane_on_each_os(self):
        self.assertIn("lane: ${{ needs.decide.outputs.lanes == 'quick' && 'shared' || 'all' }}", job(GATE, "linux"))
        self.assertIn(
            "lane: ${{ needs.decide.outputs.lanes == 'quick' && 'shared' || needs.decide.outputs.lanes == 'queue' && 'native,capture,installer' || 'all' }}",
            job(GATE, "windows"),
        )
        for name in ("linux", "windows"):
            self.assertIn("harness_filter: ${{ needs.decide.outputs.lanes == 'quick' && 'electron' || '' }}", job(GATE, name))
        self.assertIn("-f quick=true", job(GATE, "macos-pr"))

    def test_merge_queue_runs_the_lanes_that_fit_its_timeout_on_gated_paths(self):
        decide = job(GATE, "decide")
        self.assertIn('files=$(compare_files "$QUEUE_BASE" "$QUEUE_HEAD")', decide)
        self.assertIn('emit true queue "merge queue touching gated paths', decide)
        self.assertIn('emit false queue "merge queue without gated paths"', decide)
        self.assertIn('emit true queue "merge queue, changed files unreadable" queue', decide)
        # Linux runs every lane; Windows all but shared Electron + Tauri; macOS and the browsers run after merge.
        self.assertIn("needs.decide.outputs.lanes != 'queue'", job(GATE, "macos"))
        self.assertIn("needs.decide.outputs.lanes != 'queue'", job(GATE, "browsers"))

    def test_changed_files_read_in_part_count_as_unreadable(self):
        import os
        import subprocess
        import tempfile
        decide = job(GATE, "decide")
        helper = decide[decide.index("compare_files() {"):]
        helper = helper[:helper.index("\n          }\n") + len("\n          }\n")]
        def files(stub):
            with tempfile.TemporaryDirectory() as d:
                gh = Path(d) / "gh"
                gh.write_text("#!/bin/bash\n" + stub + "\n")
                gh.chmod(0o755)
                return subprocess.run(["bash", "-c", helper + 'compare_files a b'], capture_output=True, text=True,
                                      env={**os.environ, "PATH": f"{d}:{os.environ['PATH']}", "REPO": "o/r"},
                                      check=True).stdout.split()
        self.assertEqual(files("printf 'a.rs\\nb.rs\\n'"), ["a.rs", "b.rs"])
        # A page that fails after others printed, and a list the API cut off at 300, are not a file list.
        self.assertEqual(files("printf 'a.rs\\n'; exit 1"), [])
        self.assertEqual(files("seq 300"), [])
        self.assertEqual(len(files("seq 299")), 299)

    def test_browser_lanes_are_full_only_and_do_not_fail_the_quick_result(self):
        self.assertIn("needs.decide.outputs.lanes != 'quick'", job(GATE, "browsers"))
        self.assertIn('(.key != "browsers" or .value.result != "skipped" or (env.LANES_SIZE != "quick" and env.LANES_SIZE != "queue"))', job(GATE, "result"))
        self.assertIn('(.key != "macos" or .value.result != "skipped" or (env.EVENT != "pull_request" and env.LANES_SIZE != "queue"))', job(GATE, "result"))

    def test_full_macos_lane_runs_on_push_and_is_not_quick(self):
        macos = job(GATE, "macos")
        self.assertIn("github.event_name != 'pull_request'", macos)
        # Not the quick lane: the only `quick` here is the shared stage's scope decision.
        self.assertNotIn("quick", macos.replace("needs.quick.outputs.skip", "").replace("needs: [quick, ", "needs: ["))

    def test_windows_lanes_take_a_comma_list(self):
        text = (WF / "e2e-rust-windows.yml").read_text()
        for lane in ("shared", "native", "capture", "installer"):
            self.assertIn(f"if: inputs.lane == 'all' || contains(format(',{{0}},', inputs.lane), ',{lane},')", text)
        self.assertIn("if: success() && inputs.lane == 'all'", text)

    def test_failures_after_merge_open_the_tracking_issue(self):
        report = job(GATE, "nightly-report")
        self.assertIn("needs.decide.outputs.mode == 'post-merge'", report)
        self.assertIn("fix it forward", report)

    def test_release_gate_still_runs_every_lane_on_the_tag_sha(self):
        for wf in ("e2e-rust-linux.yml", "e2e-rust-windows.yml", "e2e-rust-macos.yml", "e2e-rust-standalone-browsers.yml"):
            self.assertIn(f"uses: ./.github/workflows/{wf}", CD)
        for name in ("e2e-linux", "e2e-windows", "e2e-macos", "e2e-standalone-browsers"):
            if re.search(rf"^  {name}:", CD, re.M):
                body = job(CD, name)
                self.assertNotIn("harness_filter", body)
                self.assertNotIn("quick", body)
                self.assertNotRegex(body, r"lane:\s")

    def test_filter_inputs_default_to_everything(self):
        for wf in ("e2e-rust-linux.yml", "e2e-rust-windows.yml"):
            text = (WF / wf).read_text()
            self.assertEqual(len(re.findall(r"harness_filter:\n(?:        .*\n)*?        default: \"\"", text)), 2, wf)
            self.assertIn("CUA_E2E_HARNESS_FILTER: ${{ inputs.harness_filter }}", text)


if __name__ == "__main__":
    unittest.main()
