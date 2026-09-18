from __future__ import annotations

import re
import unittest
from pathlib import Path


class AuthorizedWorkflowTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        root = Path(__file__).resolve().parents[6]
        cls.workflow = (
            root / ".github/workflows/authorized-live-jev-use.yml"
        ).read_text(encoding="utf-8")

    def test_actions_are_pinned_to_commits(self) -> None:
        action_refs = re.findall(r"^\s*- uses: [^@\s]+@([^\s]+)", self.workflow, re.MULTILINE)
        self.assertTrue(action_refs)
        self.assertTrue(all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in action_refs))

    def test_secret_bearing_steps_use_only_preinstalled_python(self) -> None:
        steps = re.findall(
            r"(?ms)^      - (?:id: [^\n]+\n        )?name:.*?(?=^      - |\Z)",
            self.workflow,
        )
        secret_steps = [step for step in steps if "TYPESAFE_API_KEY:" in step]
        self.assertEqual(len(secret_steps), 2)
        self.assertIn(
            'python_bin="$GITHUB_WORKSPACE/libs/cua-driver/examples/jev-use/.venv/bin/python"',
            self.workflow,
        )
        for step in secret_steps:
            self.assertNotRegex(step, r"\b(?:uv|npm|npx|cargo|pip|python)\s+(?:run|sync|install|build)")
            self.assertRegex(step, r"(?:\.venv/bin/python|\$python_bin)")


if __name__ == "__main__":
    unittest.main()
