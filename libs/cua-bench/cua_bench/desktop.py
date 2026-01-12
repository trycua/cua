"""Desktop environment management for cua-bench."""

import json
import random
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Union

from jinja2 import Template


@dataclass
class Window:
    """Represents a window in the desktop environment."""

    x: int
    y: int
    width: int
    height: int
    title: str
    content: str
    focused: bool = False
    icon: Optional[str] = None
    title_bar_style: str = "hidden"


@dataclass
class DesktopState:
    """State of the unified desktop environment."""

    os_type: str = "win11"  # win11, win10, win7, macos, winxp, win98, android, ios
    width: int = 1024
    height: int = 768
    background: str = "#000"
    windows: List[Window] = field(default_factory=list)
    dock_state: Dict[str, List[Dict[str, str]]] = field(
        default_factory=lambda: {
            "pinned_apps": [],
            "recent_apps": [],
            "pinned_folders": [],
        }
    )
    taskbar_state: Dict[str, List[Dict[str, str]]] = field(
        default_factory=lambda: {
            "pinned_apps": [],
            "open_apps": [],
        }
    )


class Desktop:
    """Desktop environment manager."""

    def __init__(self, env):
        """Initialize desktop.

        Args:
            env: Environment instance
        """
        self.env = env
        self.state = DesktopState()
        self.template = self._load_template()

    def _load_template(self) -> Template:
        """Load the desktop HTML template."""
        template_path = Path(__file__).parent / "www" / "index.html"
        with open(template_path, "r", encoding="utf-8") as f:
            template_content = f.read()
        return Template(template_content)

    def configure(
        self,
        os_type: Optional[str] = None,
        width: Optional[int] = None,
        height: Optional[int] = None,
        background: Optional[str] = None,
        *,
        dock_state: Optional[Dict[str, List[Union[str, Dict[str, str]]]]] = None,
        randomize_dock: bool = True,
        taskbar_state: Optional[Dict[str, List[Union[str, Dict[str, str]]]]] = None,
        randomize_taskbar: bool = True,
    ):
        """Configure desktop appearance.

        Args:
            os_type: OS appearance (win11, win10, win7, macos, winxp, win98, android, ios)
            width: Screen width in pixels
            height: Screen height in pixels
            background: Background color
            dock_state: Explicit dock state to set with keys 'pinned_apps', 'recent_apps', 'pinned_folders'
            randomize_dock: If True, populate dock_state using macOS icon sets
            taskbar_state: Explicit taskbar state to set with keys 'pinned_apps', 'open_apps'
            randomize_taskbar: If True, populate taskbar_state using Windows 11 icon sets
        """
        if os_type is not None:
            self.state.os_type = os_type
        if width is not None:
            self.state.width = width
        if height is not None:
            self.state.height = height
        if background is not None:
            self.state.background = background
        # Dock state handling
        if dock_state is not None:
            # Normalize to list of {icon, title}
            def norm(items: List[Union[str, Dict[str, str]]], kind: str) -> List[Dict[str, str]]:
                out: List[Dict[str, str]] = []
                for it in items or []:
                    if isinstance(it, str):
                        title = (
                            f"{it} app"
                            if kind in ("pinned_apps", "recent_apps")
                            else ("Trash folder" if "trash" in it.lower() else f"{it} folder")
                        )
                        out.append({"icon": it, "title": title})
                    elif isinstance(it, dict):
                        icon = it.get("icon") or it.get("name")
                        title = it.get("title") or (
                            f"{icon} app"
                            if kind in ("pinned_apps", "recent_apps")
                            else (
                                "Trash folder"
                                if icon and "trash" in icon.lower()
                                else f"{icon} folder"
                            )
                        )
                        if icon:
                            out.append({"icon": icon, "title": title})
                return out

            self.state.dock_state = {
                "pinned_apps": norm(dock_state.get("pinned_apps", []), "pinned_apps"),
                "recent_apps": norm(dock_state.get("recent_apps", []), "recent_apps"),
                "pinned_folders": norm(dock_state.get("pinned_folders", []), "pinned_folders"),
            }
        elif randomize_dock:
            # Load icon name lists
            icons_json = Path(__file__).parent / "www" / "iconsets" / "macos.json"
            application_icons: List[str] = []
            try:
                data = json.loads(icons_json.read_text(encoding="utf-8"))
                icons = data.get("icons", {})
                list(icons.get("system_icons", []) or [])
                application_icons = list(icons.get("application_icons", []) or [])
            except Exception:
                application_icons = []

            # Random pinned apps: 1-5 from application_icons
            pin_count = 0
            if application_icons:
                pin_count = random.randint(1, min(5, len(application_icons)))
            pinned_apps_names = random.sample(application_icons, pin_count) if pin_count else []
            # Recent apps: 0-3 exclusive of pinned
            remaining = [a for a in application_icons if a not in pinned_apps_names]
            recent_count = 0
            if remaining:
                recent_count = random.randint(0, min(3, len(remaining)))
            recent_apps_names = random.sample(remaining, recent_count) if recent_count else []
            # Pinned folders: choose one trash icon
            trash_choice_pool = ["FullTrashIcon", "TrashIcon"]
            pinned_folder_icon = random.choice(trash_choice_pool)
            # Build objects with icon and title
            pinned_apps = [{"icon": n, "title": f"{n} app"} for n in pinned_apps_names]
            recent_apps = [{"icon": n, "title": f"{n} app"} for n in recent_apps_names]
            pinned_folders = [{"icon": pinned_folder_icon, "title": "Trash folder"}]
            self.state.dock_state = {
                "pinned_apps": pinned_apps,
                "recent_apps": recent_apps,
                "pinned_folders": pinned_folders,
            }

        # Taskbar state handling
        if taskbar_state is not None:

            def norm_taskbar(items: List[Union[str, Dict[str, str]]]) -> List[Dict[str, str]]:
                out: List[Dict[str, str]] = []
                for it in items or []:
                    if isinstance(it, str):
                        out.append({"icon": it, "title": it})
                    elif isinstance(it, dict):
                        icon = it.get("icon") or it.get("name")
                        title = it.get("title") or (icon or "")
                        if icon:
                            out.append({"icon": icon, "title": title})
                return out

            self.state.taskbar_state = {
                "pinned_apps": norm_taskbar(taskbar_state.get("pinned_apps", [])),
                "open_apps": norm_taskbar(taskbar_state.get("open_apps", [])),
            }
        elif randomize_taskbar:
            # Load Windows 11 application icon list
            win_icons_json = Path(__file__).parent / "www" / "iconsets" / "win11.json"
            win_application_icons: List[str] = []
            try:
                data = json.loads(win_icons_json.read_text(encoding="utf-8"))
                icons = data.get("icons", {})
                win_application_icons = list(icons.get("application_icons", []) or [])
            except Exception:
                win_application_icons = []

            # Random pinned apps: 0-3 if available
            pin_count = 0
            if win_application_icons:
                pin_count = random.randint(0, min(3, len(win_application_icons)))
            pinned_apps_names = random.sample(win_application_icons, pin_count) if pin_count else []

            # Open apps: 0-3 exclusive of pinned
            remaining = [a for a in win_application_icons if a not in pinned_apps_names]
            open_count = 0
            if remaining:
                open_count = random.randint(0, min(3, len(remaining)))
            open_apps_names = random.sample(remaining, open_count) if open_count else []

            pinned_apps_tb = [{"icon": n, "title": n} for n in pinned_apps_names]
            open_apps_tb = [{"icon": n, "title": n} for n in open_apps_names]
            self.state.taskbar_state = {
                "pinned_apps": pinned_apps_tb,
                "open_apps": open_apps_tb,
            }

        self._render()

    def launch(
        self,
        content: str,
        title: str = "Window",
        x: Optional[int] = None,
        y: Optional[int] = None,
        width: int = 600,
        height: int = 400,
        icon: Optional[str] = None,
        use_inner_size: bool = False,
        title_bar_style: str = "default",
    ) -> Window:
        """Launch a new window on the desktop.

        Args:
            content: HTML content for the window body
            title: Window title
            x: X position (auto-calculated if None)
            y: Y position (auto-calculated if None)
            width: Window width
            height: Window height
            use_inner_size: Whether to use the inner size of the window (i.e. content size)

        Returns:
            Window instance
        """
        # Auto-calculate position if not provided
        if x is None:
            x = 100 + (len(self.state.windows) * 30)
        if y is None:
            y = 100 + (len(self.state.windows) * 30)

        # Unfocus all existing windows
        for w in self.state.windows:
            w.focused = False

        if use_inner_size:
            # TODO: improve this
            height += 30

        # Create new window
        window = Window(
            x=x,
            y=y,
            width=width,
            height=height,
            title=title,
            content=content,
            focused=True,
            icon=icon,
            title_bar_style=title_bar_style,
        )

        self.state.windows.append(window)
        self._render()

        return window

    def _render(self):
        """Render the desktop template and update the page."""
        # Initialize playwright if needed
        self.env._init_playwright()

        self.env.page.set_viewport_size({"width": self.state.width, "height": self.state.height})

        # Serve CSS files statically
        self.env.serve_static("css", "www/css")
        self.env.serve_static("iconsets", "www/iconsets")

        # Render template with current state
        html = self.template.render(
            os_type=self.state.os_type,
            width=self.state.width,
            height=self.state.height,
            background=self.state.background,
            windows=self.state.windows,
            dock_state=self.state.dock_state,
            taskbar_state=self.state.taskbar_state,
        )

        # Navigate to a proper origin for localStorage support
        def handle_root(route):
            route.fulfill(status=200, content_type="text/html", body="")

        self.env.page.route("http://127.0.0.1/", handle_root)
        self.env.page.goto("http://127.0.0.1")
        self.env.page.unroute("http://127.0.0.1/")

        # Set the rendered content
        self.env.page.set_content(html)
