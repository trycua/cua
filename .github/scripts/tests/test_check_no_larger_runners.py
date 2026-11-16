import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_no_larger_runners as chk  # noqa: E402

REPO = Path(__file__).resolve().parents[3]


def make_repo(workflows: dict[str, str], allowed: str = "") -> Path:
    root = Path(tempfile.mkdtemp())
    (root / ".github" / "workflows").mkdir(parents=True)
    for name, body in workflows.items():
        (root / ".github" / "workflows" / name).write_text(textwrap.dedent(body))
    (root / ".github" / "larger-runners-allowed.txt").write_text(allowed)
    return root


class LargerRunnerCheck(unittest.TestCase):
    def test_label_classifier(self):
        for bad in ("macos-26-xlarge", "macos-latest-large", "macos-15-xlarge",
                    "cua-linux-16core", "cua-windows-8core", "ubuntu-latest-8-cores",
                    "windows-latest-16-cores"):
            self.assertTrue(chk.is_larger(bad), bad)
        for ok in ("ubuntu-latest", "ubuntu-24.04-arm", "macos-26", "macos-15-intel",
                   "windows-latest", "windows-11-arm", "self-hosted", "lume"):
            self.assertFalse(chk.is_larger(ok), ok)

    def test_literal_and_list(self):
        root = make_repo({"a.yml": """
            on: push
            jobs:
              one: {runs-on: macos-26-xlarge, steps: []}
              two: {runs-on: [self-hosted, cua-linux-16core], steps: []}
              three: {runs-on: ubuntu-latest, steps: []}
              four: {runs-on: {group: big-runners}, steps: []}
        """})
        got = {(j, l) for _, j, l in chk.find_uses(root)}
        self.assertEqual(got, {("one", "macos-26-xlarge"), ("two", "cua-linux-16core"),
                               ("four", "group:big-runners")})

    def test_matrix_expression_and_inputs(self):
        root = make_repo({"m.yml": """
            on:
              workflow_call:
                inputs:
                  runner: {type: string, default: macos-latest-large}
            jobs:
              j:
                strategy:
                  matrix:
                    os: [ubuntu-latest, cua-windows-8core]
                runs-on: ${{ matrix.os == 'ubuntu-latest' && 'ubuntu-latest' || matrix.os }}
                steps: []
              k:
                runs-on: ${{ inputs.runner }}
                steps: []
              l:
                strategy:
                  matrix:
                    include:
                      - {runner: macos-26-xlarge}
                runs-on: ${{ matrix.runner }}
                steps: []
        """})
        got = {(j, l) for _, j, l in chk.find_uses(root)}
        self.assertEqual(got, {("j", "cua-windows-8core"), ("k", "macos-latest-large"),
                               ("l", "macos-26-xlarge")})

    def test_allowlist_and_exit_code(self):
        wf = {"a.yml": "on: push\njobs:\n  j:\n    runs-on: macos-26-xlarge\n    steps: []\n"}
        self.assertEqual(chk.main([str(make_repo(wf))]), 1)
        allowed = "# reason\na.yml macos-26-xlarge  # approved\n"
        self.assertEqual(chk.main([str(make_repo(wf, allowed))]), 0)

    def test_repository_has_no_larger_runners_and_empty_allowlist(self):
        self.assertEqual(chk.find_uses(REPO), [])
        self.assertEqual(chk.load_allowed(REPO), set())

    def test_missing_pyyaml_gives_a_clear_error_not_a_traceback(self):
        import os
        import subprocess

        block = Path(tempfile.mkdtemp())
        (block / "yaml.py").write_text("raise ImportError('blocked')\n")
        env = {"PYTHONPATH": str(block), "CUA_LARGER_RUNNERS_REEXEC": "1", "PATH": os.environ.get("PATH", "")}
        done = subprocess.run([sys.executable, str(Path(chk.__file__))], env=env, capture_output=True, text=True)
        self.assertEqual(done.returncode, 1)
        self.assertIn("needs PyYAML", done.stderr)
        self.assertNotIn("Traceback", done.stderr)


if __name__ == "__main__":
    unittest.main()
