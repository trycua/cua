from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

INSTALL_LOCAL = Path(__file__).resolve().parents[1] / "_install-local-rust.sh"


def _write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def test_installer_stages_binary_from_custom_cargo_target(tmp_path: Path) -> None:
    fixture_root = tmp_path / "cua-driver"
    scripts_dir = fixture_root / "scripts"
    rust_dir = fixture_root / "rust"
    scripts_dir.mkdir(parents=True)
    rust_dir.mkdir()
    shutil.copy2(INSTALL_LOCAL, scripts_dir / INSTALL_LOCAL.name)

    stale_binary = rust_dir / "target/release/cua-driver"
    _write_executable(stale_binary, "printf 'stale workspace target\\n'")

    custom_target = tmp_path / "custom target"
    fake_bin = tmp_path / "fake-bin"
    _write_executable(
        fake_bin / "cargo",
        """set -eu
test "${1:-}" = build
mkdir -p "$CARGO_TARGET_DIR/release"
printf 'fresh custom target\n' > "$CARGO_TARGET_DIR/release/cua-driver"
chmod +x "$CARGO_TARGET_DIR/release/cua-driver"
""",
    )
    _write_executable(
        fake_bin / "uname",
        """case "${1:-}" in
    -s) printf 'Linux\n' ;;
    -m) printf 'x86_64\n' ;;
    *) exit 2 ;;
esac
""",
    )
    _write_executable(fake_bin / "systemctl", "exit 0")
    _write_executable(fake_bin / "pkill", "exit 0")

    local_home = tmp_path / "local-home"
    install_bin = tmp_path / "install-bin"
    env = os.environ.copy()
    env.pop("SUDO_USER", None)
    env.update(
        {
            "HOME": str(tmp_path / "home"),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CARGO_TARGET_DIR": str(custom_target),
            "CUA_DRIVER_SOURCE_SHA": "a" * 40,
            "CUA_DRIVER_LOCAL_HOME": str(local_home),
            "CUA_DRIVER_LOCAL_INSTALL_DIR": str(install_bin),
        }
    )

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / INSTALL_LOCAL.name), "--release"],
        cwd=fixture_root,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert (custom_target / "release/cua-driver").read_text() == "fresh custom target\n"
    assert (install_bin / "cua-driver-local").read_text() == "fresh custom target\n"
