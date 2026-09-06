"""Focused contract for stable plugin source assets in Driver releases."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[5]


class WorkflowTest(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/cd-rust-cua-driver.yml").read_text()
        self.job = self.workflow.split("  build-hyprland-plugin-source:\n", 1)[1].split(
            "  verify-release-artifacts:\n", 1
        )[0]
        self.release = self.workflow.split("  release:\n", 1)[1]

    def test_stable_immutable_candidates_only_and_publication_dependency(self):
        self.assertIn("needs: release-attribution-preflight", self.job)
        self.assertIn(
            "    if: >-\n"
            "      inputs.channel != 'nightly' &&\n"
            "      (startsWith(github.ref, 'refs/tags/cua-driver-rs-v') ||\n"
            "       (github.event_name == 'workflow_dispatch' && inputs.publish == true))",
            self.job,
        )
        # Every publishing dispatch runs the source job. Manual build-only and
        # nightly invocations skip both publication and this stable source job.
        needs, steps = self.release.split("    steps:\n", 1)
        self.assertIn("build-hyprland-plugin-source]", needs)
        self.assertIn("if: github.event_name == 'workflow_dispatch' && inputs.publish == true", needs)
        self.assertNotIn("always()", needs)
        self.assertIn('if [[ "$SHA" != "$TAG_SHA" ]]; then', steps)
        self.assertIn("python3 .github/scripts/validate_release_versions.py --product driver", steps)

    def test_source_override_is_checked_and_assets_remain_namespaced(self):
        self.assertIn("fetch-depth: 0", self.job)
        self.assertIn(
            "ref: ${{ inputs.source_ref || github.event_name == 'workflow_dispatch' && "
            "inputs.publish && format('refs/tags/cua-driver-rs-v{0}', inputs.version) || github.ref }}",
            self.job,
        )
        self.assertIn('--repo . --revision "$(git rev-parse HEAD)"', self.job)
        self.assertIn('--driver-version "$VERSION" --release-assets', self.job)
        self.assertIn("path: plugin-release-assets/*.tar.gz", self.job)
        self.assertIn("if-no-files-found: error", self.job)
        self.assertIn("if: steps.source.outputs.available == 'true'", self.job)
        self.assertLess(self.job.index('test "$SHA" = "$TAG_SHA"'), self.job.index("if ! git cat-file"))
        self.assertLess(self.job.index('test "$VERSION" = "$SOURCE_VERSION"'), self.job.index("if ! git cat-file"))
        self.assertNotIn("apply-version", self.job)
        self.assertIn("cua-hyprland-plugin-*.tar.gz", self.release)
        self.assertNotIn("--clobber", self.release)


if __name__ == "__main__":
    unittest.main()
