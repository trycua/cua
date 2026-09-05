"""Ownership-safe Claude MCP cleanup in the release uninstaller."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def _executable(path: Path, body: str = "exit 0") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def _expand(value, *, home: Path, tmp_path: Path):
    if isinstance(value, str):
        return value.format(home=home, tmp=tmp_path)
    if isinstance(value, list):
        return [_expand(item, home=home, tmp_path=tmp_path) for item in value]
    if isinstance(value, dict):
        return {
            key: _expand(item, home=home, tmp_path=tmp_path)
            for key, item in value.items()
        }
    return value


def _run(
    tmp_path: Path,
    config: dict,
    *,
    rust_marker: bool,
    with_claude_cli: bool = False,
    create_canonical_launcher: bool = True,
    custom_release_launchers: tuple[str, ...] = (),
) -> tuple[dict, str]:
    """Run the real release uninstaller against a fake HOME."""

    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    (home / ".local/bin").mkdir(parents=True, exist_ok=True)

    release_binary = home / ".cua-driver/packages/current/cua-driver"
    if rust_marker:
        _executable(release_binary)
        if create_canonical_launcher:
            (home / ".local/bin/cua-driver").symlink_to(release_binary)
        for raw_path in custom_release_launchers:
            launcher = Path(raw_path.format(home=home, tmp=tmp_path))
            launcher.parent.mkdir(parents=True, exist_ok=True)
            launcher.symlink_to(release_binary)

    claude_json = home / ".claude.json"
    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pkill")
    _executable(fake_bin / "systemctl")

    if with_claude_cli:
        _executable(
            fake_bin / "claude",
            f'echo "$@" >> "{tmp_path / "claude-calls"}"\n'
            f'[ "$1" = mcp ] && [ "$2" = remove ] || exit 1\n'
            f'exec {shlex.quote(sys.executable)} '
            f'{shlex.quote(str(Path(__file__).with_name("_fake_claude_remove.py")))} '
            f'"{claude_json}" "$3"',
        )

    expanded = _expand(config, home=home, tmp_path=tmp_path)
    claude_json.write_text(json.dumps(expanded), encoding="utf-8")

    env = os.environ.copy()
    env.update({"HOME": str(home), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL)],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return json.loads(claude_json.read_text(encoding="utf-8")), result.stdout


def _user_servers(config: dict) -> set[str]:
    return set(config["mcpServers"])


def _project_servers(config: dict, project: str = "/work/repo") -> set[str]:
    return set(config["projects"][project]["mcpServers"])


def _claude_removals(tmp_path: Path) -> set[str]:
    calls = (tmp_path / "claude-calls").read_text(encoding="utf-8").splitlines()
    return {
        parts[2]
        for parts in (call.split() for call in calls)
        if parts[:2] == ["mcp", "remove"]
    }


def test_release_registration_is_removed(tmp_path: Path) -> None:
    config, stdout = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{home}/.local/bin/cua-driver",
                    "args": ["mcp"],
                },
                "unrelated": {"command": "/usr/bin/other", "args": ["mcp"]},
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == {"unrelated"}
    assert "removed Claude MCP registration(s): user:cua-computer-use" in stdout


def test_project_release_registration_is_removed(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {},
            "projects": {
                "/work/repo": {
                    "mcpServers": {
                        "cua-computer-use": {
                            "command": "{home}/.local/bin/cua-driver",
                            "args": ["mcp"],
                        },
                        "unrelated": {"command": "/usr/bin/other"},
                    }
                }
            },
        },
        rust_marker=True,
    )

    assert _project_servers(config) == {"unrelated"}


def test_renamed_key_is_removed_when_target_is_release_owned(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "my-driver": {
                    "command": "{home}/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == set()


def test_shared_name_without_command_is_preserved(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {"mcpServers": {"cua-computer-use": {"args": ["mcp"]}}},
        rust_marker=True,
    )

    assert _user_servers(config) == {"cua-computer-use"}


def test_legacy_name_is_removed_without_a_marker(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-driver-rs": {
                    "command": "/x/cua-driver-rs",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=False,
    )

    assert _user_servers(config) == set()


def test_shared_release_name_is_kept_without_rust_marker(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{home}/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=False,
    )

    assert _user_servers(config) == {"cua-computer-use"}


def test_reported_cua_driver_local_registration_is_preserved(tmp_path: Path) -> None:
    """The release uninstaller must not claim the source-built local product."""

    local_launcher = tmp_path / "home/.local/bin/cua-driver-local"
    _executable(local_launcher)
    config, stdout = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{home}/.local/bin/cua-driver-local",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == {"cua-computer-use"}
    assert "preserved local Claude MCP registration(s): user:cua-computer-use" in stdout
    assert "uninstall-local.sh" in stdout


def test_same_filename_at_unrelated_path_is_preserved(tmp_path: Path) -> None:
    other_launcher = tmp_path / "other/bin/cua-driver"
    _executable(other_launcher)
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{tmp}/other/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == {"cua-computer-use"}


def test_custom_bin_launcher_is_removed_when_it_resolves_into_release(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{tmp}/custom-bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
        custom_release_launchers=("{tmp}/custom-bin/cua-driver",),
    )

    assert _user_servers(config) == set()


def test_missing_canonical_launcher_is_safe_fallback(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{home}/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
        create_canonical_launcher=False,
    )

    assert _user_servers(config) == set()


def test_scope_ownership_does_not_leak_to_same_name_in_project(tmp_path: Path) -> None:
    other_launcher = tmp_path / "other/bin/cua-driver"
    _executable(other_launcher)
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{home}/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            },
            "projects": {
                "/work/repo": {
                    "mcpServers": {
                        "cua-computer-use": {
                            "command": "{tmp}/other/bin/cua-driver",
                            "args": ["mcp"],
                        }
                    }
                }
            },
        },
        rust_marker=True,
        with_claude_cli=True,
    )

    assert _user_servers(config) == set()
    assert _project_servers(config) == {"cua-computer-use"}
    assert _claude_removals(tmp_path) == {"cua-driver-rs"}


def test_cli_fallback_only_uses_unambiguous_legacy_name(tmp_path: Path) -> None:
    _run(
        tmp_path,
        {"mcpServers": {}},
        rust_marker=True,
        with_claude_cli=True,
    )

    assert _claude_removals(tmp_path) == {"cua-driver-rs"}


def test_shared_name_never_enters_cli_fallback_after_json_scrub(tmp_path: Path) -> None:
    _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "{home}/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
        with_claude_cli=True,
    )

    assert _claude_removals(tmp_path) == {"cua-driver-rs"}


def test_windows_guidance_uses_registered_user_scope() -> None:
    script = (REPO_ROOT / "libs/cua-driver/scripts/uninstall.ps1").read_text(
        encoding="utf-8-sig"
    )

    assert "claude mcp remove cua-computer-use -s user" in script
    assert "claude mcp remove cua-driver-rs -s user" in script
