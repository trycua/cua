from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"


def test_unix_local_install_reports_missing_release_cli_without_aliasing_it() -> None:
    installer = (SCRIPTS / "_install-local-rust.sh").read_text()

    warning = installer.index('if [ ! -e "$RELEASE_BIN" ]; then')
    assert installer.index('RELEASE_BIN="$BIN_DIR/cua-driver"') < warning
    assert installer.index('INSTALLED_BIN="$BIN_DIR/cua-driver-local"') < warning
    assert (
        "Existing MCP clients configured for 'cua-driver' will not use this local build."
        in installer
    )
    assert "$INSTALLED_BIN mcp-config --client codex" in installer
    assert "https://cua.ai/driver/install.sh" in installer
    assert 'ln -sf "$INSTALLED_BIN" "$RELEASE_BIN"' not in installer
    assert 'ln -s "$INSTALLED_BIN" "$RELEASE_BIN"' not in installer


def test_windows_local_install_reports_missing_release_cli_without_aliasing_it() -> None:
    installer = (SCRIPTS / "install-local.ps1").read_text(encoding="utf-8-sig")

    warning = installer.index("if (-not (Test-Path -LiteralPath $releaseBinary -PathType Leaf))")
    assert (
        installer.index(
            "$releaseBinary = Join-Path $env:LOCALAPPDATA "
            '"Programs\\Cua\\cua-driver\\bin\\cua-driver.exe"'
        )
        < warning
    )
    assert installer.index("$installedBinary = Join-Path $VisibleBinDir $BinaryName") < warning
    assert (
        "Existing MCP clients configured for 'cua-driver' will not use this local build."
        in installer
    )
    assert "$installedBinary mcp-config --client codex" in installer
    assert "irm https://cua.ai/driver/install.ps1 | iex" in installer
    assert "New-Item -ItemType SymbolicLink" not in installer[warning:]


def test_migration_warning_runs_after_shared_post_install_hints() -> None:
    unix = (SCRIPTS / "_install-local-rust.sh").read_text()
    windows = (SCRIPTS / "install-local.ps1").read_text(encoding="utf-8-sig")

    assert unix.index('sed "s|{{BINARY}}|$INSTALLED_BIN|g" "$HINTS_TXT"') < unix.index(
        'if [ ! -e "$RELEASE_BIN" ]; then'
    )
    assert windows.index("$hintsRaw -replace") < windows.index(
        "if (-not (Test-Path -LiteralPath $releaseBinary -PathType Leaf))"
    )
