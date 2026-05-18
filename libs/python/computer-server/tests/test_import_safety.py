import os
import platform
import subprocess
import sys

import pytest


def _assert_import_exits_cleanly(import_statement: str):
    env = os.environ.copy()
    for key in ("CUA_BACKEND", "CUA_VNC_HOST", "CUA_VNC_PORT", "CUA_VNC_PASSWORD"):
        env.pop(key, None)

    result = subprocess.run(
        [sys.executable, "-X", "faulthandler", "-c", f"{import_statement}; print('ok')"],
        capture_output=True,
        env=env,
        text=True,
        timeout=20,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "ok" in result.stdout


def test_package_import_does_not_import_native_handlers_at_teardown():
    _assert_import_exits_cleanly("import computer_server")


def test_generic_handler_import_does_not_import_native_handlers_at_teardown():
    _assert_import_exits_cleanly("import computer_server.handlers.generic")


@pytest.mark.skipif(platform.system() != "Darwin", reason="macOS handler requires AppKit")
def test_macos_handler_creation_exits_cleanly_at_teardown():
    _assert_import_exits_cleanly(
        "from computer_server.handlers.factory import HandlerFactory; "
        "HandlerFactory.create_handlers()"
    )
