"""Controller adapter that bridges cua-bench session to WAA's expected interface.

WAA evaluators expect a controller object with specific methods. This adapter
wraps the cua-bench session to provide those methods.
"""

import base64
import json
import logging
from typing import Any, Dict, List, Optional

logger = logging.getLogger("winarena.controller")


class WAAControllerAdapter:
    """Adapts cua-bench session to WAA controller interface.

    WAA evaluators call methods like `env.controller.get_file()` or
    `env.controller.execute_python_command()`. This adapter translates
    those calls to the cua-bench session/computer interface.
    """

    def __init__(self, session):
        """Initialize adapter with cua-bench session.

        Args:
            session: A cua-bench DesktopSession (e.g., RemoteDesktopSession)
        """
        self.session = session

    # --- Screenshot and Screen ---

    async def get_screenshot(self) -> Optional[bytes]:
        """Get a screenshot from the VM."""
        try:
            return await self.session.screenshot()
        except Exception as e:
            logger.error(f"Failed to get screenshot: {e}")
            return None

    async def get_vm_screen_size(self) -> Optional[Dict[str, int]]:
        """Get the size of the VM screen."""
        return {"width": getattr(self.session, '_width', 1920),
                "height": getattr(self.session, '_height', 1080)}

    # --- File Operations ---

    async def get_file(self, file_path: str) -> Optional[bytes]:
        """Get a file from the VM as bytes."""
        try:
            return await self.session.read_bytes(file_path)
        except Exception as e:
            logger.error(f"Failed to get file {file_path}: {e}")
            return None

    async def get_file_as_text(self, file_path: str) -> Optional[str]:
        """Get a file from the VM as text."""
        try:
            content = await self.get_file(file_path)
            if content:
                return content.decode("utf-8", errors="replace")
            return None
        except Exception as e:
            logger.error(f"Failed to get file as text {file_path}: {e}")
            return None

    async def get_vm_file_exists_in_path(self, file_name: str, path: str) -> bool:
        """Check if a file exists in the given path."""
        try:
            full_path = f"{path}\\{file_name}" if "\\" in path else f"{path}/{file_name}"
            return await self.session.file_exists(full_path)
        except Exception as e:
            logger.error(f"Failed to check file existence: {e}")
            return False

    async def get_vm_folder_exists_in_path(self, folder_name: str, path: str) -> bool:
        """Check if a folder exists in the given path."""
        try:
            full_path = f"{path}\\{folder_name}" if "\\" in path else f"{path}/{folder_name}"
            return await self.session.directory_exists(full_path)
        except Exception as e:
            logger.error(f"Failed to check folder existence: {e}")
            return False

    async def get_vm_directory_tree(self, path: str) -> Optional[List[str]]:
        """Get directory listing."""
        try:
            return await self.session.list_dir(path)
        except Exception as e:
            logger.error(f"Failed to list directory {path}: {e}")
            return None

    # --- Command Execution ---

    async def execute_shell_command(self, command: str) -> Optional[Dict[str, Any]]:
        """Execute a shell command on the VM."""
        try:
            result = await self.session.run_command(command)
            # Handle different result formats
            if isinstance(result, dict):
                return {
                    "status": "success",
                    "output": result.get("stdout", result.get("output", str(result))),
                    "stderr": result.get("stderr", result.get("error", "")),
                    "returncode": result.get("exit_code", result.get("returncode", 0)),
                }
            return {
                "status": "success",
                "output": result.stdout if hasattr(result, "stdout") else str(result),
                "stderr": result.stderr if hasattr(result, "stderr") else "",
                "returncode": result.return_code if hasattr(result, "return_code") else 0,
            }
        except Exception as e:
            logger.error(f"Failed to execute command: {e}")
            return {"status": "error", "output": "", "stderr": str(e), "returncode": -1}

    async def execute_python_command(self, command: str) -> Optional[Dict[str, Any]]:
        """Execute a Python command on the VM."""
        # Escape quotes in the command
        escaped_command = command.replace('"', '\\"')
        shell_cmd = f'python -c "{escaped_command}"'
        return await self.execute_shell_command(shell_cmd)

    async def execute_python_windows_command(self, command: str) -> Optional[Dict[str, Any]]:
        """Execute a Python command on Windows."""
        if command in ["WAIT", "FAIL", "DONE"]:
            return None
        return await self.execute_python_command(command)

    # --- Path Helpers ---

    async def get_vm_desktop_path(self) -> Optional[str]:
        """Get the desktop path on the VM."""
        result = await self.execute_python_command(
            "import os; print(os.path.join(os.path.expanduser('~'), 'Desktop'))"
        )
        if result and result.get("status") == "success":
            return result["output"].strip()
        return None

    async def get_vm_documents_path(self) -> Optional[str]:
        """Get the documents path on the VM."""
        result = await self.execute_python_command(
            "import os; print(os.path.join(os.path.expanduser('~'), 'Documents'))"
        )
        if result and result.get("status") == "success":
            return result["output"].strip()
        return None

    async def get_vm_platform(self) -> str:
        """Get the platform of the VM."""
        result = await self.execute_python_command("import platform; print(platform.system())")
        if result and result.get("status") == "success":
            return result["output"].strip()
        return "Windows"  # Default for WAA

    # --- Accessibility Tree ---

    async def get_accessibility_tree(self, backend: Optional[str] = None) -> Optional[str]:
        """Get the accessibility tree from the VM."""
        try:
            tree = await self.session.get_accessibility_tree()
            if isinstance(tree, dict):
                return json.dumps(tree)
            return str(tree)
        except Exception as e:
            logger.error(f"Failed to get accessibility tree: {e}")
            return None

    # --- Clipboard ---

    async def get_clipboard_content(self) -> Optional[str]:
        """Get clipboard content from the VM."""
        try:
            # Use PowerShell to get clipboard on Windows
            result = await self.session.run_command('powershell -Command "Get-Clipboard"')
            if isinstance(result, dict):
                return result.get("stdout", result.get("output", ""))
            return str(result)
        except Exception as e:
            logger.error(f"Failed to get clipboard: {e}")
            return None

    async def set_clipboard_content(self, text: str) -> bool:
        """Set clipboard content on the VM."""
        try:
            # Use PowerShell to set clipboard on Windows
            escaped = text.replace("'", "''")
            await self.session.run_command(f"powershell -Command \"Set-Clipboard -Value '{escaped}'\"")
            return True
        except Exception as e:
            logger.error(f"Failed to set clipboard: {e}")
            return False

    # --- Registry (Windows-specific) ---

    async def get_registry_key(self, path: str, value: str) -> Optional[Dict[str, Any]]:
        """Fetch a registry key value."""
        command = f"powershell -Command \"Get-ItemPropertyValue -Path '{path}' -Name '{value}'\""
        return await self.execute_shell_command(command)

    async def set_registry_key(self, path: str, name: str, value: str) -> Optional[Dict[str, Any]]:
        """Set a registry key value."""
        command = f"powershell -Command \"Set-ItemProperty -Path '{path}' -Name '{name}' -Value '{value}'\""
        return await self.execute_shell_command(command)

    # --- File System Utilities ---

    async def get_file_hidden_status(self, file_path: str) -> int:
        """Get whether a file is hidden (1=hidden, 0=visible)."""
        command = f"import os; print(1 if os.path.basename(r'{file_path}').startswith('.') or (os.name == 'nt' and bool(os.stat(r'{file_path}').st_file_attributes & 2)) else 0)"
        result = await self.execute_python_command(command)
        if result and result.get("status") == "success":
            try:
                return int(result["output"].strip())
            except ValueError:
                return 0
        return 0

    async def get_vm_are_files_sorted_by_modified_time(self, directory: str) -> bool:
        """Check if files are sorted by modified time."""
        command = f"""import os
files = [(f, os.path.getmtime(os.path.join(r'{directory}', f))) for f in os.listdir(r'{directory}') if os.path.isfile(os.path.join(r'{directory}', f))]
sorted_files = sorted(files, key=lambda x: x[1], reverse=True)
print('true' if files == sorted_files else 'false')"""
        result = await self.execute_python_command(command.replace("\n", "; "))
        if result and result.get("status") == "success":
            return result["output"].strip().lower() == "true"
        return False

    async def get_vm_is_directory_read_only_for_user(self, directory: str, user: str) -> bool:
        """Check if directory is read-only for user."""
        command = f"powershell -Command \"(Get-Acl '{directory}').Access | Where-Object {{ $_.IdentityReference -like '*{user}*' }} | Select-Object -ExpandProperty FileSystemRights\""
        result = await self.execute_shell_command(command)
        if result and result.get("status") == "success":
            rights = result["output"].strip().lower()
            return "write" not in rights and "read" in rights
        return False

    # --- Wallpaper ---

    async def get_vm_wallpaper(self) -> Optional[bytes]:
        """Get the current wallpaper image."""
        command = 'powershell -Command "(Get-ItemProperty \'HKCU:\\Control Panel\\Desktop\' -Name Wallpaper).Wallpaper"'
        result = await self.execute_shell_command(command)
        if result and result.get("status") == "success":
            wallpaper_path = result["output"].strip()
            if wallpaper_path:
                return await self.get_file(wallpaper_path)
        return None

    # --- Window Operations ---

    async def get_vm_window_size(self, app_class_name: str) -> Optional[Dict[str, int]]:
        """Get window size for an application - not directly supported."""
        # Window-specific size queries would require UI automation
        # Return screen size as default
        return {"width": getattr(self.session, '_width', 1920),
                "height": getattr(self.session, '_height', 1080)}

    # --- Library Folders (Windows) ---

    async def get_vm_library_folders(self, library_name: str) -> Optional[List[str]]:
        """Get Windows library folders."""
        command = f"""powershell -Command "
$shell = New-Object -ComObject Shell.Application
$library = $shell.NameSpace('shell:::{{{library_name}}}')
if ($library) {{
    $library.Items() | ForEach-Object {{ $_.Path }}
}}
\""""
        result = await self.execute_shell_command(command)
        if result and result.get("status") == "success":
            output = result["output"].strip()
            if output:
                return output.split("\n")
        return None

    # --- Clock App (Windows) ---

    async def get_vm_check_if_timer_started(self, hours: int, minutes: int, seconds: int) -> str:
        """Check if a timer exists in Clock app."""
        # Simplified check - actual implementation would need UI automation
        command = 'powershell -Command "Get-Process -Name \'Time\' -ErrorAction SilentlyContinue | Select-Object -First 1"'
        result = await self.execute_shell_command(command)
        if result and result.get("status") == "success" and result["output"].strip():
            return "True"
        return "False"

    async def get_vm_check_if_world_clock_exists(self, city: str, country: str) -> str:
        """Check if a world clock exists in Clock app."""
        # Would require UI automation to check
        return "False"

    # --- Probe ---

    async def get_probe(self) -> bool:
        """Check if VM is running."""
        try:
            screenshot = await self.get_screenshot()
            return screenshot is not None
        except Exception:
            return False
