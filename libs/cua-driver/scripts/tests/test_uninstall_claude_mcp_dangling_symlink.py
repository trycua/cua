"""Regression coverage for dangling canonical Claude MCP launchers."""

from __future__ import annotations

import importlib.util
from pathlib import Path


_HELPERS_PATH = Path(__file__).with_name("test_uninstall_claude_mcp.py")
_SPEC = importlib.util.spec_from_file_location("cua_uninstall_mcp_test_helpers", _HELPERS_PATH)
assert _SPEC is not None and _SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_HELPERS)


def test_foreign_dangling_canonical_launcher_is_preserved(tmp_path: Path) -> None:
    """A known foreign symlink target is ownership evidence even when missing."""

    fake_bin = tmp_path / "fake-bin"
    _HELPERS._executable(fake_bin / "pgrep", "exit 1")
    _HELPERS._executable(fake_bin / "id", "printf '1000\\n'")

    launcher = tmp_path / "home/.local/bin/cua-driver"
    foreign_target = tmp_path / "other-install/bin/cua-driver"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.symlink_to(foreign_target)

    config, _ = _HELPERS._run(
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

    assert _HELPERS._user_servers(config) == {"cua-computer-use"}
    assert launcher.is_symlink()
    assert launcher.readlink() == foreign_target
