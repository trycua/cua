"""Unity game engine app definition."""

from __future__ import annotations

import asyncio
from typing import Any, Optional

from .registry import App, install, launch, uninstall


class Unity(App):
    """Unity game engine.

    A cross-platform game engine for 2D, 3D, VR, and AR development.

    Install options:
        - version: Unity version (default "2022.3.0f1" LTS)
        - with_shortcut: Create desktop shortcut (default True)

    Launch options:
        - project_path: Path to project directory
        - create_project: Create new project at path (default False)
        - execute_method: Method to execute on launch
        - batch_mode: Run in batch mode (default False)
        - quit: Quit after execution (default False)
    """

    name = "unity"
    description = "Unity game engine for 2D, 3D, VR, and AR development"

    # =========================================================================
    # Linux
    # =========================================================================

    @install("linux")
    async def install_linux(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "2022.3.0f1",
    ) -> None:
        """Install Unity Hub on Linux.

        Unity is typically installed via Unity Hub which manages versions.
        """
        # Install Unity Hub via .deb package
        await session.run_command(
            "wget -qO - https://hub.unity3d.com/linux/keys/public | sudo apt-key add - && "
            'sudo sh -c \'echo "deb https://hub.unity3d.com/linux/repos/deb stable main" > '
            "/etc/apt/sources.list.d/unityhub.list' && "
            "sudo apt-get update && "
            "sudo apt-get install -y unityhub"
        )

        if with_shortcut:
            # Download Unity icon and create .desktop file
            await session.run_command(
                "mkdir -p ~/.local/share/icons && "
                "wget -q -O ~/.local/share/icons/unity-hub.png "
                "'https://img.icons8.com/ios-filled/512/unity.png'"
            )

            desktop_entry = """[Desktop Entry]
Name=Unity Hub
Comment=Unity Game Engine Hub
Exec=/opt/unityhub/unityhub
Icon=~/.local/share/icons/unity-hub.png
Type=Application
Categories=Development;IDE;
Terminal=false
"""
            await session.run_command(
                f"echo '{desktop_entry}' > ~/Desktop/UnityHub.desktop && "
                f"chmod +x ~/Desktop/UnityHub.desktop"
            )

    @launch("linux")
    async def launch_linux(
        session: Any,
        *,
        project_path: Optional[str] = None,
        create_project: bool = False,
        execute_method: Optional[str] = None,
        batch_mode: bool = False,
        quit: bool = False,
    ) -> None:
        """Launch Unity on Linux."""
        # Find Unity Editor (typically in ~/Unity/Hub/Editor/<version>/Editor/)
        cmd = "unityhub"

        if project_path:
            cmd = f"unity-editor -projectPath {project_path}"
            if create_project:
                cmd += " -createProject"
            if batch_mode:
                cmd += " -batchmode"
            if quit:
                cmd += " -quit"
            if execute_method:
                cmd += f" -executeMethod {execute_method}"

        await session.run_command(f"{cmd} &")
        await asyncio.sleep(5)

    @uninstall("linux")
    async def uninstall_linux(session: Any, **kwargs) -> None:
        """Uninstall Unity from Linux."""
        await session.run_command(
            "sudo apt-get remove -y unityhub && "
            "rm -f ~/Desktop/UnityHub"
        )

    # =========================================================================
    # Windows
    # =========================================================================

    @install("windows")
    async def install_windows(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "2022.3.0f1",
    ) -> None:
        """Install Unity Hub on Windows.

        Uses winget or direct download of Unity Hub installer.
        """
        # Try winget first
        result = await session.run_command(
            "winget install -e --id Unity.UnityHub --accept-package-agreements --accept-source-agreements"
        )

        stdout = result.stdout if hasattr(result, "stdout") else str(result)

        # If winget fails, try direct download
        if "error" in stdout.lower() or "failed" in stdout.lower():
            await session.run_command(
                'curl -L -o "%TEMP%\\UnityHubSetup.exe" '
                '"https://public-cdn.cloud.unity3d.com/hub/prod/UnityHubSetup.exe" && '
                '"%TEMP%\\UnityHubSetup.exe" /S'
            )
            await asyncio.sleep(30)  # Wait for installation

    @launch("windows")
    async def launch_windows(
        session: Any,
        *,
        project_path: Optional[str] = None,
        create_project: bool = False,
        execute_method: Optional[str] = None,
        batch_mode: bool = False,
        quit: bool = False,
    ) -> None:
        """Launch Unity on Windows."""
        if project_path:
            # Launch Unity Editor directly with project
            # Find Unity Editor path (default location)
            cmd = '"%ProgramFiles%\\Unity\\Hub\\Editor\\2022.3.0f1\\Editor\\Unity.exe"'
            cmd += f' -projectPath "{project_path}"'

            if create_project:
                cmd += " -createProject"
            if batch_mode:
                cmd += " -batchmode"
            if quit:
                cmd += " -quit"
            if execute_method:
                cmd += f" -executeMethod {execute_method}"

            await session.run_command(f"start /B {cmd}")
        else:
            # Launch Unity Hub
            await session.run_command(
                'start "" "%ProgramFiles%\\Unity Hub\\Unity Hub.exe"'
            )

        await asyncio.sleep(5)

    @uninstall("windows")
    async def uninstall_windows(session: Any, **kwargs) -> None:
        """Uninstall Unity from Windows."""
        await session.run_command(
            'winget uninstall -e --id Unity.UnityHub'
        )

    # =========================================================================
    # macOS
    # =========================================================================

    @install("macos")
    async def install_macos(
        session: Any,
        *,
        with_shortcut: bool = True,
        version: str = "2022.3.0f1",
    ) -> None:
        """Install Unity Hub on macOS."""
        # Download Unity Hub DMG
        await session.run_command(
            "curl -L -o ~/Downloads/UnityHubSetup.dmg "
            '"https://public-cdn.cloud.unity3d.com/hub/prod/UnityHubSetup.dmg" && '
            "hdiutil attach ~/Downloads/UnityHubSetup.dmg && "
            'cp -R "/Volumes/Unity Hub/Unity Hub.app" /Applications/ && '
            'hdiutil detach "/Volumes/Unity Hub" && '
            "rm ~/Downloads/UnityHubSetup.dmg"
        )

        if with_shortcut:
            await session.run_command(
                'ln -sf "/Applications/Unity Hub.app" ~/Desktop/UnityHub'
            )

    @launch("macos")
    async def launch_macos(
        session: Any,
        *,
        project_path: Optional[str] = None,
        create_project: bool = False,
        execute_method: Optional[str] = None,
        batch_mode: bool = False,
        quit: bool = False,
    ) -> None:
        """Launch Unity on macOS."""
        if project_path:
            # Launch Unity Editor with project
            cmd = 'open -a "/Applications/Unity/Hub/Editor/2022.3.0f1/Unity.app" --args'
            cmd += f' -projectPath "{project_path}"'

            if create_project:
                cmd += " -createProject"
            if batch_mode:
                cmd += " -batchmode"
            if quit:
                cmd += " -quit"
            if execute_method:
                cmd += f" -executeMethod {execute_method}"

            await session.run_command(cmd)
        else:
            await session.run_command('open -a "Unity Hub"')

        await asyncio.sleep(5)

    @uninstall("macos")
    async def uninstall_macos(session: Any, **kwargs) -> None:
        """Uninstall Unity from macOS."""
        await session.run_command(
            'rm -rf "/Applications/Unity Hub.app" ~/Desktop/UnityHub'
        )
