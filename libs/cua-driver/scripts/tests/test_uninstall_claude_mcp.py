"""Claude MCP scrubbing in the release uninstaller.

`cua-driver mcp-config --client claude` registers the server as
`cua-computer-use`, so an uninstaller that only matched `cua-driver-rs` left a
live registration behind while reporting a clean uninstall.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def _executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def _run(
    tmp_path: Path,
    servers: dict,
    *,
    rust_marker: bool,
    with_claude_cli: bool = False,
) -> tuple[dict, str]:
    """Uninstall against a fake HOME and return the surviving servers."""

    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    (home / ".local/bin").mkdir(parents=True)
    if rust_marker:
        (home / ".cua-driver/packages/current").mkdir(parents=True)

    claude_json = home / ".claude.json"

    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pkill", "exit 0")
    _executable(fake_bin / "systemctl", "exit 0")
    if with_claude_cli:
        # `claude mcp remove` matches on the name alone, so the stub has to
        # mutate the config the way the real CLI does — a stub that only
        # records its arguments cannot show an entry being deleted after the
        # scrub deliberately kept it.
        _executable(
            fake_bin / "claude",
            f'echo "$@" >> "{tmp_path / "claude-calls"}"\n'
            f'[ "$1" = mcp ] && [ "$2" = remove ] || exit 1\n'
            f'exec {shlex.quote(sys.executable)} '
            f'{shlex.quote(str(Path(__file__).with_name("_fake_claude_remove.py")))} '
            f'"{claude_json}" "$3"\n',
        )

    claude_json.write_text(json.dumps(servers), encoding="utf-8")

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


def _user_servers(config: dict) -> set:
    return set(config["mcpServers"])


def test_registered_claude_server_is_removed(tmp_path: Path) -> None:
    """The reported bug: the name the CLI actually registers survived."""

    config, stdout = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "/home/u/.local/bin/cua-driver",
                    "args": ["mcp"],
                },
                "unrelated": {"command": "/usr/bin/other", "args": ["mcp"]},
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == {"unrelated"}
    assert "removed Claude MCP registration(s): user:cua-computer-use" in stdout


def test_project_scoped_registrations_are_scrubbed_too(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {},
            "projects": {
                "/work/repo": {
                    "mcpServers": {
                        "cua-computer-use": {
                            "command": "/home/u/.local/bin/cua-driver",
                            "args": ["mcp"],
                        },
                        "unrelated": {"command": "/usr/bin/other"},
                    }
                }
            },
        },
        rust_marker=True,
    )

    assert set(config["projects"]["/work/repo"]["mcpServers"]) == {"unrelated"}


def test_a_renamed_key_still_matches_on_the_launcher(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "my-driver": {"command": "/home/u/.local/bin/cua-driver", "args": ["mcp"]}
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == set()


def test_a_name_without_a_readable_command_is_still_removed(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {"mcpServers": {"cua-computer-use": {"args": ["mcp"]}}},
        rust_marker=True,
    )

    assert _user_servers(config) == set()


def test_the_legacy_name_is_removed_without_a_marker(tmp_path: Path) -> None:
    """`cua-driver-rs` was only ever ours, so it needs no disambiguation."""

    config, _ = _run(
        tmp_path,
        {"mcpServers": {"cua-driver-rs": {"command": "/x/cua-driver-rs", "args": ["mcp"]}}},
        rust_marker=False,
    )

    assert _user_servers(config) == set()


def test_shared_names_are_kept_without_a_rust_marker(tmp_path: Path) -> None:
    """A Swift-only Mac used the same names; leave its registrations alone."""

    config, stdout = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "/home/u/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=False,
    )

    assert _user_servers(config) == {"cua-computer-use"}
    assert "no Claude MCP registrations for cua-driver found" in stdout


def test_another_installs_launcher_is_left_to_its_own_uninstaller(tmp_path: Path) -> None:
    """A side-by-side launcher registers under the same name and owns its entry."""

    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "/home/u/.local/bin/cua-driver-sidecar",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
    )

    assert _user_servers(config) == {"cua-computer-use"}


def _claude_removals(tmp_path: Path) -> set:
    """Server names the uninstaller asked the `claude` CLI to remove."""

    calls = (tmp_path / "claude-calls").read_text(encoding="utf-8").splitlines()
    return {parts[2] for parts in (call.split() for call in calls) if parts[:2] == ["mcp", "remove"]}


def test_cli_fallback_asks_for_a_name_the_scrub_vouched_for(tmp_path: Path) -> None:
    """Ownership established, so the CLI may clear the name from other scopes."""

    _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "/home/u/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
        with_claude_cli=True,
    )

    assert _claude_removals(tmp_path) == {"cua-driver-rs", "cua-computer-use"}


def test_cli_fallback_asks_only_for_the_unambiguous_name_without_evidence(
    tmp_path: Path,
) -> None:
    """Nothing in the config vouches for a shared name, so do not guess."""

    _run(tmp_path, {"mcpServers": {}}, rust_marker=True, with_claude_cli=True)

    assert _claude_removals(tmp_path) == {"cua-driver-rs"}


def test_cli_fallback_keeps_shared_names_without_a_rust_marker(tmp_path: Path) -> None:
    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "/home/u/.local/bin/cua-driver",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=False,
        with_claude_cli=True,
    )

    assert _claude_removals(tmp_path) == {"cua-driver-rs"}
    assert _user_servers(config) == {"cua-computer-use"}


def test_cli_fallback_does_not_delete_another_installs_registration(tmp_path: Path) -> None:
    """The scrub keeps this entry; the name-only CLI pass must not undo that."""

    config, _ = _run(
        tmp_path,
        {
            "mcpServers": {
                "cua-computer-use": {
                    "command": "/home/u/.local/bin/cua-driver-sidecar",
                    "args": ["mcp"],
                }
            }
        },
        rust_marker=True,
        with_claude_cli=True,
    )

    assert "cua-computer-use" not in _claude_removals(tmp_path)
    assert _user_servers(config) == {"cua-computer-use"}


def test_windows_guidance_names_the_registered_server() -> None:
    """Windows does not auto-edit the config, so the printed command must be right."""

    script = (REPO_ROOT / "libs/cua-driver/scripts/uninstall.ps1").read_text(
        encoding="utf-8-sig"
    )

    assert "claude mcp remove cua-computer-use" in script
    # The legacy name stays for installs that registered under it.
    assert "claude mcp remove cua-driver-rs" in script
