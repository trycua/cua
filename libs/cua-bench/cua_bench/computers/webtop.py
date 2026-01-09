from __future__ import annotations

import mimetypes
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

from jinja2 import Template
from playwright.async_api import (
    Browser,
    BrowserContext,
    Page,
    Playwright,
    async_playwright,
)

from .base import DesktopSession, DesktopSetupConfig


class WebDesktopSession(DesktopSession):
    """Playwright-backed desktop session (local webtop)."""

    playwright: Optional[Playwright] = None
    browser: Optional[Browser] = None
    context: Optional[BrowserContext] = None
    _page: Optional[Page] = None

    def __init__(self):
        self.playwright = None
        self.browser = None
        self.context = None
        self._page = None
        self.static_routes: dict[str, Path] = {}
        self.headless = True
        # Desktop-like state (inlined from desktop.py)
        self._pid_counter: int = 0
        self._pid_to_index: dict[str, int] = {}
        self._template: Optional[Template] = None
        self._window_content: dict[str, str] = {}
        self._state: Dict[str, Any] = {
            "os_type": "win11",
            "width": 1024,
            "height": 768,
            "background": "#000",
            "windows": [],  # list[Window]
            "dock_state": {"pinned_apps": [], "recent_apps": [], "pinned_folders": []},
            "taskbar_state": {"pinned_apps": [], "open_apps": []},
            "desktop_icons": [],  # list[DesktopIcon]
        }

    # =========================================================================
    # Async Context Manager & Lifecycle
    # =========================================================================

    async def __aenter__(self) -> "WebDesktopSession":
        """Async context manager entry - initialize and start the session."""
        await self.start()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        """Async context manager exit - cleanup resources."""
        await self.close()

    async def start(
        self,
        config: Optional[DesktopSetupConfig] = None,
        headless: Optional[bool] = None,
    ) -> None:
        """Start the session and configure the desktop.

        Args:
            config: Optional configuration to apply. If not provided, uses defaults.
            headless: If False, runs browser in headed mode. Defaults to True.

        Example:
            async with WebDesktopSession(env) as session:
                await session.screenshot()

            # Or with config
            session = WebDesktopSession(env)
            await session.start(config={"os_type": "macos", "width": 1280})
        """
        if headless is not None:
            self.headless = headless

        await self._init()

        # Load template once
        if self._template is None:
            tpl_path = Path(__file__).parent.parent / "www" / "index.html"
            self._template = Template(tpl_path.read_text(encoding="utf-8"))

        # Apply configuration (use provided config or defaults)
        await self._configure(
            os_type=(
                config.get("os_type", "win11") if config else self._state.get("os_type", "win11")
            ),
            width=(
                int(config.get("width", 1024) or 1024) if config else self._state.get("width", 1024)
            ),
            height=(
                int(config.get("height", 768) or 768) if config else self._state.get("height", 768)
            ),
            background=(
                str(config.get("background", "#000") or "#000")
                if config
                else self._state.get("background", "#000")
            ),
            randomize_apps=bool(config.get("randomize_apps", True)) if config else True,
            time=config.get("time") if config else None,
        )

    @property
    def page(self):
        return self._page

    @property
    def vnc_url(self) -> str:
        """Return the VNC URL for accessing the desktop environment."""
        return "http://localhost:8006/?autoconnect=true"

    async def _init(self) -> None:
        if self.playwright is not None:
            return
        self.playwright = await async_playwright().start()
        self.browser = await self.playwright.chromium.launch(
            headless=self.headless,
            args=[
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--ignore-certificate-errors",
                "--disable-web-security",
            ],
        )
        self.context = await self.browser.new_context()
        self._page = await self.context.new_page()

        async def handle_all_static(route):
            url = route.request.url

            # Handle window content requests
            if "/window/" in url:
                window_id = url.split("/window/", 1)[1].split("?", 1)[0]
                window_content = getattr(self, "_window_content", {})
                if window_id in window_content:
                    await route.fulfill(
                        status=200,
                        content_type="text/html; charset=utf-8",
                        body=window_content[window_id],
                    )
                    return
                else:
                    await route.fulfill(status=404, body="Window not found")
                    return

            # Handle static file requests
            for url_path, local_dir in self.static_routes.items():
                if f"/{url_path}/" in url:
                    file_path = url.split(f"/{url_path}/", 1)[1].split("?", 1)[0]
                    full_path = local_dir / file_path
                    if full_path.exists() and full_path.is_file():
                        content_type, _ = mimetypes.guess_type(str(full_path))
                        if content_type is None:
                            content_type = "application/octet-stream"
                        if content_type.startswith("text/"):
                            content_type += "; charset=utf-8"
                            body = full_path.read_text(encoding="utf-8")
                        else:
                            body = full_path.read_bytes()
                        await route.fulfill(status=200, content_type=content_type, body=body)
                        return
                    else:
                        print(f"Static file not found: {full_path}")
                        await route.fulfill(status=404)
                        return
            await route.continue_()

        await self._page.route("*://127.0.0.1/**", handle_all_static)

    async def serve_static(self, url_path: str, local_path: str) -> None:
        await self._init()
        # Resolve relative to package by default
        local_dir = Path(local_path)
        if not local_dir.is_absolute():
            pkg_relative = Path(__file__).parent.parent / local_path
            if pkg_relative.exists():
                local_dir = pkg_relative
            else:
                # Fallback to CWD (task dir)
                cwd_relative = Path.cwd() / local_path
                if cwd_relative.exists():
                    local_dir = cwd_relative
                else:
                    raise FileNotFoundError(f"Static directory not found: {local_path}")
        self.static_routes[url_path] = local_dir

    async def launch_window(
        self,
        url: Optional[str] = None,
        *,
        html: Optional[str] = None,
        folder: Optional[str] = None,
        title: str = "Window",
        x: Optional[int] = None,
        y: Optional[int] = None,
        width: int = 600,
        height: int = 400,
        icon: Optional[str] = None,
        use_inner_size: bool = False,
        title_bar_style: str = "default",
    ) -> str:
        if url is not None:
            raise ValueError("url is not supported for webtop provider")

        # Handle folder parameter
        if folder is not None:
            # Serve the folder as static content
            import hashlib

            folder_hash = hashlib.md5(folder.encode()).hexdigest()[:8]
            url_path = f"folder_{folder_hash}"

            # Register the folder as a static route
            await self.serve_static(url_path, folder)

            # Set html to load index.html from the served folder - don't wrap in template yet
            html = f'<iframe src="http://127.0.0.1/{url_path}/index.html" style="width: 100%; height: 100%; border: none;"></iframe>'

        if html is None:
            raise ValueError("html or folder is required for webtop provider")

        # # Process iconify-icon elements
        # from cua_bench.iconify import process_icons
        # html = process_icons(html)

        # Create HTML template with Tailwind and Iconify
        # Important: body must have proper flex layout for iframe to fill
        template = (
            "<!doctype html>\n"
            '<html style="height: 100%;">\n'
            "  <head>\n"
            '    <meta charset="UTF-8" />\n'
            '    <meta name="viewport" content="width=device-width, initial-scale=1.0" />\n'
            '    <script src="https://cdn.jsdelivr.net/npm/@tailwindcss/browser@4"></script>\n'
            '    <script src="https://cdn.jsdelivr.net/npm/iconify-icon@3.0.2/dist/iconify-icon.min.js"></script>\n'
            "  </head>\n"
            '  <body style="margin: 0; padding: 0; width: 100%; height: 100%; display: flex; flex-direction: column; overflow: hidden;">\n'
            "{content}\n"
            "  </body>\n"
            "</html>\n"
        )

        # Create full HTML document with the content
        full_html = template.format(content=html)

        # Generate PID first
        self._pid_counter += 1
        pid = str(self._pid_counter)

        # Store the HTML content to serve via HTTP
        self._window_content = getattr(self, "_window_content", {})
        self._window_content[pid] = full_html

        # Create iframe with HTTP URL and PID attribute
        iframe_content = f'<iframe pid="{pid}" src="http://127.0.0.1/window/{pid}" style="width: 100%; height: 100%; border: none; display: block; flex: 1;"></iframe>'

        # Create a new window in state and render
        if x is None:
            x = 100 + (len(self._state["windows"]) * 30)
        if y is None:
            y = 100 + (len(self._state["windows"]) * 30)
        # Unfocus existing
        for w in self._state["windows"]:
            w["focused"] = False
        if use_inner_size:
            height = int(height) + 30
        window: Dict[str, Any] = {
            "x": int(x),
            "y": int(y),
            "width": int(width),
            "height": int(height),
            "title": title,
            "content": iframe_content,
            "focused": True,
            "icon": icon,
            "title_bar_style": title_bar_style,
        }
        self._state["windows"].append(window)
        await self._render()

        # Track index of this window for potential future scoping
        index = max(0, len(self._state["windows"]) - 1)
        self._pid_to_index[pid] = index
        # Tag the DOM element (best effort; template doesn't render pid)
        # Note: evaluate is also async but called sync here - needs fixing
        return pid

    def exec_playwright(self, code: str) -> None:
        if self._page is None:
            raise RuntimeError("Provider not initialized. Call reset() first.")
        exec_globals = {"page": self._page}
        exec(code, exec_globals)

    async def execute_javascript(self, pid: int | str, javascript: str) -> Any:
        if self._page is None:
            raise RuntimeError("Provider not initialized. Call reset() first.")
        # Use Playwright's frameLocator to target the iframe by PID attribute
        frame = self._page.frame_locator(f'iframe[pid="{pid}"]').locator(":root")
        return await frame.evaluate(javascript)

    async def get_element_rect(
        self, pid: int | str, selector: str, *, space: str = "window", timeout: float = 0.5
    ) -> dict[str, Any] | None:
        if self._page is None:
            raise RuntimeError("Provider not initialized. Call reset() first.")

        # JavaScript to execute inside the iframe
        js = (
            f"(() => {{"
            f"const sel = {repr(selector)};"
            f"const el = document.querySelector(sel); if(!el) return null;"
            f"const r = el.getBoundingClientRect();"
            f"return {{x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height)}};"
            f"}})()"
        )

        retry_interval = max(0.1, timeout / 2.0)
        import time

        start_time = time.time()

        while True:
            result = await self.execute_javascript(pid, js)
            if result is not None:
                # If space is "screen", offset by the iframe's position on the desktop
                if space == "screen":
                    # Get the iframe's bounding rect on the desktop page
                    iframe_js = f"(() => {{ const iframe = document.querySelector('iframe[pid=\"{pid}\"]'); if(!iframe) return null; const r = iframe.getBoundingClientRect(); return {{x: Math.round(r.x), y: Math.round(r.y)}}; }})()"
                    iframe_rect = await self._page.evaluate(iframe_js)
                    if iframe_rect:
                        result["x"] += iframe_rect["x"]
                        result["y"] += iframe_rect["y"]
                return result

            elapsed = time.time() - start_time
            if elapsed >= timeout:
                return None

            import asyncio

            await asyncio.sleep(retry_interval)

    async def screenshot(self) -> bytes:
        if self._page is None:
            raise RuntimeError("Provider not initialized. Call reset() first.")
        return await self._page.screenshot()

    async def execute_action(self, action: Any) -> None:
        if self._page is None:
            raise RuntimeError("Provider not initialized. Call reset() first.")

        # Import all action types
        from ..types import (
            ClickAction,
            DoneAction,
            DoubleClickAction,
            DragAction,
            HotkeyAction,
            KeyAction,
            MiddleClickAction,
            MoveToAction,
            RightClickAction,
            ScrollAction,
            TypeAction,
            WaitAction,
        )

        key_map = {
            # Arrow keys
            "left": "ArrowLeft",
            "right": "ArrowRight",
            "up": "ArrowUp",
            "down": "ArrowDown",
            "arrowleft": "ArrowLeft",
            "arrowright": "ArrowRight",
            "arrowup": "ArrowUp",
            "arrowdown": "ArrowDown",
            "leftarrow": "ArrowLeft",
            "rightarrow": "ArrowRight",
            "uparrow": "ArrowUp",
            "downarrow": "ArrowDown",
            # Enter/Return
            "enter": "Enter",
            "return": "Enter",
            "ret": "Enter",
            # Escape
            "escape": "Escape",
            "esc": "Escape",
            # Tab
            "tab": "Tab",
            # Backspace/Delete
            "backspace": "Backspace",
            "back": "Backspace",
            "delete": "Delete",
            "del": "Delete",
            # Space
            "space": "Space",
            " ": "Space",
            # Modifiers
            "ctrl": "Control",
            "control": "Control",
            "alt": "Alt",
            "option": "Alt",
            "shift": "Shift",
            "meta": "Meta",
            "command": "Meta",
            "cmd": "Meta",
            "win": "Meta",
            "windows": "Meta",
            # Function keys
            "f1": "F1",
            "f2": "F2",
            "f3": "F3",
            "f4": "F4",
            "f5": "F5",
            "f6": "F6",
            "f7": "F7",
            "f8": "F8",
            "f9": "F9",
            "f10": "F10",
            "f11": "F11",
            "f12": "F12",
            # Navigation
            "home": "Home",
            "end": "End",
            "pageup": "PageUp",
            "pagedown": "PageDown",
            "pgup": "PageUp",
            "pgdn": "PageDown",
            # Insert
            "insert": "Insert",
            "ins": "Insert",
            # Caps Lock
            "capslock": "CapsLock",
            "caps": "CapsLock",
            # Numpad
            "numpad0": "Numpad0",
            "numpad1": "Numpad1",
            "numpad2": "Numpad2",
            "numpad3": "Numpad3",
            "numpad4": "Numpad4",
            "numpad5": "Numpad5",
            "numpad6": "Numpad6",
            "numpad7": "Numpad7",
            "numpad8": "Numpad8",
            "numpad9": "Numpad9",
            "numpadadd": "NumpadAdd",
            "numpadsubtract": "NumpadSubtract",
            "numpadmultiply": "NumpadMultiply",
            "numpaddivide": "NumpadDivide",
            "numpaddecimal": "NumpadDecimal",
            "numpadenter": "NumpadEnter",
        }

        match action:
            # Mouse click actions
            case ClickAction(x=x, y=y):
                await self._page.mouse.click(x, y)

            case RightClickAction(x=x, y=y):
                await self._page.mouse.click(x, y, button="right")

            case DoubleClickAction(x=x, y=y):
                await self._page.mouse.dblclick(x, y)

            case MiddleClickAction(x=x, y=y):
                await self._page.mouse.click(x, y, button="middle")

            # Drag and move actions
            case DragAction(from_x=fx, from_y=fy, to_x=tx, to_y=ty, duration=_dur):
                await self._page.mouse.move(fx, fy)
                await self._page.mouse.down()
                # Playwright doesn't have a built-in drag with duration, so we'll move and release
                await self._page.mouse.move(tx, ty)
                await self._page.mouse.up()

            case MoveToAction(x=x, y=y, duration=_):
                await self._page.mouse.move(x, y)

            # Scroll actions
            case ScrollAction(direction=direction, amount=amount):
                # Playwright scroll - positive deltaY scrolls down, negative scrolls up
                delta_y = -amount if direction == "up" else amount
                await self._page.mouse.wheel(0, delta_y)

            # Keyboard actions
            case TypeAction(text=text):
                await self._page.keyboard.type(text)

            case KeyAction(key=key):
                # Comprehensive key mapping for Playwright
                key_name = key_map.get(key.lower(), key)
                await self._page.keyboard.press(key_name)

            case HotkeyAction(keys=keys):
                # Map all keys in the combination
                mapped_keys = [key_map.get(k.lower(), k) for k in keys]

                # Press keys in sequence (hold modifiers, press key, release all)
                for key in mapped_keys[:-1]:  # Hold all but last key
                    await self._page.keyboard.down(key)
                await self._page.keyboard.press(mapped_keys[-1])  # Press and release last key
                for key in reversed(mapped_keys[:-1]):  # Release modifiers in reverse order
                    await self._page.keyboard.up(key)

            # Control actions
            case WaitAction(seconds=seconds):
                import asyncio

                await asyncio.sleep(float(seconds))

            case DoneAction():
                return

            case _:
                raise ValueError(f"Unsupported action type: {type(action)}")

    # --- Internal helpers ---
    async def _configure(
        self,
        *,
        os_type: Optional[str] = None,
        width: Optional[int] = None,
        height: Optional[int] = None,
        background: Optional[str] = None,
        dock_state: Optional[Dict[str, List[Union[str, Dict[str, str]]]]] = None,
        taskbar_state: Optional[Dict[str, List[Union[str, Dict[str, str]]]]] = None,
        desktop_icons: Optional[List[Dict[str, str]]] = None,
        randomize_apps: bool = True,
        time: Optional[str] = None,
    ) -> None:
        if os_type is not None:
            self._state["os_type"] = os_type
        if width is not None:
            self._state["width"] = int(width)
        if height is not None:
            self._state["height"] = int(height)
        if background is not None:
            self._state["background"] = background
        if dock_state is not None:
            self._state["dock_state"] = dock_state
        if taskbar_state is not None:
            self._state["taskbar_state"] = taskbar_state
        if desktop_icons is not None:
            self._state["desktop_icons"] = desktop_icons
        if time is not None:
            self._state["time"] = time

        # Randomize dock/taskbar icons when requested and not explicitly provided
        if randomize_apps:
            pkg_root = Path(__file__).parent.parent
            # macOS dock
            if self._state["os_type"].lower() == "macos" and dock_state is None:
                import json
                import random

                icons_json = pkg_root / "www" / "iconsets" / "macos.json"
                application_icons: List[str] = []
                try:
                    data = json.loads(icons_json.read_text(encoding="utf-8"))
                    icons = data.get("icons", {})
                    list(icons.get("system_icons", []) or [])
                    application_icons = list(icons.get("application_icons", []) or [])
                except Exception:
                    pass
                pin_count = 0
                if application_icons:
                    pin_count = random.randint(1, min(5, len(application_icons)))
                pinned_apps_names = random.sample(application_icons, pin_count) if pin_count else []
                remaining = [a for a in application_icons if a not in pinned_apps_names]
                recent_count = 0
                if remaining:
                    recent_count = random.randint(0, min(3, len(remaining)))
                recent_apps_names = random.sample(remaining, recent_count) if recent_count else []
                trash_choice_pool = ["FullTrashIcon", "TrashIcon"]
                pinned_folder_icon = random.choice(trash_choice_pool)
                pinned_apps = [{"icon": n, "title": f"{n} app"} for n in pinned_apps_names]
                recent_apps = [{"icon": n, "title": f"{n} app"} for n in recent_apps_names]
                pinned_folders = [{"icon": pinned_folder_icon, "title": "Trash folder"}]
                self._state["dock_state"] = {
                    "pinned_apps": pinned_apps,
                    "recent_apps": recent_apps,
                    "pinned_folders": pinned_folders,
                }
            # Windows 11 taskbar
            if self._state["os_type"].lower() == "win11" and taskbar_state is None:
                import json
                import random

                win_icons_json = pkg_root / "www" / "iconsets" / "win11.json"
                try:
                    data = json.loads(win_icons_json.read_text(encoding="utf-8"))
                    icons = data.get("icons", {})
                    win_application_icons: List[str] = list(
                        icons.get("application_icons", []) or []
                    )
                except Exception:
                    win_application_icons = []
                pin_count = 0
                if win_application_icons:
                    pin_count = random.randint(0, min(3, len(win_application_icons)))
                pinned_apps_names = (
                    random.sample(win_application_icons, pin_count) if pin_count else []
                )
                remaining = [a for a in win_application_icons if a not in pinned_apps_names]
                open_count = 0
                if remaining:
                    open_count = random.randint(0, min(3, len(remaining)))
                open_apps_names = random.sample(remaining, open_count) if open_count else []
                pinned_apps_tb = [{"icon": n, "title": n} for n in pinned_apps_names]
                open_apps_tb = [{"icon": n, "title": n} for n in open_apps_names]
                self._state["taskbar_state"] = {
                    "pinned_apps": pinned_apps_tb,
                    "open_apps": open_apps_tb,
                }

            # Randomize desktop icons (0-25 icons) when requested and not explicitly provided
            if desktop_icons is None:
                import json
                import random

                desktop_icon_list: List[Dict[str, str]] = []

                # Determine which icon set to use based on OS type
                if self._state["os_type"].lower() == "macos":
                    icons_json = pkg_root / "www" / "iconsets" / "macos.json"
                    try:
                        data = json.loads(icons_json.read_text(encoding="utf-8"))
                        icons = data.get("icons", {})
                        # Use both system and application icons for desktop
                        available_icons = list(icons.get("system_icons", [])) + list(
                            icons.get("application_icons", [])
                        )
                    except Exception:
                        available_icons = []
                elif self._state["os_type"].lower() == "win11":
                    win_icons_json = pkg_root / "www" / "iconsets" / "win11.json"
                    try:
                        data = json.loads(win_icons_json.read_text(encoding="utf-8"))
                        icons = data.get("icons", {})
                        available_icons = list(icons.get("application_icons", []))
                    except Exception:
                        available_icons = []
                else:
                    available_icons = []

                # Generate 0-25 random desktop icons
                if available_icons:
                    icon_count = random.randint(0, min(25, len(available_icons)))
                    if icon_count > 0:
                        selected_icons = random.sample(available_icons, icon_count)
                        for i, icon_name in enumerate(selected_icons):
                            # Create a simple title from the icon name
                            title = icon_name.replace("Icon", "").replace("Folder", "").strip()
                            if not title:
                                title = icon_name
                            desktop_icon_list.append({"icon": icon_name, "title": title})

                self._state["desktop_icons"] = desktop_icon_list
        await self._render()

    async def _render(self) -> None:
        if self._page is None or self._template is None:
            return
        # Set viewport
        await self._page.set_viewport_size(
            {
                "width": self._state["width"],
                "height": self._state["height"],
            }
        )
        # Ensure static routes for css/iconsets (explicit package-relative paths)
        pkg_root = Path(__file__).parent.parent
        await self.serve_static("css", (pkg_root / "www" / "css").absolute().as_posix())
        await self.serve_static("iconsets", (pkg_root / "www" / "iconsets").absolute().as_posix())
        # Render HTML
        html = self._template.render(
            os_type=self._state["os_type"],
            width=self._state["width"],
            height=self._state["height"],
            background=self._state["background"],
            windows=self._state["windows"],
            dock_state=self._state["dock_state"],
            taskbar_state=self._state["taskbar_state"],
            desktop_icons=self._state["desktop_icons"],
        )
        # Navigate to an origin and set content if not already there
        if self._page.url != "http://127.0.0.1":

            async def handle_root(route):
                await route.fulfill(status=200, content_type="text/html", body="")

            await self._page.route("http://127.0.0.1/", handle_root)
            await self._page.goto("http://127.0.0.1")
            await self._page.unroute("http://127.0.0.1/")
        await self._page.set_content(html)
        # Install clock with custom time if specified
        if self._state.get("time"):
            await self._page.clock.install(time=self._state["time"])
            await self._page.evaluate("() => window.updateDateTime()")

    async def close_all_windows(self) -> None:
        self._state["windows"] = []
        self._pid_to_index.clear()
        self._window_content.clear()
        await self._render()

    async def get_snapshot(self):  # type: ignore[override]
        try:
            from ..types import Snapshot, WindowSnapshot
        except Exception:
            return None
        wins: List[WindowSnapshot] = []

        # Load snapshot.js code
        from pathlib import Path

        snapshot_js_path = Path(__file__).resolve().parents[1] / "www" / "js" / "snapshot.js"
        snapshot_js_code = snapshot_js_path.read_text(encoding="utf-8")

        # First, create desktop snapshot by running snapshot.js in self.page
        desktop_html = None
        try:
            js = (
                snapshot_js_code
                + "\n;(() => { try { return window.__td_build_snapshot({ screenOffsetX: 0, screenOffsetY: 0, forceWindowPosition: true }); } catch(e) { return ''; } })();"
            )
            desktop_html = await self.page.evaluate(js)
        except Exception:
            desktop_html = ""

        # Add desktop window snapshot at [0,0,width,height] with pid -1
        wins.append(
            WindowSnapshot(
                window_type="desktop",
                pid="-1",
                url=None,
                html=desktop_html,
                title="Desktop",
                x=0,
                y=0,
                width=self._state["width"],
                height=self._state["height"],
                active=False,
                minimized=False,
            )
        )

        # Then process individual windows
        for pid_str, window_idx in self._pid_to_index.items():
            if window_idx >= len(self._state.get("windows", [])):
                continue
            w = self._state["windows"][window_idx]

            # Get iframe position from desktop HTML using data-bbox attributes
            iframe_x = 0
            iframe_y = 0

            if desktop_html:
                from pyquery import PyQuery as pq

                desktop_doc = pq(desktop_html)
                iframe_elem = desktop_doc(f'iframe[pid="{pid_str}"]')
                iframe_x = int(float(iframe_elem.attr("data-bbox-x") or 0))
                iframe_y = int(float(iframe_elem.attr("data-bbox-y") or 0))
            else:
                # Fallback to window state coordinates
                iframe_x = int(w.get("x", 0))
                iframe_y = int(w.get("y", 0))

            # Create snapshot call with iframe position as screen offsets
            js = (
                snapshot_js_code
                + f"\n;(() => {{ try {{ return window.__td_build_snapshot({{ screenOffsetX: {iframe_x}, screenOffsetY: {iframe_y}, forceWindowPosition: true }}); }} catch(e) {{ return ''; }} }})();"
            )

            # Execute snapshot.js inside the iframe to get the actual HTML content
            win_html = None
            try:
                win_html = await self.execute_javascript(pid_str, js)
            except Exception:
                # Fallback to static content if snapshot fails
                win_html = str(w.get("content", ""))

            wins.append(
                WindowSnapshot(
                    window_type="webview",
                    pid=pid_str,
                    url=None,
                    html=win_html,
                    title=str(w.get("title", "")),
                    x=int(w.get("x", 0)),
                    y=int(w.get("y", 0)),
                    width=int(w.get("width", 0)),
                    height=int(w.get("height", 0)),
                    active=bool(w.get("focused", False)),
                    minimized=False,
                )
            )
        return Snapshot(windows=wins)

    # --- Playwright-like Automation API ---

    async def click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and click its center.

        Uses get_element_rect to fetch element rect in screen space
        and then dispatches a ClickAction via env.step().
        """
        from ..types import ClickAction

        rect = await self.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        await self.env.step(ClickAction(x=cx, y=cy))

    async def right_click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and right-click its center."""
        from ..types import RightClickAction

        rect = await self.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        await self.env.step(RightClickAction(x=cx, y=cy))

    async def close(self) -> None:
        if self._page:
            await self._page.close()
        if self.context:
            await self.context.close()
        if self.browser:
            await self.browser.close()
        if self.playwright:
            await self.playwright.stop()
        self._page = None
        self.context = None
        self.browser = None
        self.playwright = None
