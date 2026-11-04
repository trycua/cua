from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[4]
BUILD_SCRIPT = REPO_ROOT / "libs/cua-driver/tests/fixtures/build/macos.sh"

FAKE_UNAME = '#!/usr/bin/env bash\nprintf "%s\\n" "$FAKE_UNAME_ARCH"\n'
FAKE_XCRUN = """#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$FAKE_TOOL_LOG"
while [[ $# -gt 0 && "$1" != "-o" ]]; do shift; done
: > "$2"
"""


def _write_executable(path: Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


@pytest.mark.skipif(
    os.name != "posix" or shutil.which("bash") is None,
    reason="requires a posix host with bash",
)
@pytest.mark.parametrize("arch", ["x86_64", "arm64"])
def test_fixture_build_targets_host_arch(tmp_path: Path, arch: str) -> None:
    driver_root = tmp_path / "cua-driver"
    script = driver_root / "tests/fixtures/build/macos.sh"
    script.parent.mkdir(parents=True)
    shutil.copy2(BUILD_SCRIPT, script)
    (driver_root / "rust/test-apps").mkdir(parents=True)

    fake_bin = tmp_path / "fake-bin"
    fake_bin.mkdir()
    _write_executable(fake_bin / "uname", FAKE_UNAME)
    _write_executable(fake_bin / "xcrun", FAKE_XCRUN)
    tool_log = tmp_path / "tools.log"

    result = subprocess.run(
        ["bash", str(script), "--only", "appkit"],
        check=False,
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}",
            "FAKE_TOOL_LOG": str(tool_log),
            "FAKE_UNAME_ARCH": arch,
        },
    )

    assert result.returncode == 0, result.stderr
    assert f"-target {arch}-apple-macos13.0 " in tool_log.read_text(encoding="utf-8")
