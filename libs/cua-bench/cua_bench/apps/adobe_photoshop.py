"""Adobe Photoshop app definition (via WINE on Linux)."""

from __future__ import annotations

import asyncio
from typing import Any, Optional

from .registry import App, install, launch, uninstall


class AdobePhotoshop(App):
    """Adobe Photoshop running via WINE.

    Supports Photoshop CS6 Portable (commonly used for WINE compatibility).

    Note: This requires a legitimate Photoshop installer/portable version.
    The installer expects the Photoshop files to be available or downloadable.

    Install options:
        - version: Photoshop version ("cs6", "cc2019", etc.) - default "cs6"
        - with_shortcut: Create desktop shortcut (default True)
        - wine_prefix: Custom WINE prefix (default "~/.wine-photoshop")

    Launch options:
        - file_path: Optional PSD file to open
        - wine_prefix: Custom WINE prefix if different from install
    """

    name = "adobe_photoshop"
    description = "Adobe Photoshop image editor (via WINE on Linux, native on Windows)"

    # =========================================================================
    # Linux (via WINE)
    # =========================================================================

    @install("linux")
    async def install_linux(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "cs6",
        wine_prefix: str = "~/.wine-photoshop",
    ) -> None:
        """Install Photoshop on Linux via WINE.

        This installs:
        1. WINE and dependencies
        2. Required Windows components via winetricks
        3. Photoshop CS6 Portable (user must provide installer or use portable)
        """
        # Install WINE and winetricks
        await session.run_command(
            "sudo dpkg --add-architecture i386 && "
            "sudo apt-get update && "
            "sudo apt-get install -y wine64 wine32 winetricks cabextract"
        )

        # Create dedicated WINE prefix for Photoshop
        await session.run_command(f"mkdir -p {wine_prefix}")

        # Initialize 64-bit WINE prefix
        await session.run_command(
            f"WINEPREFIX={wine_prefix} WINEARCH=win64 wineboot --init"
        )
        await asyncio.sleep(5)  # Wait for wineboot to complete

        # Install required dependencies via winetricks
        # Photoshop needs: vcrun2008, vcrun2010, vcrun2012, corefonts, msxml3/6
        deps = [
            "corefonts",
            "vcrun2008",
            "vcrun2010",
            "vcrun2012",
            "vcrun2013",
            "msxml3",
            "msxml6",
            "gdiplus",
            "atmlib",
        ]

        for dep in deps:
            await session.run_command(
                f"WINEPREFIX={wine_prefix} winetricks -q {dep}"
            )
            await asyncio.sleep(2)

        # Set Windows version to Windows 10 for better compatibility
        await session.run_command(
            f"WINEPREFIX={wine_prefix} winetricks -q win10"
        )

        # Create Photoshop directory
        photoshop_dir = f"{wine_prefix}/drive_c/Program Files/Adobe/Photoshop"
        await session.run_command(f'mkdir -p "{photoshop_dir}"')

        # Download Photoshop CS6 Portable (using a common portable distribution)
        # Note: In production, this would point to a legitimate source
        # For now, we'll set up the structure and user can copy files manually
        await session.run_command(
            f'echo "Place Photoshop.exe and required files in {photoshop_dir}" > '
            f'"{photoshop_dir}/README.txt"'
        )

        # Try to download a portable version if available from a configured source
        # This is a placeholder - in real usage, point to your own hosted files
        portable_url = "https://example.com/photoshop-cs6-portable.zip"  # Placeholder

        # Create launcher script
        launcher_script = f"""#!/bin/bash
export WINEPREFIX="{wine_prefix}"
export WINEDEBUG=-all
cd "{photoshop_dir}"
wine64 Photoshop.exe "$@"
"""

        await session.run_command(
            f"echo '{launcher_script}' > ~/Desktop/photoshop.sh && "
            f"chmod +x ~/Desktop/photoshop.sh"
        )

        if with_shortcut:
            # Create .desktop file for proper desktop integration
            desktop_entry = f"""[Desktop Entry]
Name=Adobe Photoshop CS6
Comment=Image Editor (WINE)
Exec=env WINEPREFIX={wine_prefix} wine64 "{photoshop_dir}/Photoshop.exe"
Type=Application
Categories=Graphics;2DGraphics;RasterGraphics;
Icon=photoshop
Terminal=false
"""
            await session.run_command(
                f"echo '{desktop_entry}' > ~/.local/share/applications/photoshop.desktop && "
                f"cp ~/.local/share/applications/photoshop.desktop ~/Desktop/"
            )

    @launch("linux")
    async def launch_linux(
        session: Any,
        *,
        file_path: Optional[str] = None,
        wine_prefix: str = "~/.wine-photoshop",
    ) -> None:
        """Launch Photoshop on Linux via WINE."""
        photoshop_dir = f"{wine_prefix}/drive_c/Program Files/Adobe/Photoshop"

        cmd = (
            f'WINEPREFIX={wine_prefix} WINEDEBUG=-all '
            f'wine64 "{photoshop_dir}/Photoshop.exe"'
        )

        if file_path:
            # Convert Linux path to Windows path for WINE
            cmd += f' "Z:{file_path}"'

        await session.run_command(f"{cmd} &")
        await asyncio.sleep(8)  # Photoshop takes longer to start via WINE

    @uninstall("linux")
    async def uninstall_linux(
        session: Any,
        *,
        wine_prefix: str = "~/.wine-photoshop",
    ) -> None:
        """Uninstall Photoshop from Linux."""
        await session.run_command(
            f"rm -rf {wine_prefix} && "
            f"rm -f ~/Desktop/photoshop.sh && "
            f"rm -f ~/Desktop/photoshop.desktop && "
            f"rm -f ~/.local/share/applications/photoshop.desktop"
        )

    # =========================================================================
    # Windows (native)
    # =========================================================================

    @install("windows")
    async def install_windows(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "cs6",
    ) -> None:
        """Install Photoshop on Windows.

        Note: This is a placeholder. In production, you would either:
        1. Use a pre-installed image with Photoshop
        2. Run the actual installer silently
        3. Use a portable version
        """
        # For WinArena-style tasks, Photoshop is typically pre-installed
        # This just verifies it exists
        result = await session.run_command(
            'if exist "%ProgramFiles%\\Adobe\\Adobe Photoshop*\\Photoshop.exe" '
            '(echo FOUND) else (echo NOT_FOUND)'
        )

        stdout = result.get("stdout", "") if isinstance(result, dict) else str(result)
        if "NOT_FOUND" in stdout:
            # Could attempt to install via chocolatey or other means
            # For now, just note that it needs manual installation
            await session.run_command(
                'echo Photoshop not found. Please install manually. > '
                '%USERPROFILE%\\Desktop\\photoshop_install_note.txt'
            )

    @launch("windows")
    async def launch_windows(
        session: Any,
        *,
        file_path: Optional[str] = None,
    ) -> None:
        """Launch Photoshop on Windows."""
        # Find Photoshop executable
        cmd = 'start "" "%ProgramFiles%\\Adobe\\Adobe Photoshop 2024\\Photoshop.exe"'

        if file_path:
            cmd += f' "{file_path}"'

        await session.run_command(cmd)
        await asyncio.sleep(5)

    @uninstall("windows")
    async def uninstall_windows(session: Any, **kwargs) -> None:
        """Uninstall Photoshop from Windows."""
        # Use Adobe's uninstaller
        await session.run_command(
            'if exist "%ProgramFiles%\\Common Files\\Adobe\\Installers\\*\\Setup.exe" '
            '("%ProgramFiles%\\Common Files\\Adobe\\Installers\\*\\Setup.exe" /uninstall)'
        )
