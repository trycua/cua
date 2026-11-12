"""Ownership and scope contracts through the real release uninstaller."""

import json
from pathlib import Path
import shlex
import shutil

import pytest


@pytest.mark.parametrize("scope,name,custom", [
    pytest.param("user", "cua-computer-use", False, id="canonical-user"),
    pytest.param("project", "cua-computer-use", False, id="canonical-project"),
    pytest.param("user", "my-driver", False, id="renamed-key"),
    pytest.param("user", "cua-computer-use", True, id="custom-launcher"),
])
def test_owned_registration_is_removed(release_install, tmp_path, scope, name, custom):
    f = release_install
    command = f.launcher
    if custom:
        command = tmp_path / "custom-bin/cua-driver"
        command.parent.mkdir()
        command.symlink_to(f.release)
    servers = {name: {"command": str(command), "args": ["mcp"]},
               "unrelated": {"command": "/usr/bin/other", "args": ["mcp"]}}
    data = {"mcpServers": servers} if scope == "user" else {
        "mcpServers": {}, "projects": {"/work/repo": {"mcpServers": servers}}}
    f.write_config(data)
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    actual = json.loads(f.config.read_text())
    remaining = actual["mcpServers"] if scope == "user" else actual["projects"]["/work/repo"]["mcpServers"]
    assert remaining == {"unrelated": servers["unrelated"]}
    assert not f.release.exists()
    if scope == "user" and name == "cua-computer-use" and not custom:
        assert "removed Claude MCP registration(s): user:cua-computer-use" in result.stdout


@pytest.mark.parametrize("case", [
    "missing-command", "legacy-without-marker", "shared-without-marker",
    "local-product", "foreign-filename", "missing-launcher", "foreign-dangling",
])
def test_unproven_registration_is_preserved(release_install, tmp_path, case):
    f = release_install
    command, name = f.launcher, "cua-computer-use"
    if case.endswith("without-marker"):
        f.launcher.unlink()
        shutil.rmtree(f.home / ".cua-driver")
    if case == "legacy-without-marker":
        command, name = Path("/x/cua-driver-rs"), "cua-driver-rs"
    elif case == "local-product":
        command = f.home / ".local/bin/cua-driver-local"
        f.executable(command)
    elif case == "foreign-filename":
        command = tmp_path / "other/bin/cua-driver"
        f.executable(command)
    elif case in ("missing-launcher", "foreign-dangling"):
        f.launcher.unlink()
        if case == "foreign-dangling":
            foreign = tmp_path / "other-install/bin/cua-driver"
            f.launcher.symlink_to(foreign)
    server = {"args": ["mcp"]}
    if case != "missing-command":
        server["command"] = str(command)
    original = {"mcpServers": {name: server}}
    f.write_config(original)
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(f.config.read_text()) == original
    if case == "local-product":
        assert "preserved local Claude MCP registration(s): user:cua-computer-use" in result.stdout
        assert "uninstall-local.sh" in result.stdout
        assert command.exists()
    elif case == "foreign-filename":
        assert command.exists()
    elif case == "foreign-dangling":
        assert f.launcher.is_symlink() and f.launcher.readlink() == foreign


def test_relative_command_is_not_owned_by_the_uninstaller_cwd(release_install):
    f = release_install
    original = {"projects": {"/work/repo": {"mcpServers": {
        "cua-computer-use": {"command": "./cua-driver", "args": ["mcp"]}}}}}
    f.write_config(original)
    result = f.run(cwd=f.release.parent)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(f.config.read_text()) == original


@pytest.mark.parametrize("case", ["empty-config", "owned-user", "foreign-project-same-name"])
def test_no_name_only_cli_fallback(release_install, tmp_path, case):
    f = release_install
    calls = tmp_path / "claude-calls"
    # Any invocation fails the assertion; no fake JSON editor is necessary.
    f.executable(f.bin / "claude", f'printf "%s\\n" "$*" >> {shlex.quote(str(calls))}')
    original = {"mcpServers": {}}
    if case != "empty-config":
        original["mcpServers"]["cua-computer-use"] = {"command": str(f.launcher), "args": ["mcp"]}
    if case == "foreign-project-same-name":
        foreign = tmp_path / "other/bin/cua-driver"
        f.executable(foreign)
        original["projects"] = {"/work/repo": {"mcpServers": {
            "cua-computer-use": {"command": str(foreign), "args": ["mcp"]}}}}
    f.write_config(original)
    result = f.run()
    assert result.returncode == 0, result.stdout + result.stderr
    actual = json.loads(f.config.read_text())
    assert actual["mcpServers"] == {}
    if case == "foreign-project-same-name":
        assert actual["projects"] == original["projects"]
        assert foreign.exists()
    assert not calls.exists(), "uninstaller invoked Claude CLI without per-entry ownership proof"
