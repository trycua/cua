"""
Generic handlers for all OSes.

Includes:
- DesktopHandler
- FileHandler

"""

import base64
import os
import platform
import subprocess
import webbrowser
from pathlib import Path
from typing import Any, Dict, Optional

from ..utils import wallpaper
from .base import BaseDesktopHandler, BaseFileHandler, BaseWindowHandler

try:
    import pywinctl as pwc
except Exception:  # pragma: no cover
    pwc = None  # type: ignore


def resolve_path(path: str) -> Path:
    """Resolve a path to its absolute path. Expand ~ to the user's home directory.

    Args:
        path: The file or directory path to resolve

    Returns:
        Path: The resolved absolute path
    """
    return Path(path).expanduser().resolve()


# ===== Cross-platform Desktop command handlers =====


class GenericDesktopHandler(BaseDesktopHandler):
    """
    Generic desktop handler providing desktop-related operations.

    Implements:
    - get_desktop_environment: detect current desktop environment
    - set_wallpaper: set desktop wallpaper path
    """

    async def get_desktop_environment(self) -> Dict[str, Any]:
        """
        Get the current desktop environment.

        Returns:
            Dict containing 'success' boolean and either 'environment' string or 'error' string
        """
        try:
            env = wallpaper.get_desktop_environment()
            return {"success": True, "environment": env}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def set_wallpaper(self, path: str) -> Dict[str, Any]:
        """
        Set the desktop wallpaper to the specified path.

        Args:
            path: The file path to set as wallpaper

        Returns:
            Dict containing 'success' boolean and optionally 'error' string
        """
        try:
            file_path = resolve_path(path)
            ok = wallpaper.set_wallpaper(str(file_path))
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}


# ===== Cross-platform window control command handlers =====


class GenericWindowHandler(BaseWindowHandler):
    """
    Cross-platform window management using pywinctl where possible.
    """

    async def open(self, target: str) -> Dict[str, Any]:
        try:
            if target.startswith("http://") or target.startswith("https://"):
                ok = webbrowser.open(target)
                return {"success": bool(ok)}
            path = str(resolve_path(target))
            sys = platform.system().lower()
            if sys == "darwin":
                subprocess.Popen(["open", path])
            elif sys == "linux":
                subprocess.Popen(["xdg-open", path])
            elif sys == "windows":
                os.startfile(path)  # type: ignore[attr-defined]
            else:
                return {"success": False, "error": f"Unsupported OS: {sys}"}
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def launch(self, app: str, args: Optional[list[str]] = None) -> Dict[str, Any]:
        try:
            if args:
                proc = subprocess.Popen([app, *args])
            else:
                # allow shell command like "libreoffice --writer"
                proc = subprocess.Popen(app, shell=True)
            return {"success": True, "pid": proc.pid}
        except Exception as e:
            return {"success": False, "error": str(e)}

    def _get_window_by_id(self, window_id: int | str) -> Optional[Any]:
        if pwc is None:
            raise RuntimeError("pywinctl not available")
        # Find by native handle among Window objects; getAllWindowsDict keys are titles
        try:
            for w in pwc.getAllWindows():
                if str(w.getHandle()) == str(window_id):
                    return w
            return None
        except Exception:
            return None

    async def get_current_window_id(self) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            win = pwc.getActiveWindow()
            if not win:
                return {"success": False, "error": "No active window"}
            return {"success": True, "window_id": win.getHandle()}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_application_windows(self, app: str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            wins = pwc.getWindowsWithTitle(app, condition=pwc.Re.CONTAINS, flags=pwc.Re.IGNORECASE)
            ids = [w.getHandle() for w in wins]
            return {"success": True, "windows": ids}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_window_name(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            return {"success": True, "name": w.title}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_window_size(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            width, height = w.size
            return {"success": True, "width": int(width), "height": int(height)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_window_position(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            x, y = w.position
            return {"success": True, "x": int(x), "y": int(y)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def set_window_size(
        self, window_id: int | str, width: int, height: int
    ) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            ok = w.resizeTo(int(width), int(height))
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def set_window_position(self, window_id: int | str, x: int, y: int) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            ok = w.moveTo(int(x), int(y))
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def maximize_window(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            ok = w.maximize()
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def minimize_window(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            ok = w.minimize()
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def activate_window(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            ok = w.activate()
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def close_window(self, window_id: int | str) -> Dict[str, Any]:
        try:
            if pwc is None:
                return {"success": False, "error": "pywinctl not available"}
            w = self._get_window_by_id(window_id)
            if not w:
                return {"success": False, "error": "Window not found"}
            ok = w.close()
            return {"success": bool(ok)}
        except Exception as e:
            return {"success": False, "error": str(e)}


# ===== Cross-platform file system command handlers =====


class GenericFileHandler(BaseFileHandler):
    """
    Generic file handler that provides file system operations for all operating systems.

    This class implements the BaseFileHandler interface and provides methods for
    file and directory operations including reading, writing, creating, and deleting
    files and directories.
    """

    async def file_exists(self, path: str) -> Dict[str, Any]:
        """
        Check if a file exists at the specified path.

        Args:
            path: The file path to check

        Returns:
            Dict containing 'success' boolean and either 'exists' boolean or 'error' string
        """
        try:
            return {"success": True, "exists": resolve_path(path).is_file()}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def directory_exists(self, path: str) -> Dict[str, Any]:
        """
        Check if a directory exists at the specified path.

        Args:
            path: The directory path to check

        Returns:
            Dict containing 'success' boolean and either 'exists' boolean or 'error' string
        """
        try:
            return {"success": True, "exists": resolve_path(path).is_dir()}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def list_dir(self, path: str) -> Dict[str, Any]:
        """
        List all files and directories in the specified directory.

        Args:
            path: The directory path to list

        Returns:
            Dict containing 'success' boolean and either 'files' list of names or 'error' string
        """
        try:
            return {
                "success": True,
                "files": [
                    p.name for p in resolve_path(path).iterdir() if p.is_file() or p.is_dir()
                ],
            }
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def read_text(self, path: str) -> Dict[str, Any]:
        """
        Read the contents of a text file.

        Args:
            path: The file path to read from

        Returns:
            Dict containing 'success' boolean and either 'content' string or 'error' string
        """
        try:
            return {"success": True, "content": resolve_path(path).read_text()}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def write_text(self, path: str, content: str) -> Dict[str, Any]:
        """
        Write text content to a file.

        Args:
            path: The file path to write to
            content: The text content to write

        Returns:
            Dict containing 'success' boolean and optionally 'error' string
        """
        try:
            resolve_path(path).write_text(content)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def write_bytes(
        self, path: str, content_b64: str, append: bool = False
    ) -> Dict[str, Any]:
        """
        Write binary content to a file from base64 encoded string.

        Args:
            path: The file path to write to
            content_b64: Base64 encoded binary content
            append: If True, append to existing file; if False, overwrite

        Returns:
            Dict containing 'success' boolean and optionally 'error' string
        """
        try:
            mode = "ab" if append else "wb"
            with open(resolve_path(path), mode) as f:
                f.write(base64.b64decode(content_b64))
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def read_bytes(
        self, path: str, offset: int = 0, length: Optional[int] = None
    ) -> Dict[str, Any]:
        """
        Read binary content from a file and return as base64 encoded string.

        Args:
            path: The file path to read from
            offset: Byte offset to start reading from
            length: Number of bytes to read; if None, read entire file from offset

        Returns:
            Dict containing 'success' boolean and either 'content_b64' string or 'error' string
        """
        try:
            file_path = resolve_path(path)
            with open(file_path, "rb") as f:
                if offset > 0:
                    f.seek(offset)

                if length is not None:
                    content = f.read(length)
                else:
                    content = f.read()

            return {"success": True, "content_b64": base64.b64encode(content).decode("utf-8")}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_file_size(self, path: str) -> Dict[str, Any]:
        """
        Get the size of a file in bytes.

        Args:
            path: The file path to get size for

        Returns:
            Dict containing 'success' boolean and either 'size' integer or 'error' string
        """
        try:
            file_path = resolve_path(path)
            size = file_path.stat().st_size
            return {"success": True, "size": size}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def delete_file(self, path: str) -> Dict[str, Any]:
        """
        Delete a file at the specified path.

        Args:
            path: The file path to delete

        Returns:
            Dict containing 'success' boolean and optionally 'error' string
        """
        try:
            resolve_path(path).unlink()
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def create_dir(self, path: str) -> Dict[str, Any]:
        """
        Create a directory at the specified path.

        Creates parent directories if they don't exist and doesn't raise an error
        if the directory already exists.

        Args:
            path: The directory path to create

        Returns:
            Dict containing 'success' boolean and optionally 'error' string
        """
        try:
            resolve_path(path).mkdir(parents=True, exist_ok=True)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def delete_dir(self, path: str) -> Dict[str, Any]:
        """
        Delete an empty directory at the specified path.

        Args:
            path: The directory path to delete

        Returns:
            Dict containing 'success' boolean and optionally 'error' string
        """
        try:
            resolve_path(path).rmdir()
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}
