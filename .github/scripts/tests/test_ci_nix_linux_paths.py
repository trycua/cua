"""Regression tests for the expensive Nix workflow's ordered path filters."""

import json
from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW_PATH = REPO_ROOT / ".github/workflows/ci-nix-linux.yml"


def event_paths(workflow: str, event: str) -> list[str]:
    """Read one event's double-quoted paths without YAML 1.1 `on` coercion."""
    lines = workflow.splitlines()
    event_start = lines.index(f"  {event}:")
    event_end = next(
        (
            index
            for index in range(event_start + 1, len(lines))
            if lines[index].startswith("  ")
            and not lines[index].startswith("    ")
        ),
        len(lines),
    )
    paths_start = lines.index("    paths:", event_start, event_end)

    paths: list[str] = []
    for line in lines[paths_start + 1 : event_end]:
        if not line.startswith("      - "):
            if line.strip():
                break
            continue
        paths.append(json.loads(line.removeprefix("      - ")))
    return paths


def pattern_matches(pattern: str, path: str) -> bool:
    """Match the exact and directory-recursive globs used by this workflow."""
    if pattern.endswith("/**"):
        prefix = pattern.removesuffix("/**")
        return path == prefix or path.startswith(f"{prefix}/")
    if "*" in pattern or "?" in pattern:
        raise AssertionError(f"test matcher does not support workflow glob: {pattern}")
    return path == pattern


def path_selected(patterns: list[str], path: str) -> bool:
    """Apply GitHub's ordered positive/negative `paths` semantics to one path."""
    selected = False
    for raw_pattern in patterns:
        excluded = raw_pattern.startswith("!")
        pattern = raw_pattern.removeprefix("!")
        if pattern_matches(pattern, path):
            selected = not excluded
    return selected


class TestCiNixLinuxPaths(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = WORKFLOW_PATH.read_text()
        self.pull_request_paths = event_paths(self.workflow, "pull_request")
        self.push_paths = event_paths(self.workflow, "push")

    def workflow_selected(self, changed_paths: tuple[str, ...]) -> bool:
        return any(path_selected(self.pull_request_paths, path) for path in changed_paths)

    def test_pull_request_and_main_push_filters_stay_aligned(self) -> None:
        self.assertEqual(self.pull_request_paths, self.push_paths)

    def test_gallery_only_changes_do_not_select_the_five_job_matrix(self) -> None:
        gallery_only = (
            "libs/cua-driver/rust/crates/cursor-overlay/examples/export_gallery_frames.rs",
            "libs/cua-driver/scripts/cursor-gallery.sh",
            "libs/cua-driver/tools/cursor-gallery/app.js",
            "libs/cua-driver/tools/cursor-gallery/capture-gallery.mjs",
            "libs/cua-driver/tools/cursor-gallery/README.md",
            "libs/cua-driver/tools/cursor-gallery/index.html",
            "libs/cua-driver/tools/cursor-gallery/styles.css",
            "docs/content/docs/how-to-guides/driver/personalize-cursor.mdx",
            "docs/public/img/cua-driver/cursor-themes/delivery-target-context.gif",
        )

        self.assertEqual(self.workflow.count("          - name:"), 5)
        self.assertFalse(self.workflow_selected(gallery_only))

    def test_production_nix_and_workflow_changes_still_select_the_matrix(self) -> None:
        covered_changes = (
            "libs/cua-driver/rust/crates/cursor-overlay/src/render_state.rs",
            "libs/cua-driver/rust/crates/cursor-overlay/assets/cua.default.cua-theme",
            "libs/cua-driver/rust/crates/cua-driver/src/main.rs",
            "nix/cua-driver/package.nix",
            "flake.lock",
            ".github/workflows/ci-nix-linux.yml",
        )

        for path in covered_changes:
            with self.subTest(path=path):
                self.assertTrue(self.workflow_selected((path,)))

    def test_gallery_exclusion_does_not_hide_mixed_production_changes(self) -> None:
        changed_paths = (
            "libs/cua-driver/rust/crates/cursor-overlay/examples/export_gallery_frames.rs",
            "libs/cua-driver/rust/crates/cursor-overlay/src/lib.rs",
        )

        self.assertTrue(self.workflow_selected(changed_paths))


if __name__ == "__main__":
    unittest.main()
