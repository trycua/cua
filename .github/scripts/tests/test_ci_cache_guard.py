"""cache_guard.py: a green main run must leave its build caches on main. Offline."""

import importlib.util
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("cache_guard", ROOT / ".github/scripts/cache_guard.py")
cg = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cg)

PREFIX = "sccache-v2-macOS-ARM64-libs-cua-sdk-release-macos_"
SLICE_KEY = "swiftui-rust-v3-aarch64-apple-darwin-rust1.97.1-trycua-cua-abc123"


def fake_gh(jobs, keys, notices=None):
    def fake(*args):
        url = args[-1]
        if url.endswith("/actions/runs/9"):
            return {"run_started_at": "2026-11-05T10:00:00Z", "updated_at": "2026-11-05T10:50:00Z"}
        if "/actions/runs/9/jobs" in url:
            return [{"jobs": jobs}]
        if "/annotations" in url:
            return notices or []
        if "/actions/caches" in url:
            return [{"actions_caches": [{"key": k} for k in keys]}]
        raise AssertionError(url)

    return fake


def job(name, conclusion="success", id=1):
    return {"name": name, "conclusion": conclusion, "id": id}


class SccacheEntry(unittest.TestCase):
    def run_check(self, jobs, keys):
        with mock.patch.object(cg, "gh_json", fake_gh(jobs, keys)):
            return cg.check("trycua/cua", "9", [f"infinite-canvas-swift (swift test)={PREFIX}"], [])

    def test_saved_today_is_fine(self):
        self.assertEqual(self.run_check([job("infinite-canvas-swift (swift test)")], [PREFIX + "20261105"]), [])

    def test_green_run_without_todays_entry_is_reported(self):
        problems = self.run_check([job("infinite-canvas-swift (swift test)")], [PREFIX + "20261104"])
        self.assertEqual(len(problems), 1)
        self.assertIn("no " + PREFIX, problems[0])

    def test_skipped_or_failed_job_is_not_checked(self):
        for conclusion in ("skipped", "failure", None):
            self.assertEqual(self.run_check([job("infinite-canvas-swift (swift test)", conclusion)], []), [])

    def test_a_run_that_crossed_midnight_may_have_saved_either_day(self):
        base = fake_gh([job("j")], [PREFIX + "20261106"])
        run = {"run_started_at": "2026-11-05T23:40:00Z", "updated_at": "2026-11-06T00:20:00Z"}
        with mock.patch.object(cg, "gh_json", lambda *a: run if a[-1].endswith("/runs/9") else base(*a)):
            self.assertEqual(cg.check("trycua/cua", "9", [f"j={PREFIX}"], []), [])


class SliceEntry(unittest.TestCase):
    NOTICE = [{"message": f"SwiftUI Rust slice cache hit = false ({SLICE_KEY})"}]

    def run_check(self, keys, notices):
        with mock.patch.object(cg, "gh_json", fake_gh([job("SwiftUI Rust (arm64)", id=5)], keys, notices)):
            return cg.check("trycua/cua", "9", [], ["SwiftUI Rust ("])

    def test_key_on_main(self):
        self.assertEqual(self.run_check([SLICE_KEY], self.NOTICE), [])

    def test_key_missing_from_main(self):
        self.assertIn("does not hold", self.run_check([], self.NOTICE)[0])

    def test_no_notice_is_reported_not_ignored(self):
        self.assertIn("no 'SwiftUI Rust slice cache hit' notice", self.run_check([SLICE_KEY], [])[0])


class Wiring(unittest.TestCase):
    def test_notice_text_matches_the_action(self):
        action = (ROOT / ".github/actions/spaces-rust-slice/action.yml").read_text()
        self.assertIn("SwiftUI Rust slice cache hit = ${HIT} (${KEY})", action)

    def test_workflow_watches_the_workflows_that_save(self):
        text = (ROOT / ".github/workflows/ci-cache-guard.yml").read_text()
        for name in ("CI: infinite-canvas (Swift)", "CI: Cua Spaces SwiftUI Rust slice (producer)"):
            self.assertIn(f'- "{name}"', text)
            self.assertIn(f'name: "{name}"', (ROOT / ".github/workflows" / {
                "CI: infinite-canvas (Swift)": "ci-infinite-canvas-swift.yml",
                "CI: Cua Spaces SwiftUI Rust slice (producer)": "ci-cua-spaces-rust-slice.yml",
            }[name]).read_text())

    def test_rust_cache_still_falls_back_to_the_newest_family_entry(self):
        text = (ROOT / ".github/actions/rust-cache/action.yml").read_text()
        self.assertEqual(text.count("restore-keys: ${{ steps.plan.outputs.prefix }}"), 2)


if __name__ == "__main__":
    unittest.main()
