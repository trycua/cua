"""A process holding cua SDK handles at exit prints nothing on stderr.

The cached runtime in ``cua_sandbox._sdk`` outlives the ``cua._native``
module globals at interpreter shutdown, which made every process print
``Exception ignored in: <function Cua.__del__> ... AttributeError: 'NoneType'
object has no attribute 'uniffi_cua_sdk_fn_free_cua'`` until the binding's
finalizers were made shutdown-safe (libs/cua/scripts/uniffi-python-postprocess.mjs).
Needs the built cua-sdk library; nothing is started (an embedded runtime does
no I/O until its first call).
"""

from __future__ import annotations

import os
import subprocess
import sys

import pytest

SCRIPT = """
import sys
import cua_sandbox._sdk as sdk

runtime = sdk.runtime(state_dir=sys.argv[1])
runtime.sandboxes()
# The pattern of a real program: a handle in a library module's globals.
sdk._test_extra_handle = sdk.sdk().embedded(state_dir=sys.argv[1], fleet_from_env=False)
print("ok")
"""


def _native_library_loads() -> bool:
    try:
        import cua._native  # noqa: F401
    except OSError:
        return False
    return True


@pytest.mark.skipif(not _native_library_loads(), reason="the cua-sdk library is not built")
def test_sdk_handles_alive_at_exit_print_nothing(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    env = {
        **os.environ,
        "HOME": str(home),
        "CUA_HOME": str(home / ".cua"),
        "CUA_CREDENTIAL_STORE": "file",
        "CUA_FLEET_SESSION": "0",
        "CUA_TELEMETRY_ENABLED": "false",
    }
    for var in ("FLEETS_TOKEN", "CUA_CLIENT_ID", "CUA_CLIENT_SECRET"):
        env.pop(var, None)
    result = subprocess.run(
        [sys.executable, "-c", SCRIPT, str(tmp_path / "state")],
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "ok"
    assert result.stderr == "", result.stderr
