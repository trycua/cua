"""OpenShot video editor app definition."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from .registry import App, install, launch, uninstall

if TYPE_CHECKING:
    from .registry import BoundApp


class OpenShot(App):
    """OpenShot Video Editor.

    A free and open-source video editor for Linux.

    Install options:
        - with_shortcut: Create desktop shortcut (default True)

    Launch options:
        - project_path: Path to project file to open
    """

    name = "openshot"
    description = "OpenShot video editor for creating and editing videos"

    # =========================================================================
    # Linux
    # =========================================================================

    @install("linux")
    async def install_linux(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install OpenShot on Linux via apt.

        Uses the Ubuntu package which supports both amd64 and arm64 architectures.
        See: https://packages.ubuntu.com/jammy/openshot-qt
        """
        # Update package list and install openshot-qt
        await self.session.run_command(
            "sudo apt-get update && sudo apt-get install -y openshot-qt",
            check=True,
        )

        if with_shortcut:
            # Create desktop shortcut
            await self.session.run_command(
                "ln -sf /usr/bin/openshot-qt ~/Desktop/OpenShot",
                check=False,
            )

    @launch("linux")
    async def launch_linux(
        self: "BoundApp",
        *,
        project_path: str | None = None,
    ) -> None:
        """Launch OpenShot on Linux."""
        cmd = "openshot-qt"

        if project_path:
            cmd += f" {project_path}"

        await self.session.run_command(f"{cmd} &", check=False)
        await asyncio.sleep(3)  # Wait for OpenShot to start

    @uninstall("linux")
    async def uninstall_linux(self: "BoundApp", **kwargs) -> None:
        """Uninstall OpenShot from Linux."""
        await self.session.run_command(
            "sudo apt-get remove -y openshot-qt && " "rm -f ~/Desktop/OpenShot",
            check=False,
        )
