from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL_LOCAL = REPO_ROOT / "libs/cua-driver/scripts/uninstall-local.sh"
SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"


def _executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def test_unix_local_uninstall_removes_owned_links_and_preserves_release(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    local_home = home / ".cua-driver-local"
    release_home = home / ".cua-driver"
    local_bin = home / ".local/bin"
    local_cache = home / ".cache/cua-driver-local"
    release_cache = home / ".cache/cua-driver"
    for path in (fake_bin, local_home, release_home, local_bin, local_cache, release_cache):
        path.mkdir(parents=True, exist_ok=True)

    local_marker = local_home / "packages/current/cua-driver-local"
    local_marker.parent.mkdir(parents=True)
    local_marker.write_text("local\n", encoding="utf-8")

    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pkill", "exit 0")
    _executable(fake_bin / "systemctl", "exit 0")

    local_cli = local_bin / "cua-driver-local"
    local_cli.symlink_to(local_home / "packages/current/cua-driver-local")
    release_cli = local_bin / "cua-driver"
    release_cli.symlink_to(release_home / "packages/current/cua-driver")

    local_skill = home / ".agents/skills/cua-driver"
    local_skill.parent.mkdir(parents=True)
    local_skill.symlink_to(local_home / "skills/cua-driver")

    local_unit = home / ".config/systemd/user/cua-driver-local.service"
    release_unit = home / ".config/systemd/user/cua-driver.service"
    local_unit.parent.mkdir(parents=True)
    local_unit.write_text("local\n", encoding="utf-8")
    release_unit.write_text("release\n", encoding="utf-8")

    claude_json = home / ".claude.json"
    claude_json.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "local": {"command": str(local_cli), "args": ["mcp"]},
                    "release": {"command": str(release_cli), "args": ["mcp"]},
                }
            }
        ),
        encoding="utf-8",
    )

    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_LOCAL_HOME": str(local_home),
            "CUA_DRIVER_LOCAL_INSTALL_DIR": str(local_bin),
        }
    )
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--force", "--keep-tcc"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr

    assert not local_cli.exists() and not local_cli.is_symlink()
    assert not local_skill.exists() and not local_skill.is_symlink()
    assert not local_home.exists()
    assert not local_cache.exists()
    assert not local_unit.exists()

    assert release_cli.is_symlink()
    assert release_home.exists()
    assert release_cache.exists()
    assert release_unit.exists()
    remaining = json.loads(claude_json.read_text(encoding="utf-8"))["mcpServers"]
    assert set(remaining) == {"release"}


def test_unix_local_uninstall_keeps_shared_skill_link_owned_by_release(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    release_skill_target = home / ".cua-driver/skills/cua-driver"
    release_skill_target.mkdir(parents=True)
    skill_link = home / ".agents/skills/cua-driver"
    skill_link.parent.mkdir(parents=True)
    skill_link.symlink_to(release_skill_target)
    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pkill", "exit 0")

    env = os.environ.copy()
    env.update({"HOME": str(home), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--force", "--keep-tcc"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert skill_link.is_symlink()
    assert skill_link.resolve() == release_skill_target


def test_local_uninstall_contract_is_explicit_on_both_platforms() -> None:
    unix = (SCRIPTS / "uninstall-local.sh").read_text()
    windows = (SCRIPTS / "uninstall-local.ps1").read_text(encoding="utf-8-sig")

    for token in (
        "/Applications/CuaDriverLocal.app",
        "com.trycua.driver.local",
        ".cua-driver-local",
        "cua-driver-local.service",
        "com.trycua.cua-driver-local.plist",
    ):
        assert token in unix
    for token in (
        "cua-driver-local-serve",
        "cua-driver-uia-local",
        ".cua-driver-local",
        "Programs\\Cua\\cua-driver-local\\bin",
    ):
        assert token in windows
    assert "Test-LocalLinkTarget" in windows
    assert "is_local_target" in unix
    assert "LOCAL_HOME_MARKER" in unix
    assert "$LocalHomeMarker" in windows
    assert "Remove-Item -LiteralPath $HomeDir -Force -Recurse" not in windows
    assert 'rm -rf "$HOME_DIR"' not in unix
    assert "--validate-only" in unix
    assert "$ValidateOnly" in windows


def test_unix_local_uninstall_rejects_release_home_override(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "CUA_DRIVER_LOCAL_HOME": str(home / ".cua-driver"),
        }
    )
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--validate-only"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 2
    assert "release-owned local home" in result.stderr


def test_release_uninstallers_do_not_target_local_identity() -> None:
    for name in ("uninstall.sh", "uninstall.ps1"):
        script = (SCRIPTS / name).read_text(encoding="utf-8-sig")
        assert "CuaDriverLocal" not in script
        assert ".cua-driver-local" not in script
        assert "cua-driver-local-serve" not in script
