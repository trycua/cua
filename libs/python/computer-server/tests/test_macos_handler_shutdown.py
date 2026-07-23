import subprocess
import sys

import pytest


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS-only PyObjC lifecycle test")
def test_macos_handler_import_exits_cleanly() -> None:
    """Importing the handler must not double-release PyObjC-owned objects."""
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "from computer_server.handlers.macos import MacOSAutomationHandler",
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
