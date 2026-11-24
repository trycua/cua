"""Guard: workflows must not pin a Python that ubuntu-latest cannot provide.

`ubuntu-latest` moved to Ubuntu 26.04, where actions/setup-python has no
3.8/3.9 build. A job pinning one fails with "version not found". Jobs on
`ubuntu-latest` may only use ALLOWED_VERSIONS; an older Python needs a
pinned `runs-on: ubuntu-24.04` (or another non-latest label) on that job.
"""

import re
import tempfile
from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[3]
GITHUB_DIR = REPO_ROOT / ".github"
ALLOWED_VERSIONS = {"3.11", "3.12", "3.13", "3.14"}

JOB_RE = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
RUNS_ON_RE = re.compile(r"^\s{4}runs-on:\s*(.+?)\s*(#.*)?$")
VERSION_RE = re.compile(
    r"^\s*(?:-\s*)?(python-version|python_version)\s*:\s*(.+?)\s*(#.*)?$"
)
DEFAULT_RE = re.compile(r"^\s+default:\s*(.+?)\s*(#.*)?$")


def versions_in(value: str) -> list[str]:
    value = value.strip()
    if value.startswith("${{") or not value:
        return []
    return re.findall(r"\d+\.\d+(?:\.\d+)?", value)


def pinned_versions(path: Path):
    """Yield (line_no, version, runs_on) for each literal Python pin."""
    runs_on = None
    in_python_input = False
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if JOB_RE.match(line):
            runs_on = None
        match = RUNS_ON_RE.match(line)
        if match:
            runs_on = match.group(1).strip("\"'")
        match = VERSION_RE.match(line)
        if match:
            in_python_input = match.group(1) == "python_version"
            for version in versions_in(match.group(2)):
                yield number, version, runs_on
            continue
        if in_python_input:
            default = DEFAULT_RE.match(line)
            if default:
                for version in versions_in(default.group(1)):
                    yield number, version, runs_on
                in_python_input = False


def minor(version: str) -> str:
    return ".".join(version.split(".")[:2])


class WorkflowPythonVersionTests(unittest.TestCase):
    def test_ubuntu_latest_jobs_use_available_python(self):
        files = sorted(GITHUB_DIR.glob("workflows/*.yml")) + sorted(
            GITHUB_DIR.glob("actions/**/action.yml")
        )
        bad = []
        for path in files:
            for number, version, runs_on in pinned_versions(path):
                latest = runs_on is None or "ubuntu-latest" in runs_on
                if latest and minor(version) not in ALLOWED_VERSIONS:
                    bad.append(f"{path.relative_to(REPO_ROOT)}:{number} pins Python {version}")
        self.assertEqual(
            bad,
            [],
            "ubuntu-latest (26.04) has no setup-python build for these; use "
            f"{sorted(ALLOWED_VERSIONS)} or pin runs-on: ubuntu-24.04 with a comment.",
        )

    def test_guard_flags_old_python_on_latest(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "w.yml"
            path.write_text(
                "jobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n"
                "      - with:\n          python-version: \"3.9\"\n"
                "  b:\n    runs-on: ubuntu-24.04\n    steps:\n"
                "      - with:\n          python-version: \"3.9\"\n"
            )
            found = [(v, r) for _, v, r in pinned_versions(path)]
        self.assertEqual(found, [("3.9", "ubuntu-latest"), ("3.9", "ubuntu-24.04")])


if __name__ == "__main__":
    unittest.main()
