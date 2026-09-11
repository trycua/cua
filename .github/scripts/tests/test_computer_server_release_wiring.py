"""Computer-server publication must use the version-matching landed tag."""

import importlib.util
import subprocess
import unittest
from pathlib import Path
from unittest import mock

import yaml

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "computer_server_release", ROOT / ".github/scripts/resolve_computer_server_release.py"
)
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)
SHA = "a" * 40
PROJECT = '[project]\nname="cua-computer-server"\nversion="0.3.46"\n'


class ReleaseResolutionTests(unittest.TestCase):
    def test_existing_tag_resolves_to_version_matching_landed_commit(self):
        with mock.patch.object(RELEASE, "git", side_effect=[SHA, "", PROJECT]) as git:
            self.assertEqual(
                RELEASE.resolve("0.3.46", "refs/heads/main"), {"version": "0.3.46", "sha": SHA}
            )
        self.assertEqual(
            git.call_args_list,
            [
                mock.call("rev-parse", "--verify", "refs/tags/computer-server-v0.3.46^{commit}"),
                mock.call("merge-base", "--is-ancestor", SHA, "refs/remotes/origin/main"),
                mock.call("show", f"{SHA}:libs/python/computer-server/pyproject.toml"),
            ],
        )

    def test_tag_push_selects_same_source(self):
        with mock.patch.object(RELEASE, "git", side_effect=[SHA, "", PROJECT]):
            self.assertEqual(RELEASE.resolve("", "refs/tags/computer-server-v0.3.46")["sha"], SHA)

    def test_invalid_input_never_reaches_git(self):
        for version in ("", "main", "0.3.46-rc.1", "0.3.46\n", "01.3.46", "0.3.46;echo test"):
            with self.subTest(version=version), mock.patch.object(RELEASE, "git") as git:
                with self.assertRaises(ValueError):
                    RELEASE.resolve(version, "refs/heads/main")
                git.assert_not_called()
        with mock.patch.object(RELEASE, "git") as git:
            with self.assertRaises(ValueError):
                RELEASE.resolve("0.3.46", "refs/tags/computer-server-v0.3.45")
            git.assert_not_called()

    def test_invalid_sha_metadata_missing_tag_and_unmerged_source_fail_closed(self):
        cases = [
            ["main"],
            [SHA, "", PROJECT.replace("0.3.46", "0.3.45")],
            [SHA, "", PROJECT.replace("cua-computer-server", "other-package")],
            subprocess.CalledProcessError(1, ["git", "rev-parse"]),
            [SHA, subprocess.CalledProcessError(1, ["git", "merge-base"])],
        ]
        for replies in cases:
            with (
                self.subTest(replies=replies),
                mock.patch.object(RELEASE, "git", side_effect=replies),
            ):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    RELEASE.resolve("0.3.46", "refs/heads/main")


class ReleaseWiringTests(unittest.TestCase):
    def workflow(self, name):
        return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())

    def test_all_release_outputs_use_the_resolved_commit(self):
        workflow = self.workflow("cd-py-computer-server.yml")
        jobs = workflow["jobs"]
        self.assertEqual(
            workflow["concurrency"],
            {"group": "computer-server-publish", "cancel-in-progress": False},
        )
        self.assertEqual(jobs["prepare"]["outputs"]["sha"], "${{ steps.get-version.outputs.sha }}")
        for name in ("publish", "create-release"):
            self.assertEqual(jobs[name]["with"]["source_sha"], "${{ needs.prepare.outputs.sha }}")
        self.assertEqual(jobs["publish"]["with"]["python_version"], "3.12")
        self.assertEqual(
            jobs["build-binaries"]["steps"][0]["with"]["ref"], "${{ needs.prepare.outputs.sha }}"
        )

    def test_immutable_publish_does_not_run_legacy_main_reset(self):
        workflow = self.workflow("py-reusable-publish.yml")
        steps = workflow["jobs"]["build-and-publish"]["steps"]
        self.assertEqual(steps[0]["with"]["ref"], "${{ inputs.source_sha || 'main' }}")
        reset = next(step for step in steps if step.get("name") == "Ensure latest main branch")
        self.assertEqual(reset["if"], "inputs.source_sha == ''")
        verify = next(
            step for step in steps if step.get("name") == "Verify immutable release checkout"
        )
        self.assertEqual(verify["if"], "inputs.source_sha != ''")
        self.assertIn('test "$(git rev-parse HEAD)" = "$RELEASE_SOURCE_SHA"', verify["run"])
        self.assertLess(
            steps.index(verify),
            next(i for i, step in enumerate(steps) if step.get("name") == "Build and publish"),
        )


if __name__ == "__main__":
    unittest.main()
