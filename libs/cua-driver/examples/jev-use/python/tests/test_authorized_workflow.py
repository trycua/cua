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
        action_refs = re.findall(r"^\s*(?:-\s+)?uses:\s*[^@\s]+@([^\s]+)", self.workflow, re.MULTILINE)
        self.assertTrue(action_refs)
        self.assertTrue(all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in action_refs))

    def test_workflow_is_credential_free(self) -> None:
        self.assertNotIn("TYPESAFE_API_KEY", self.workflow)
        self.assertNotIn("secrets.", self.workflow)
        self.assertNotIn("run_live", self.workflow)
        self.assertNotIn("--live", self.workflow)

    def test_mock_steps_use_preinstalled_python_and_audit_mock_rows(self) -> None:
        self.assertIn(
            'python_bin="$GITHUB_WORKSPACE/libs/cua-driver/examples/jev-use/.venv/bin/python"',
            self.workflow,
        )
        self.assertIn("verify_setup.py --typescript --max-steps 2", self.workflow)
        self.assertIn(".venv/bin/python verify_choice_cli.py", self.workflow)
        self.assertIn('assert summary["live_requested"] is False', self.workflow)
        self.assertIn('assert summary["typescript_requested"] is True', self.workflow)
        mock_checks = re.search(
            r"name: Run bounded mock checks\n(?:[^\n]*\n)*?.*?verify_setup\.py --typescript",
            self.workflow,
        )
        self.assertIsNotNone(mock_checks)


if __name__ == "__main__":
    unittest.main()
