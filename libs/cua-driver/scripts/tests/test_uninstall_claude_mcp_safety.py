"""Regression coverage for release-uninstaller Claude MCP safety gaps."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import shlex
import sys

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def _executable(path: Path, body: str = "exit 0") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def _release_fixture(tmp_path: Path, *, pgrep_body: str = "exit 1") -> tuple[Path, Path, Path, dict[str, str]]:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    release_binary = home / ".cua-driver/packages/current/cua-driver"
    _executable(release_binary)

    launcher = home / ".local/bin/cua-driver"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.symlink_to(release_binary)

    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pgrep", pgrep_body)
    _executable(
        fake_bin / "id",
        "if [ \"$1\" = -u ]; then printf '1000\\n'; else /usr/bin/id \"$@\"; fi",
    )
    _executable(fake_bin / "pkill")
    _executable(fake_bin / "systemctl")

    # Preserve real rm semantics, but never allow a fixture to remove host state.
    _executable(fake_bin / "rm", f"exec {shlex.quote(sys.executable)} {shlex.quote(str(fake_bin / 'safe_rm.py'))} \"$@\"")
    (fake_bin / "safe_rm.py").write_text(
        "import os, pathlib, sys\n"
        f"root = pathlib.Path({str(tmp_path.resolve())!r})\n"
        "for arg in sys.argv[1:]:\n"
        "    if arg.startswith('-'): continue\n"
        "    path = pathlib.Path(arg)\n"
        "    assert root == path.parent.resolve() or root in path.parent.resolve().parents, arg\n"
        "os.execv('/bin/rm', ['/bin/rm', *sys.argv[1:]])\n"
    )
    env = os.environ.copy()
    for key in list(env):
        if key.startswith("CUA_DRIVER_"):
            env.pop(key)
    env.update({"HOME": str(home), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    return home, release_binary, launcher, env


def _run(env: dict[str, str], *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["/bin/bash", str(UNINSTALL), *args],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


def test_legacy_substring_in_unrelated_argument_is_preserved(tmp_path: Path) -> None:
    home, _, _, env = _release_fixture(tmp_path)
    claude_json = home / ".claude.json"
    original = {
        "mcpServers": {
            "notes-server": {
                "command": "/usr/bin/other",
                "args": ["/work/cua-driver-rs-notes"],
            }
        }
    }
    claude_json.write_text(json.dumps(original), encoding="utf-8")

    result = _run(env)

    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(claude_json.read_text(encoding="utf-8")) == original


def test_malformed_claude_config_fails_before_release_or_daemon_mutation(tmp_path: Path) -> None:
    pgrep_marker = tmp_path / "pgrep-called"
    home, release_binary, launcher, env = _release_fixture(
        tmp_path,
        pgrep_body=f"printf called > {pgrep_marker}; exit 1",
    )
    claude_json = home / ".claude.json"
    malformed = '{"mcpServers":'
    claude_json.write_text(malformed, encoding="utf-8")

    result = _run(env)

    assert result.returncode != 0
    assert "could not read Claude config" in result.stderr
    assert claude_json.read_text(encoding="utf-8") == malformed
    assert release_binary.exists()
    assert launcher.is_symlink()
    assert not pgrep_marker.exists(), "daemon inspection ran before Claude config validation"
    assert "cua-driver uninstalled." not in result.stdout


def _config(home: Path, servers: dict) -> Path:
    path = home / ".claude.json"
    path.write_text(json.dumps({"mcpServers": servers}))
    return path


@pytest.mark.parametrize("owned", [False, True])
def test_original_symlink_dotdot_command_controls_ownership(tmp_path: Path, owned: bool) -> None:
    home, release, _, env = _release_fixture(tmp_path)
    normal, other = tmp_path / "normal", tmp_path / "other"
    normal.mkdir()
    (other / "child").mkdir(parents=True)
    (normal / "jump").symlink_to(other / "child", target_is_directory=True)
    foreign = (normal if owned else other) / "cua-driver"
    _executable(foreign)
    ((other if owned else normal) / "cua-driver").symlink_to(release)
    command = str(normal / "jump") + "/../cua-driver"
    assert Path(command).samefile(release if owned else foreign)
    config = _config(home, {"entry": {"command": command}})
    result = _run(env)
    assert result.returncode == 0, result.stdout + result.stderr
    assert ("entry" in json.loads(config.read_text())["mcpServers"]) is not owned
    assert foreign.exists()
    assert not release.exists()
    assert Path(command).exists() is not owned


@pytest.mark.parametrize("purge", [False, True])
def test_selected_home_symlink_matches_actual_removal(tmp_path: Path, purge: bool) -> None:
    home, _, launcher, env = _release_fixture(tmp_path)
    actual = tmp_path / "actual-home"
    release = actual / "packages/current/cua-driver"
    _executable(release)
    selected = tmp_path / "selected-home"
    selected.symlink_to(actual, target_is_directory=True)
    env["CUA_DRIVER_HOME"] = str(selected)
    config = _config(home, {"entry": {"command": str(release)}})
    result = _run(env, *(('--purge',) if purge else ()))
    assert result.returncode == 0, result.stdout + result.stderr
    assert ("entry" in json.loads(config.read_text())["mcpServers"]) is purge
    assert release.exists() is purge
    assert selected.is_symlink() is not purge
    assert launcher.exists()  # the other installation remains


def test_linux_does_not_claim_app_or_foreign_legacy_name(tmp_path: Path) -> None:
    home, _, _, env = _release_fixture(tmp_path)
    foreign = tmp_path / "foreign"
    _executable(foreign)
    original = {
        "cua-driver-rs": {"command": str(foreign)},
        "mac-release": {"command": "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"},
        "legacy-app": {"command": "/Applications/CuaDriverRs.app/Contents/MacOS/cua-driver"},
    }
    config = _config(home, original)
    result = _run(env)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == original
    assert foreign.exists()


@pytest.mark.parametrize("operation", ["backup", "replace"])
def test_config_write_failure_preserves_release(tmp_path: Path, operation: str) -> None:
    home, release, launcher, env = _release_fixture(tmp_path)
    config = _config(home, {"entry": {"command": str(release)}})
    before = config.read_bytes()
    fake_bin = tmp_path / "fake-bin"
    # Fault injection stays at the existing python process seam, not in production.
    wrapper = fake_bin / "python3"
    wrapper.write_text(
        f"#!{sys.executable}\n"
        "import os, shutil, sys\n"
        "if sys.argv[1:2] == ['-']: sys.argv = sys.argv[1:]\n"
        "def fail(*args, **kwargs): raise OSError('fixture write failure')\n"
        + ("shutil.copyfileobj = fail\nshutil.copy2 = fail\n" if operation == "backup" else "os.replace = fail\n")
        + "exec(compile(sys.stdin.read(), '<uninstaller>', 'exec'))\n"
    )
    wrapper.chmod(0o755)
    result = _run(env)
    assert result.returncode != 0
    assert "fixture write failure" in result.stderr
    assert config.read_bytes() == before
    assert release.exists() and launcher.exists()
    assert not list(home.glob(".claude.json.*.tmp"))
    backups = list(home.glob(".claude.json.bak-*"))
    if operation == "replace":
        assert len(backups) == 1 and backups[0].read_bytes() == before


def test_failed_history_purge_preserves_registration_and_launcher(tmp_path: Path) -> None:
    home, release, launcher, env = _release_fixture(tmp_path)
    _executable(release, "exit 1")
    config = _config(home, {"entry": {"command": str(release)}})
    before = config.read_bytes()
    result = _run(env, "--purge")
    assert result.returncode != 0
    assert "history_purge_incomplete" in result.stderr
    assert config.read_bytes() == before
    assert release.exists() and launcher.exists()


def test_lexical_root_is_not_alternative_ownership_evidence(tmp_path: Path) -> None:
    home, release, _, env = _release_fixture(tmp_path)
    foreign = tmp_path / "foreign/packages/cua-driver"
    _executable(foreign)
    (tmp_path / "foreign/child").mkdir()
    jump = home / ".cua-driver/jump"
    jump.symlink_to(tmp_path / "foreign/child", target_is_directory=True)
    command = str(jump) + "/../packages/cua-driver"
    assert Path(command).samefile(foreign)
    config = _config(home, {"entry": {"command": command}})
    before = config.read_bytes()
    result = _run(env)
    assert result.returncode == 0, result.stdout + result.stderr
    assert config.read_bytes() == before
    assert Path(command).samefile(foreign)
    assert not release.exists()


def test_final_packages_symlink_is_unlinked_not_traversed(tmp_path: Path) -> None:
    home, release, _, env = _release_fixture(tmp_path)
    packages = home / ".cua-driver/packages"
    external = tmp_path / "external-packages"
    packages.rename(external)
    packages.symlink_to(external, target_is_directory=True)
    foreign = external / "current/cua-driver"
    config = _config(home, {"entry": {"command": str(foreign)}})
    before = config.read_bytes()
    result = _run(env)
    assert result.returncode == 0, result.stdout + result.stderr
    assert config.read_bytes() == before
    assert foreign.exists()
    assert not packages.is_symlink()
    assert not release.exists()


@pytest.mark.parametrize("suffix", ["/", "/.", "/..", "/../.cua-driver"])
def test_ambiguous_purge_operand_fails_before_mutation(tmp_path: Path, suffix: str) -> None:
    marker = tmp_path / "process-inspection"
    home, release, launcher, env = _release_fixture(tmp_path, pgrep_body=f"touch {marker}; exit 1")
    config = _config(home, {"entry": {"command": str(release)}})
    before = config.read_bytes()
    env["CUA_DRIVER_HOME"] = str(home / ".cua-driver") + suffix
    result = _run(env, "--purge")
    assert result.returncode != 0
    assert "unsafe release removal operand" in result.stderr
    assert config.read_bytes() == before
    assert release.exists() and launcher.exists()
    assert not marker.exists()


def test_original_missing_component_and_dangling_target_are_preserved(tmp_path: Path) -> None:
    home, release, launcher, env = _release_fixture(tmp_path)
    dangling = tmp_path / "dangling"
    dangling.symlink_to(release.parent / "missing")
    original = {
        "missing-step": {"command": str(release.parent / "missing") + "/../cua-driver"},
        "owned-looking-dangling": {"command": str(dangling)},
        "tilde": {"command": "~/.local/bin/cua-driver"},
    }
    config = _config(home, original)
    before = config.read_bytes()
    result = _run(env)
    assert result.returncode == 0, result.stdout + result.stderr
    assert config.read_bytes() == before
    assert not release.exists() and not launcher.exists()
    assert dangling.is_symlink()


def test_earlier_payload_cannot_break_later_removal_operand(tmp_path: Path) -> None:
    home, release, _, env = _release_fixture(tmp_path)
    external = tmp_path / "external-home"
    executable = external / "packages/current/cua-driver"
    _executable(executable)
    bridge = release.parents[1] / "bridge"
    bridge.symlink_to(external, target_is_directory=True)
    (home / ".cua-driver-rs").symlink_to(bridge, target_is_directory=True)
    config = _config(home, {"entry": {"command": str(executable)}})
    result = _run(env)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == {}
    assert not executable.exists()
    assert not release.exists()


def test_trailing_newline_operand_never_selects_neighbor(tmp_path: Path) -> None:
    home, _, _, env = _release_fixture(tmp_path)
    selected = tmp_path / "selected\n"
    neighbor = tmp_path / "selected"
    intended = selected / "packages/current/cua-driver"
    foreign = neighbor / "packages/current/cua-driver"
    _executable(intended)
    _executable(foreign)
    env["CUA_DRIVER_HOME"] = str(selected)
    config = _config(home, {"owned": {"command": str(intended)}, "foreign": {"command": str(foreign)}})
    result = _run(env, "--purge")
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == {"foreign": {"command": str(foreign)}}
    assert not intended.exists()
    assert foreign.exists()


def test_relative_home_ignores_inherited_cdpath(tmp_path: Path) -> None:
    home, _, _, env = _release_fixture(tmp_path)
    intended = tmp_path / "selected/packages/current/cua-driver"
    foreign = tmp_path / "search/selected/packages/current/cua-driver"
    _executable(intended)
    _executable(foreign)
    env.update({"CUA_DRIVER_HOME": "selected", "CDPATH": str(tmp_path / "search")})
    config = _config(home, {"owned": {"command": str(intended)}, "foreign": {"command": str(foreign)}})
    result = subprocess.run(["/bin/bash", str(UNINSTALL)], cwd=tmp_path, env=env,
                            text=True, capture_output=True, check=False)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == {"foreign": {"command": str(foreign)}}
    assert not intended.exists()
    assert foreign.exists()
