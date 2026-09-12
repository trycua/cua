"""Regression coverage for ownership-safe Windows Claude MCP guidance."""

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL_PS1 = REPO_ROOT / "libs/cua-driver/scripts/uninstall.ps1"


def test_windows_guidance_requires_command_ownership_verification() -> None:
    script = UNINSTALL_PS1.read_text(encoding="utf-8-sig")

    assert "Do NOT remove the shared" in script
    assert "verify that the exact user-scope entry's 'command' belongs to this" in script
    assert "Only if that command path is release-owned" in script
    assert "cua-driver-local" in script
    assert "claude mcp remove cua-computer-use -s user" in script
    assert "claude mcp remove cua-driver-rs -s user" in script
