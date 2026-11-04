"""Effect-based regressions for release-uninstaller Claude MCP safety."""

import json
from pathlib import Path
import sys

import pytest


def test_legacy_substring_in_unrelated_argument_is_preserved(release_install, tmp_path: Path) -> None:
    f = release_install
    claude_json = f.home / ".claude.json"
    original = {
        "mcpServers": {
            "notes-server": {
                "command": "/usr/bin/other",
                "args": ["/work/cua-driver-rs-notes"],
            }
        }
    }
    claude_json.write_text(json.dumps(original), encoding="utf-8")

    result = f.run()

    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(claude_json.read_text(encoding="utf-8")) == original


def test_malformed_claude_config_fails_before_release_or_daemon_mutation(release_install, tmp_path: Path) -> None:
    pgrep_marker = tmp_path / "pgrep-called"
    f = release_install
    f.executable(f.bin / "pgrep", f"printf called > {pgrep_marker}; exit 1")
    claude_json = f.home / ".claude.json"
    malformed = '{"mcpServers":'
    claude_json.write_text(malformed, encoding="utf-8")

    result = f.run()

    assert result.returncode != 0
    assert "could not read Claude config" in result.stderr
    assert claude_json.read_text(encoding="utf-8") == malformed
    assert f.release.exists()
    assert f.launcher.is_symlink()
    assert not pgrep_marker.exists(), "daemon inspection ran before Claude config validation"
    assert "cua-driver uninstalled." not in result.stdout


@pytest.mark.parametrize("owned", [False, True])
def test_original_symlink_dotdot_command_controls_ownership(release_install, tmp_path: Path, owned: bool) -> None:
    f = release_install
    normal, other = tmp_path / "normal", tmp_path / "other"
    normal.mkdir()
    (other / "child").mkdir(parents=True)
    (normal / "jump").symlink_to(other / "child", target_is_directory=True)
    foreign = (normal if owned else other) / "cua-driver"
    f.executable(foreign)
    ((other if owned else normal) / "cua-driver").symlink_to(f.release)
    command = str(normal / "jump") + "/../cua-driver"
    assert Path(command).samefile(f.release if owned else foreign)
    config = f.write_config({"mcpServers": {"entry": {"command": command}}})
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert ("entry" in json.loads(config.read_text())["mcpServers"]) is not owned
    assert foreign.exists()
    assert not f.release.exists()
    assert Path(command).exists() is not owned


@pytest.mark.parametrize("purge", [False, True])
def test_selected_home_symlink_matches_actual_removal(release_install, tmp_path: Path, purge: bool) -> None:
    f = release_install
    actual = tmp_path / "actual-home"
    external_release = actual / "packages/current/cua-driver"
    f.executable(external_release)
    selected = tmp_path / "selected-home"
    selected.symlink_to(actual, target_is_directory=True)
    f.env["CUA_DRIVER_HOME"] = str(selected)
    config = f.write_config({"mcpServers": {"entry": {"command": str(external_release)}}})
    result = f.run(*(('--purge',) if purge else ()))
    assert result.returncode == 0, result.stdout + result.stderr
    assert ("entry" in json.loads(config.read_text())["mcpServers"]) is purge
    assert external_release.exists() is purge
    assert selected.is_symlink() is not purge
    assert f.launcher.exists()  # the other installation remains


def test_linux_does_not_claim_app_or_foreign_legacy_name(release_install, tmp_path: Path) -> None:
    f = release_install
    foreign = tmp_path / "foreign"
    f.executable(foreign)
    original = {
        "cua-driver-rs": {"command": str(foreign)},
        "mac-release": {"command": "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"},
        "legacy-app": {"command": "/Applications/CuaDriverRs.app/Contents/MacOS/cua-driver"},
    }
    config = f.write_config({"mcpServers": original})
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == original
    assert foreign.exists()


@pytest.mark.parametrize("operation", ["backup", "replace"])
def test_config_write_failure_preserves_release(release_install, tmp_path: Path, operation: str) -> None:
    f = release_install
    config = f.write_config({"mcpServers": {"entry": {"command": str(f.release)}}})
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
    result = f.run()
    assert result.returncode != 0
    assert "fixture write failure" in result.stderr
    assert config.read_bytes() == before
    assert f.release.exists() and f.launcher.exists()
    assert not list(f.home.glob(".claude.json.*.tmp"))
    backups = list(f.home.glob(".claude.json.bak-*"))
    if operation == "replace":
        assert len(backups) == 1 and backups[0].read_bytes() == before


def test_failed_history_purge_preserves_registration_and_launcher(release_install, tmp_path: Path) -> None:
    f = release_install
    f.executable(f.release, "exit 1")
    config = f.write_config({"mcpServers": {"entry": {"command": str(f.release)}}})
    before = config.read_bytes()
    result = f.run("--purge")
    assert result.returncode != 0
    assert "history_purge_incomplete" in result.stderr
    assert config.read_bytes() == before
    assert f.release.exists() and f.launcher.exists()


def test_lexical_root_is_not_alternative_ownership_evidence(release_install, tmp_path: Path) -> None:
    f = release_install
    foreign = tmp_path / "foreign/packages/cua-driver"
    f.executable(foreign)
    (tmp_path / "foreign/child").mkdir()
    jump = f.home / ".cua-driver/jump"
    jump.symlink_to(tmp_path / "foreign/child", target_is_directory=True)
    command = str(jump) + "/../packages/cua-driver"
    assert Path(command).samefile(foreign)
    config = f.write_config({"mcpServers": {"entry": {"command": command}}})
    before = config.read_bytes()
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert config.read_bytes() == before
    assert Path(command).samefile(foreign)
    assert not f.release.exists()


def test_final_packages_symlink_is_unlinked_not_traversed(release_install, tmp_path: Path) -> None:
    f = release_install
    packages = f.home / ".cua-driver/packages"
    external = tmp_path / "external-packages"
    packages.rename(external)
    packages.symlink_to(external, target_is_directory=True)
    foreign = external / "current/cua-driver"
    config = f.write_config({"mcpServers": {"entry": {"command": str(foreign)}}})
    before = config.read_bytes()
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert config.read_bytes() == before
    assert foreign.exists()
    assert not packages.is_symlink()
    assert not f.release.exists()


@pytest.mark.parametrize("suffix", ["/", "/.", "/..", "/../.cua-driver"])
def test_ambiguous_purge_operand_fails_before_mutation(release_install, tmp_path: Path, suffix: str) -> None:
    marker = tmp_path / "process-inspection"
    f = release_install
    f.executable(f.bin / "pgrep", f"touch {marker}; exit 1")
    config = f.write_config({"mcpServers": {"entry": {"command": str(f.release)}}})
    before = config.read_bytes()
    f.env["CUA_DRIVER_HOME"] = str(f.home / ".cua-driver") + suffix
    result = f.run("--purge")
    assert result.returncode != 0
    assert "unsafe release removal operand" in result.stderr
    assert config.read_bytes() == before
    assert f.release.exists() and f.launcher.exists()
    assert not marker.exists()


def test_original_missing_component_and_dangling_target_are_preserved(release_install, tmp_path: Path) -> None:
    f = release_install
    dangling = tmp_path / "dangling"
    dangling.symlink_to(f.release.parent / "missing")
    original = {
        "missing-step": {"command": str(f.release.parent / "missing") + "/../cua-driver"},
        "owned-looking-dangling": {"command": str(dangling)},
        "tilde": {"command": "~/.local/bin/cua-driver"},
    }
    config = f.write_config({"mcpServers": original})
    before = config.read_bytes()
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert config.read_bytes() == before
    assert not f.release.exists() and not f.launcher.exists()
    assert dangling.is_symlink()


def test_earlier_payload_cannot_break_later_removal_operand(release_install, tmp_path: Path) -> None:
    f = release_install
    external = tmp_path / "external-home"
    executable = external / "packages/current/cua-driver"
    f.executable(executable)
    bridge = f.release.parents[1] / "bridge"
    bridge.symlink_to(external, target_is_directory=True)
    (f.home / ".cua-driver-rs").symlink_to(bridge, target_is_directory=True)
    config = f.write_config({"mcpServers": {"entry": {"command": str(executable)}}})
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == {}
    assert not executable.exists()
    assert not f.release.exists()


def test_trailing_newline_operand_never_selects_neighbor(release_install, tmp_path: Path) -> None:
    f = release_install
    selected = tmp_path / "selected\n"
    neighbor = tmp_path / "selected"
    intended = selected / "packages/current/cua-driver"
    foreign = neighbor / "packages/current/cua-driver"
    f.executable(intended)
    f.executable(foreign)
    f.env["CUA_DRIVER_HOME"] = str(selected)
    config = f.write_config({"mcpServers": {"owned": {"command": str(intended)}, "foreign": {"command": str(foreign)}}})
    result = f.run("--purge")
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == {"foreign": {"command": str(foreign)}}
    assert not intended.exists()
    assert foreign.exists()


def test_relative_home_ignores_inherited_cdpath(release_install, tmp_path: Path) -> None:
    f = release_install
    intended = tmp_path / "selected/packages/current/cua-driver"
    foreign = tmp_path / "search/selected/packages/current/cua-driver"
    f.executable(intended)
    f.executable(foreign)
    f.env.update({"CUA_DRIVER_HOME": "selected", "CDPATH": str(tmp_path / "search")})
    config = f.write_config({"mcpServers": {"owned": {"command": str(intended)}, "foreign": {"command": str(foreign)}}})
    result = f.run(cwd=tmp_path)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(config.read_text())["mcpServers"] == {"foreign": {"command": str(foreign)}}
    assert not intended.exists()
    assert foreign.exists()
