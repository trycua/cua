"""KiCad EDA app definition."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Optional

from .registry import App, install, launch, uninstall

if TYPE_CHECKING:
    from .registry import BoundApp


class KiCad(App):
    """KiCad - Open source electronics design automation.

    KiCad is a free software suite for schematic capture and PCB layout.

    Install options:
        - with_shortcut: Create desktop shortcut (default True)

    Launch options:
        - project_path: Path to project directory (optional)
    """

    name = "kicad"
    description = "KiCad - Open source electronics design automation"

    # =========================================================================
    # Linux
    # =========================================================================

    @install("linux")
    async def install_linux(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install KiCad on Linux via AppImage."""
        appimage_url = "https://mirrors.mit.edu/kicad/appimage/stable/kicad-9.0.7-1-x86_64.AppImage"
        appimage_path = "$HOME/Applications/KiCad.AppImage"
        launcher_path = "$HOME/.local/bin/kicad"

        result = await self.session.run_command(
            "mkdir -p \"$HOME/Applications\" \"$HOME/.local/bin\" \"$HOME/.local/share/applications\" && "
            f"(command -v curl >/dev/null 2>&1 && curl -fL \"{appimage_url}\" -o \"{appimage_path}\" || "
            f"command -v wget >/dev/null 2>&1 && wget -q -O \"{appimage_path}\" \"{appimage_url}\" || "
            "python3 -c \"import urllib.request; urllib.request.urlretrieve("
            f"'{appimage_url}', '{appimage_path}')\") && "
            f"chmod +x \"{appimage_path}\" && "
            "printf '%s\n' "
            "'#!/usr/bin/env sh' "
            "'exec \"$HOME/Applications/KiCad.AppImage\" --appimage-extract-and-run \"$@\"' "
            f"> \"{launcher_path}\" && "
            f"chmod +x \"{launcher_path}\"",
            check=False,
        )

        verify = await self.session.run_command(
            f"test -x \"{launcher_path}\" && echo FOUND || echo NOT_FOUND",
            check=False,
        )
        verify_stdout = (
            verify.get("stdout", "") if isinstance(verify, dict) else str(verify)
        ).strip()
        if verify_stdout != "FOUND":
            install_stdout = (
                result.get("stdout", "") if isinstance(result, dict) else str(result)
            )
            install_stderr = result.get("stderr", "") if isinstance(result, dict) else ""
            raise RuntimeError(
                "KiCad AppImage install failed: launcher not found. "
                f"stdout: {install_stdout[-500:]} stderr: {install_stderr[-500:]}"
            )

        if with_shortcut:
            await self.session.run_command(
                "mkdir -p ~/Desktop && "
                "printf '%s\n' "
                "'[Desktop Entry]' "
                "'Name=KiCad' "
                "'Comment=Electronic Design Automation Suite' "
                "'Exec=/bin/sh -lc \"$HOME/.local/bin/kicad\"' "
                "'Icon=applications-engineering' "
                "'Type=Application' "
                "'Categories=Development;Electronics;' "
                "'Terminal=false' "
                "> ~/Desktop/KiCad.desktop && "
                "chmod +x ~/Desktop/KiCad.desktop && "
                "cp ~/Desktop/KiCad.desktop ~/.local/share/applications/KiCad.desktop && "
                "ln -sf \"$HOME/.local/bin/kicad\" ~/Desktop/KiCad",
                check=False,
            )

    @launch("linux")
    async def launch_linux(
        self: "BoundApp",
        *,
        project_path: Optional[str] = None,
    ) -> None:
        """Launch KiCad on Linux."""
        cmd = "$HOME/.local/bin/kicad"
        if project_path:
            cmd += f" '{project_path}'"
        await self.session.run_command(f"{cmd} &", check=False)
        await asyncio.sleep(3)

    @uninstall("linux")
    async def uninstall_linux(self: "BoundApp", **kwargs) -> None:
        """Uninstall KiCad AppImage from Linux."""
        await self.session.run_command(
            "rm -f \"$HOME/Applications/KiCad.AppImage\" && "
            "rm -f \"$HOME/.local/bin/kicad\" && "
            "rm -f \"$HOME/Desktop/KiCad\" \"$HOME/Desktop/KiCad.desktop\" && "
            "rm -f \"$HOME/.local/share/applications/KiCad.desktop\"",
            check=False,
        )

    # =========================================================================
    # Windows
    # =========================================================================

    @install("windows")
    async def install_windows(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install KiCad on Windows via winget."""
        await self.session.run_command(
            "winget install -e --id KiCad.KiCad --accept-package-agreements --accept-source-agreements",
            check=False,
        )
        await asyncio.sleep(30)  # Installer may take a while

    @launch("windows")
    async def launch_windows(
        self: "BoundApp",
        *,
        project_path: Optional[str] = None,
    ) -> None:
        """Launch KiCad on Windows."""
        if project_path:
            # Launch KiCad with project path (opens project)
            await self.session.run_command(
                f'start "" "C:\\Program Files\\KiCad\\8.0\\bin\\kicad.exe" "{project_path}"',
                check=False,
            )
        else:
            await self.session.run_command(
                'start "" "C:\\Program Files\\KiCad\\8.0\\bin\\kicad.exe"',
                check=False,
            )
        await asyncio.sleep(5)

    @uninstall("windows")
    async def uninstall_windows(self: "BoundApp", **kwargs) -> None:
        """Uninstall KiCad from Windows."""
        await self.session.run_command(
            "winget uninstall -e --id KiCad.KiCad",
            check=False,
        )

    # =========================================================================
    # macOS
    # =========================================================================

    @install("macos")
    async def install_macos(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install KiCad on macOS via Homebrew."""
        await self.session.run_command(
            "brew install --cask kicad",
            check=False,
        )

    @launch("macos")
    async def launch_macos(
        self: "BoundApp",
        *,
        project_path: Optional[str] = None,
    ) -> None:
        """Launch KiCad on macOS."""
        cmd = 'open -a "KiCad"'
        if project_path:
            cmd += f' --args "{project_path}"'
        await self.session.run_command(cmd, check=False)
        await asyncio.sleep(3)

    @uninstall("macos")
    async def uninstall_macos(self: "BoundApp", **kwargs) -> None:
        """Uninstall KiCad from macOS."""
        await self.session.run_command("brew uninstall --cask kicad", check=False)
