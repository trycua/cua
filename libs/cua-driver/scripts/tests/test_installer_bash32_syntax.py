"""Parse every installer script with /bin/bash (Bash 3.2 on macOS CI)."""

from pathlib import Path
import subprocess

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = sorted((REPO_ROOT / "libs/cua-driver/scripts").glob("*.sh"))


@pytest.mark.parametrize("script", SCRIPTS, ids=lambda path: path.name)
def test_installer_scripts_parse_with_the_system_bash(script: Path) -> None:
    completed = subprocess.run(["/bin/bash", "-n", str(script)], capture_output=True, text=True)
    assert completed.returncode == 0, completed.stderr
