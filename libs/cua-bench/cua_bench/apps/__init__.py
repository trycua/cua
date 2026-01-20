"""App Registry for cua-bench.

A decorator-based API for registering platform-specific app installers and launchers.
Makes it easy for contributors to add support for new applications.

Example - Defining an app:

    # cua_bench/apps/godot.py
    from cua_bench.apps import App, install, launch

    class Godot(App):
        name = "godot"
        description = "Godot game engine"

        @install("linux")
        async def install_linux(session, *, with_shortcut=True, version="4.2.1"):
            await session.run_command(
                f"cd ~/Desktop && "
                f"wget -q https://github.com/godotengine/godot/releases/download/{version}-stable/Godot_v{version}-stable_linux.x86_64.zip && "
                f"unzip -q Godot_v{version}-stable_linux.x86_64.zip"
            )
            if with_shortcut:
                await session.run_command(
                    "ln -sf ~/Desktop/Godot_v*_linux.x86_64 ~/Desktop/Godot"
                )

        @install("windows")
        async def install_windows(session, *, with_shortcut=True, version="4.2.1"):
            await session.run_command(f"choco install godot --version={version} -y")

        @launch("linux", "windows")
        async def launch_editor(session, *, project_path=None):
            cmd = "~/Desktop/Godot" if session.os_type == "linux" else "godot"
            if project_path:
                cmd += f" --editor --path {project_path}"
            await session.run_command(f"{cmd} &")

Example - Using in a task:

    @cb.setup_task(split="train")
    async def start(task_cfg: cb.Task, session: cb.DesktopSession):
        # Install app (auto-selects platform)
        await session.install_app("godot", with_shortcut=True, version="4.2.1")

        # Launch app
        await session.launch_app("godot", project_path="~/project")
"""

# macOS first-party apps (use osascript for getters)
# Import app modules to trigger auto-registration via AppMeta metaclass
from . import adobe_photoshop  # noqa: F401
from . import calendar  # noqa: F401
from . import godot  # noqa: F401
from . import notes  # noqa: F401
from . import reminders  # noqa: F401
from . import unity  # noqa: F401
from .registry import (
    App,
    AppRegistry,
    get_app,
    install,
    launch,
    list_apps,
    uninstall,
)

__all__ = [
    "App",
    "install",
    "launch",
    "uninstall",
    "get_app",
    "list_apps",
    "AppRegistry",
]
