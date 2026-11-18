import shutil
import subprocess
from pathlib import Path

import pytest


def test_wayland_helper_cursor_paint():
    node = shutil.which("node")
    if node is None:
        pytest.fail("Node >= 20.6 is required for the helper cursor paint test")
    subprocess.run(
        [node, str(Path(__file__).with_suffix(".mjs"))], check=True
    )
