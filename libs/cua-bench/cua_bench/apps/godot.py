"""Godot game engine app definition."""

from __future__ import annotations

import asyncio
from typing import Any, Optional

from .registry import App, install, launch, uninstall


class Godot(App):
    """Godot game engine.

    A free and open-source game engine for 2D and 3D game development.

    Install options:
        - version: Godot version (default "4.2.1")
        - with_shortcut: Create desktop shortcut (default True)
        - mono: Install Mono/.NET version (default False)

    Launch options:
        - project_path: Path to project directory
        - editor: Open in editor mode (default True)
        - scene: Specific scene to open
        - headless: Run in headless mode (default False)
    """

    name = "godot"
    description = "Godot game engine for 2D and 3D game development"

    # =========================================================================
    # Linux
    # =========================================================================

    @install("linux")
    async def install_linux(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "4.2.1",
        mono: bool = False,
    ) -> None:
        """Install Godot on Linux."""
        variant = "mono_linux_x86_64" if mono else "linux.x86_64"
        filename = f"Godot_v{version}-stable_{variant}"
        zip_file = f"{filename}.zip"
        download_url = (
            f"https://github.com/godotengine/godot/releases/download/"
            f"{version}-stable/{zip_file}"
        )

        # Download and extract
        await session.run_command(
            f"cd ~/Desktop && "
            f"wget -q {download_url} && "
            f"unzip -q {zip_file} && "
            f"rm {zip_file}"
        )

        # Make executable
        await session.run_command(f"chmod +x ~/Desktop/{filename}")

        if with_shortcut:
            # Create a simpler symlink name
            await session.run_command(
                f"ln -sf ~/Desktop/{filename} ~/Desktop/Godot"
            )

    @launch("linux")
    async def launch_linux(
        session: Any,
        *,
        project_path: Optional[str] = None,
        editor: bool = True,
        scene: Optional[str] = None,
        headless: bool = False,
    ) -> None:
        """Launch Godot on Linux."""
        cmd = "~/Desktop/Godot"

        if headless:
            cmd += " --headless"
        if editor:
            cmd += " --editor"
        if project_path:
            cmd += f" --path {project_path}"
        if scene:
            cmd += f" {scene}"

        await session.run_command(f"{cmd} &")
        await asyncio.sleep(3)  # Wait for Godot to start

    @uninstall("linux")
    async def uninstall_linux(session: Any, **kwargs) -> None:
        """Uninstall Godot from Linux."""
        await session.run_command(
            "rm -f ~/Desktop/Godot ~/Desktop/Godot_v*"
        )

    # =========================================================================
    # Windows
    # =========================================================================

    @install("windows")
    async def install_windows(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "4.2.1",
        mono: bool = False,
    ) -> None:
        """Install Godot on Windows using winget or direct download."""
        # Try winget first, fall back to direct download
        variant = "mono_win64" if mono else "win64.exe"
        filename = f"Godot_v{version}-stable_{variant}"

        if mono:
            zip_file = f"{filename}.zip"
        else:
            zip_file = f"{filename}.zip"

        download_url = (
            f"https://github.com/godotengine/godot/releases/download/"
            f"{version}-stable/{zip_file}"
        )

        # Download and extract to Desktop
        await session.run_command(
            f'cd %USERPROFILE%\\Desktop && '
            f'curl -L -o {zip_file} {download_url} && '
            f'tar -xf {zip_file} && '
            f'del {zip_file}'
        )

        if with_shortcut:
            # Rename to simpler name
            exe_name = f"Godot_v{version}-stable_win64.exe"
            await session.run_command(
                f'ren "%USERPROFILE%\\Desktop\\{exe_name}" "Godot.exe"'
            )

    @launch("windows")
    async def launch_windows(
        session: Any,
        *,
        project_path: Optional[str] = None,
        editor: bool = True,
        scene: Optional[str] = None,
        headless: bool = False,
    ) -> None:
        """Launch Godot on Windows."""
        cmd = '"%USERPROFILE%\\Desktop\\Godot.exe"'

        if headless:
            cmd += " --headless"
        if editor:
            cmd += " --editor"
        if project_path:
            cmd += f" --path {project_path}"
        if scene:
            cmd += f" {scene}"

        await session.run_command(f"start /B {cmd}")
        await asyncio.sleep(3)

    @uninstall("windows")
    async def uninstall_windows(session: Any, **kwargs) -> None:
        """Uninstall Godot from Windows."""
        await session.run_command(
            'del "%USERPROFILE%\\Desktop\\Godot*.exe"'
        )

    # =========================================================================
    # macOS
    # =========================================================================

    @install("macos")
    async def install_macos(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "4.2.1",
        mono: bool = False,
    ) -> None:
        """Install Godot on macOS."""
        variant = "macos.universal"
        filename = f"Godot_v{version}-stable_{variant}"
        zip_file = f"{filename}.zip"
        download_url = (
            f"https://github.com/godotengine/godot/releases/download/"
            f"{version}-stable/{zip_file}"
        )

        # Download and extract
        await session.run_command(
            f"cd ~/Desktop && "
            f"curl -L -o {zip_file} {download_url} && "
            f"unzip -q {zip_file} && "
            f"rm {zip_file}"
        )

        if with_shortcut:
            # Create alias
            await session.run_command(
                f'ln -sf ~/Desktop/Godot.app ~/Desktop/Godot'
            )

    @launch("macos")
    async def launch_macos(
        session: Any,
        *,
        project_path: Optional[str] = None,
        editor: bool = True,
        scene: Optional[str] = None,
        headless: bool = False,
    ) -> None:
        """Launch Godot on macOS."""
        cmd = "open -a ~/Desktop/Godot.app --args"

        if headless:
            cmd += " --headless"
        if editor:
            cmd += " --editor"
        if project_path:
            cmd += f" --path {project_path}"
        if scene:
            cmd += f" {scene}"

        await session.run_command(cmd)
        await asyncio.sleep(3)

    @uninstall("macos")
    async def uninstall_macos(session: Any, **kwargs) -> None:
        """Uninstall Godot from macOS."""
        await session.run_command(
            "rm -rf ~/Desktop/Godot.app ~/Desktop/Godot"
        )
