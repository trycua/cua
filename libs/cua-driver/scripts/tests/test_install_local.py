from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

INSTALL_LOCAL = Path(__file__).resolve().parents[1] / "_install-local-rust.sh"
LOCAL_SIGNING = INSTALL_LOCAL.with_name("_local-signing.sh")
DISPATCHER = INSTALL_LOCAL.with_name("install-local.sh")


def _write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


@pytest.mark.parametrize("relative_target", [False, True], ids=["absolute", "relative"])
def test_installer_stages_binary_from_custom_cargo_target(
    tmp_path: Path, relative_target: bool
) -> None:
    fixture_root = tmp_path / "cua-driver"
    scripts_dir = fixture_root / "scripts"
    rust_dir = fixture_root / "rust"
    scripts_dir.mkdir(parents=True)
    rust_dir.mkdir()
    shutil.copy2(INSTALL_LOCAL, scripts_dir / INSTALL_LOCAL.name)
    shutil.copy2(LOCAL_SIGNING, scripts_dir / LOCAL_SIGNING.name)

    wayland_helper = fixture_root / "wayland-helper/winrects@cua"
    wayland_helper.mkdir(parents=True)
    (wayland_helper / "metadata.json").write_text('{"version":5}\n', encoding="utf-8")
    (wayland_helper / "extension.js").write_text("// semantic cursor v5\n", encoding="utf-8")

    stale_binary = rust_dir / "target/release/cua-driver"
    _write_executable(stale_binary, "printf 'stale workspace target\\n'")

    custom_target = (
        rust_dir / "relative custom target" if relative_target else tmp_path / "custom target"
    )
    cargo_target_dir = (
        str(custom_target.relative_to(rust_dir)) if relative_target else str(custom_target)
    )
    fake_bin = tmp_path / "fake-bin"
    _write_executable(
        fake_bin / "cargo",
        """set -eu
test "${1:-}" = build
test "$CARGO_TARGET_DIR" = "$EXPECTED_CARGO_TARGET_DIR"
mkdir -p "$CARGO_TARGET_DIR/release"
printf 'fresh custom target\n' > "$CARGO_TARGET_DIR/release/cua-driver"
printf 'fresh cursor theme compiler\n' > "$CARGO_TARGET_DIR/release/cua-cursor-theme"
chmod +x "$CARGO_TARGET_DIR/release/cua-driver"
chmod +x "$CARGO_TARGET_DIR/release/cua-cursor-theme"
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
    user_home = tmp_path / "home"
    installed_helper = user_home / ".local/share/gnome-shell/extensions/winrects@cua"
    installed_helper.mkdir(parents=True)
    (installed_helper / "metadata.json").write_text('{"version":4}\n', encoding="utf-8")
    (installed_helper / "extension.js").write_text("// legacy cursor\n", encoding="utf-8")
    install_bin = tmp_path / "install-bin"
    env = os.environ.copy()
    env.pop("SUDO_USER", None)
    env.update(
        {
            "HOME": str(user_home),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CARGO_TARGET_DIR": cargo_target_dir,
            "EXPECTED_CARGO_TARGET_DIR": str(custom_target),
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
    assert (
        custom_target / "release/cua-cursor-theme"
    ).read_text() == "fresh cursor theme compiler\n"
    assert (install_bin / "cua-driver-local").read_text() == "fresh custom target\n"
    assert (
        local_home / "packages/current/cua-cursor-theme"
    ).read_text() == "fresh cursor theme compiler\n"
    assert (
        local_home / "packages/current/wayland-helper/winrects@cua/metadata.json"
    ).read_text() == '{"version":5}\n'
    assert (installed_helper / "metadata.json").read_text() == '{"version":5}\n'
    assert (installed_helper / "extension.js").read_text() == "// semantic cursor v5\n"


def _linux_fixture(tmp_path: Path) -> tuple[Path, Path, dict[str, str]]:
    """Stage a minimal Linux install-local fixture: (scripts_dir, fake_bin, env)."""
    fixture_root = tmp_path / "cua-driver"
    scripts_dir = fixture_root / "scripts"
    rust_dir = fixture_root / "rust"
    scripts_dir.mkdir(parents=True)
    rust_dir.mkdir()
    for script in (INSTALL_LOCAL, LOCAL_SIGNING, DISPATCHER):
        shutil.copy2(script, scripts_dir / script.name)

    fake_bin = tmp_path / "fake-bin"
    _write_executable(
        fake_bin / "cargo",
        """set -eu
mkdir -p "$CARGO_TARGET_DIR/debug"
printf 'fresh driver\n' > "$CARGO_TARGET_DIR/debug/cua-driver"
printf 'fresh cursor theme compiler\n' > "$CARGO_TARGET_DIR/debug/cua-cursor-theme"
chmod +x "$CARGO_TARGET_DIR/debug/cua-driver"
chmod +x "$CARGO_TARGET_DIR/debug/cua-cursor-theme"
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

    env = os.environ.copy()
    env.pop("SUDO_USER", None)
    env.pop("CARGO_TARGET_DIR", None)
    env.pop("CUA_DRIVER_LOCAL_INSTALL_DIR", None)
    env.update(
        {
            "HOME": str(tmp_path / "home"),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_SOURCE_SHA": "a" * 40,
            "CUA_DRIVER_LOCAL_HOME": str(tmp_path / "local-home"),
        }
    )
    return scripts_dir, fake_bin, env


@pytest.mark.parametrize(
    "flag_form",
    [["--bin-dir", "{bin}"], ["--bin-dir={bin}"]],
    ids=["separate", "equals"],
)
def test_dispatcher_forwards_bin_dir_override(tmp_path: Path, flag_form: list[str]) -> None:
    """--bin-dir is documented by install-local.sh and forwarded verbatim; the
    helper must accept both spellings and honor them over the env default."""
    scripts_dir, _, env = _linux_fixture(tmp_path)
    flag_bin = tmp_path / "flag-bin"
    env["CUA_DRIVER_LOCAL_INSTALL_DIR"] = str(tmp_path / "env-bin")
    args = [arg.format(bin=flag_bin) for arg in flag_form]

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / DISPATCHER.name), *args],
        cwd=scripts_dir.parent,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert (flag_bin / "cua-driver-local").read_text() == "fresh driver\n"
    assert not (tmp_path / "env-bin").exists()


def test_relative_bin_dir_is_rejected(tmp_path: Path) -> None:
    """A relative bin dir would land inside the Cargo workspace (the symlink is
    created after cd'ing there) and uninstall-local.sh could never remove it."""
    scripts_dir, _, env = _linux_fixture(tmp_path)

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / DISPATCHER.name), "--bin-dir", "relative/bin"],
        cwd=scripts_dir.parent,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 2, result.stdout + result.stderr
    assert "absolute path" in result.stderr


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="ETXTBSY on a running binary is Linux-specific"
)
def test_reinstall_over_a_running_driver(tmp_path: Path) -> None:
    """Staging must replace the versioned binary by rename, not write through it.

    The version tag is stable per build config, so every rebuild targets the
    same path. If a previous cua-driver-local is still executing out of it, a
    write-in-place `cp` fails with ETXTBSY ("Text file busy") and the install
    dies mid-stage. Reproduce that with a real running executable.
    """
    scripts_dir, _, env = _linux_fixture(tmp_path)
    versioned = (
        tmp_path / "local-home/packages/releases/0.0.0-local-debug-x86_64-unknown-linux-gnu"
    )
    versioned.mkdir(parents=True)
    busy = versioned / "cua-driver-local"
    shutil.copy2("/bin/sleep", busy)

    running = subprocess.Popen([str(busy), "60"])
    try:
        result = subprocess.run(
            ["/bin/bash", str(scripts_dir / DISPATCHER.name)],
            cwd=scripts_dir.parent,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
    finally:
        running.terminate()
        running.wait(timeout=10)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "Text file busy" not in result.stderr
    assert busy.read_text() == "fresh driver\n"
    # The rename must not leave the temp file behind.
    assert not list(versioned.glob("*.stage.*"))


def test_bin_dir_without_value_is_rejected(tmp_path: Path) -> None:
    scripts_dir, _, env = _linux_fixture(tmp_path)

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / DISPATCHER.name), "--bin-dir"],
        cwd=scripts_dir.parent,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 2, result.stdout + result.stderr
    assert "--bin-dir requires a value" in result.stderr
