"""Setup controller for WAA tasks, adapted for cua-bench.

Executes config[] setup operations before a task starts.
"""

import asyncio
import logging
import os
import shutil
import sqlite3
import tempfile
import time
import uuid
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

import requests

logger = logging.getLogger("winarena.setup")

# Cache directory for downloaded files
CACHE_DIR = Path(tempfile.gettempdir()) / "waa_cache"


class WAASetupController:
    """Execute WAA config[] setup operations via cua-bench session."""

    def __init__(self, session, cache_dir: Optional[str] = None):
        """Initialize setup controller.

        Args:
            session: cua-bench DesktopSession (typically RemoteDesktopSession)
            cache_dir: Directory for caching downloaded files
        """
        self.session = session
        self.cache_dir = Path(cache_dir) if cache_dir else CACHE_DIR
        self.cache_dir.mkdir(parents=True, exist_ok=True)

    async def setup(self, config: List[Dict[str, Any]]):
        """Execute setup config operations.

        Args:
            config: List of setup operations, each with:
                - type: Operation type (maps to _{type}_setup method)
                - parameters: Dict of keyword parameters
        """
        for cfg in config:
            config_type: str = cfg["type"]
            parameters: Dict[str, Any] = cfg.get("parameters", {})

            # Map to setup method
            setup_method = f"_{config_type}_setup"
            if hasattr(self, setup_method):
                method = getattr(self, setup_method)
                # Call method (may be sync or async)
                result = method(**parameters)
                if asyncio.iscoroutine(result):
                    await result
                logger.info("SETUP: %s(%s)", setup_method, str(parameters)[:100])
            else:
                logger.warning("Unknown setup type: %s", config_type)

    # --- File Operations ---

    async def _download_setup(self, files: List[Dict[str, str]]):
        """Download files from URLs to VM.

        Args:
            files: List of {url, path} dicts
        """
        for f in files:
            url: str = f["url"]
            path: str = f["path"]

            if not url or not path:
                raise Exception(f"Setup Download - Invalid URL ({url}) or path ({path}).")

            # Generate cache path
            cache_path = self.cache_dir / f"{uuid.uuid5(uuid.NAMESPACE_URL, url)}_{os.path.basename(path)}"

            # Download if not cached
            if not cache_path.exists():
                max_retries = 3
                for i in range(max_retries):
                    try:
                        response = requests.get(url, stream=True, timeout=60)
                        response.raise_for_status()

                        with open(cache_path, "wb") as f_out:
                            for chunk in response.iter_content(chunk_size=8192):
                                if chunk:
                                    f_out.write(chunk)
                        logger.info("Downloaded: %s", url)
                        break
                    except requests.RequestException as e:
                        logger.error(f"Download failed ({max_retries - i - 1} retries left): {e}")
                        if i == max_retries - 1:
                            raise

            # Upload to VM
            try:
                content = cache_path.read_bytes()
                await self.session.write_bytes(path, content)
                logger.info("Uploaded to VM: %s", path)
            except Exception as e:
                logger.error("Failed to upload file: %s", e)
                raise

    async def _upload_file_setup(self, files: List[Dict[str, str]]):
        """Upload local files to VM.

        Args:
            files: List of {local_path, path} dicts
        """
        for f in files:
            local_path: str = f["local_path"]
            path: str = f["path"]

            if not os.path.exists(local_path):
                logger.error(f"Local path not found: {local_path}")
                continue

            try:
                content = Path(local_path).read_bytes()
                await self.session.write_bytes(path, content)
                logger.info("Uploaded to VM: %s", path)
            except Exception as e:
                logger.error("Failed to upload file: %s", e)

    # --- Application Launch ---

    async def _launch_setup(self, command: Union[str, List[str]], shell: bool = False):
        """Launch an application.

        Args:
            command: Application command (str or list)
            shell: Whether to run in shell
        """
        if not command:
            raise Exception("Empty command to launch.")

        try:
            if isinstance(command, list):
                app = command[0]
                args = command[1:] if len(command) > 1 else None
            else:
                parts = command.split()
                app = parts[0]
                args = parts[1:] if len(parts) > 1 else None

            # Use shell command to launch application
            cmd = app + (" " + " ".join(args) if args else "")
            await self.session.launch_application(cmd)
            logger.info("Launched: %s %s", app, args or "")
        except Exception as e:
            logger.error("Failed to launch application: %s", e)

    async def _open_setup(self, path: str):
        """Open a file with default application.

        Args:
            path: Path to file on VM
        """
        if not path:
            raise Exception(f"Setup Open - Invalid path ({path}).")

        try:
            # Use shell command to open file with default app
            await self.session.run_command(f'start "" "{path}"')
            logger.info("Opened: %s", path)
            await asyncio.sleep(2)  # Wait for app to open
        except Exception as e:
            logger.error("Failed to open file: %s", e)

    # --- Command Execution ---

    async def _execute_setup(
        self,
        command: Union[str, List[str]],
        stdout: str = "",
        stderr: str = "",
        shell: bool = False,
        until: Optional[Dict[str, Any]] = None,
    ):
        """Execute a shell command.

        Args:
            command: Command to execute
            stdout: File to save stdout
            stderr: File to save stderr
            shell: Whether to use shell
            until: Conditions to wait for
        """
        if not command:
            raise Exception("Empty command to execute.")

        until = until or {}
        terminates = False
        nb_failings = 0

        # Build command string
        cmd_str = " ".join(command) if isinstance(command, list) else command

        while not terminates:
            try:
                result = await self.session.run_command(cmd_str)

                # Handle different result formats
                if isinstance(result, dict):
                    results = {
                        "output": result.get("stdout", result.get("output", str(result))),
                        "error": result.get("stderr", result.get("error", "")),
                        "returncode": result.get("exit_code", result.get("returncode", 0)),
                    }
                else:
                    results = {
                        "output": result.stdout if hasattr(result, "stdout") else str(result),
                        "error": result.stderr if hasattr(result, "stderr") else "",
                        "returncode": result.exit_code if hasattr(result, "exit_code") else 0,
                    }

                if stdout:
                    (self.cache_dir / stdout).write_text(results["output"])
                if stderr:
                    (self.cache_dir / stderr).write_text(results["error"])

                logger.info("Executed: %s -> %s", cmd_str[:50], results["output"][:100])

            except Exception as e:
                logger.error("Command execution error: %s", e)
                results = None
                nb_failings += 1

            # Check termination conditions
            if len(until) == 0:
                terminates = True
            elif results is not None:
                terminates = (
                    ("returncode" in until and results["returncode"] == until["returncode"])
                    or ("stdout" in until and until["stdout"] in results["output"])
                    or ("stderr" in until and until["stderr"] in results["error"])
                )

            terminates = terminates or nb_failings >= 5
            if not terminates:
                await asyncio.sleep(0.3)

    async def _command_setup(self, command: List[str], **kwargs):
        """Alias for _execute_setup."""
        await self._execute_setup(command, **kwargs)

    # --- Timing ---

    async def _sleep_setup(self, seconds: float):
        """Wait for specified seconds."""
        await asyncio.sleep(seconds)

    # --- Window Management ---

    async def _activate_window_setup(self, window_name: str, strict: bool = False, by_class: bool = False):
        """Activate a window by name.

        Args:
            window_name: Window title or class name
            strict: Exact match required
            by_class: Match by class instead of title
        """
        if not window_name:
            raise Exception(f"Invalid window name: {window_name}")

        try:
            # Use PowerShell to activate window (Windows-specific)
            cmd = f'powershell -Command "(Get-Process | Where-Object {{$_.MainWindowTitle -like \'*{window_name}*\'}}).MainWindowHandle | ForEach-Object {{ $null = [Microsoft.VisualBasic.Interaction]::AppActivate($_) }}"'
            await self.session.run_command(cmd)
            logger.info("Activated window: %s", window_name)
        except Exception as e:
            logger.error("Failed to activate window %s: %s", window_name, e)

    async def _close_window_setup(self, window_name: str, strict: bool = False, by_class: bool = False):
        """Close a window by name."""
        if not window_name:
            raise Exception(f"Invalid window name: {window_name}")

        try:
            # Use PowerShell to close window by title
            cmd = f'powershell -Command "Get-Process | Where-Object {{$_.MainWindowTitle -like \'*{window_name}*\'}} | Stop-Process -Force"'
            await self.session.run_command(cmd)
            logger.info("Closed window: %s", window_name)
        except Exception as e:
            logger.error("Failed to close window %s: %s", window_name, e)

    async def _close_all_setup(self):
        """Close all user windows."""
        try:
            cmd = 'powershell -Command "Get-Process | Where-Object {$_.MainWindowTitle -ne \'\'} | Stop-Process -Force"'
            await self.session.run_command(cmd)
            logger.info("Closed all windows")
        except Exception as e:
            logger.error("Failed to close all windows: %s", e)

    # --- File System ---

    async def _create_folder_setup(self, path: str):
        """Create a folder on the VM."""
        if not path:
            raise Exception(f"Invalid path: {path}")

        try:
            await self.session.run_command(f'mkdir "{path}"')
            logger.info("Created folder: %s", path)
        except Exception as e:
            logger.error("Failed to create folder: %s", e)

    async def _create_file_setup(self, path: str, content: str = ""):
        """Create a file on the VM."""
        if not path:
            raise Exception(f"Invalid path: {path}")

        try:
            await self.session.write_file(path, content)
            logger.info("Created file: %s", path)
        except Exception as e:
            logger.error("Failed to create file: %s", e)

    async def _recycle_file_setup(self, path: str):
        """Move a file to recycle bin."""
        if not path:
            raise Exception(f"Invalid path: {path}")

        try:
            cmd = f"powershell -Command \"$shell = New-Object -ComObject Shell.Application; $folder = $shell.NameSpace(0); $item = $folder.ParseName('{path}'); if ($item) {{ $item.InvokeVerb('delete') }}\""
            await self.session.run_command(cmd)
            logger.info("Recycled file: %s", path)
        except Exception as e:
            logger.error("Failed to recycle file: %s", e)

    async def _change_wallpaper_setup(self, path: str):
        """Change desktop wallpaper."""
        if not path:
            raise Exception(f"Invalid path: {path}")

        try:
            # Use PowerShell to set wallpaper
            cmd = f'''powershell -Command "Add-Type @\\"
using System.Runtime.InteropServices;
public class Wallpaper {{
    [DllImport(\\"user32.dll\\", CharSet = CharSet.Auto)]
    public static extern int SystemParametersInfo(int uAction, int uParam, string lpvParam, int fuWinIni);
}}
\\"@; [Wallpaper]::SystemParametersInfo(0x0014, 0, '{path}', 0x0001)"'''
            await self.session.run_command(cmd)
            logger.info("Changed wallpaper to: %s", path)
        except Exception as e:
            logger.error("Failed to change wallpaper: %s", e)

    # --- Browser Operations ---

    async def _chrome_open_tabs_setup(self, urls_to_open: List[str]):
        """Open Chrome tabs via Playwright.

        Note: This requires Chrome to be launched with --remote-debugging-port
        """
        # Open tabs via shell command
        try:
            for url in urls_to_open:
                await self.session.run_command(f'start chrome "{url}"')
                await asyncio.sleep(1)
            logger.info("Opened Chrome tabs: %s", urls_to_open)
        except Exception as e:
            logger.error("Failed to open Chrome tabs: %s", e)

    async def _edge_open_tabs_setup(self, urls_to_open: List[str]):
        """Open Edge tabs."""
        try:
            for url in urls_to_open:
                await self.session.run_command(f'start msedge "{url}"')
                await asyncio.sleep(1)
            logger.info("Opened Edge tabs: %s", urls_to_open)
        except Exception as e:
            logger.error("Failed to open Edge tabs: %s", e)

    async def _chrome_close_tabs_setup(self, urls_to_close: List[str]):
        """Close Chrome tabs (not fully implemented)."""
        logger.warning("chrome_close_tabs_setup not fully implemented")

    # --- Browser History ---

    async def _update_browse_history_setup(self, history: List[Dict[str, Any]]):
        """Update Chrome browsing history."""
        # Create history database
        db_template = Path(__file__).parent / "assets" / "history_empty.sqlite"
        if not db_template.exists():
            logger.warning("History template not found, skipping")
            return

        cache_path = self.cache_dir / "history_new.sqlite"
        shutil.copyfile(db_template, cache_path)

        for history_item in history:
            url = history_item["url"]
            title = history_item["title"]
            visit_time = datetime.now() - timedelta(seconds=history_item["visit_time_from_now_in_seconds"])

            # Chrome uses microseconds from 1601-01-01
            epoch_start = datetime(1601, 1, 1)
            chrome_timestamp = int((visit_time - epoch_start).total_seconds() * 1000000)

            conn = sqlite3.connect(cache_path)
            cursor = conn.cursor()

            cursor.execute(
                "INSERT INTO urls (url, title, visit_count, typed_count, last_visit_time, hidden) VALUES (?, ?, ?, ?, ?, ?)",
                (url, title, 1, 0, chrome_timestamp, 0),
            )
            url_id = cursor.lastrowid

            cursor.execute(
                "INSERT INTO visits (url, visit_time, from_visit, transition, segment_id, visit_duration) VALUES (?, ?, ?, ?, ?, ?)",
                (url_id, chrome_timestamp, 0, 805306368, 0, 0),
            )

            conn.commit()
            conn.close()

        # Upload to VM
        try:
            result = await self.session.run_command(
                'python -c "import os; print(os.path.join(os.getenv(\'USERPROFILE\'), \'AppData\', \'Local\', \'Google\', \'Chrome\', \'User Data\', \'Default\', \'History\'))"'
            )
            # Handle different result formats
            if isinstance(result, dict):
                chrome_history_path = result.get("stdout", result.get("output", str(result))).strip()
            else:
                chrome_history_path = result.stdout.strip() if hasattr(result, "stdout") else str(result).strip()

            content = cache_path.read_bytes()
            await self.session.write_bytes(chrome_history_path, content)
            logger.info("Updated Chrome history")
        except Exception as e:
            logger.error("Failed to update Chrome history: %s", e)

    async def _update_browse_history_edge_setup(self, history: List[Dict[str, Any]]):
        """Update Edge browsing history."""
        # Similar to Chrome but with Edge paths
        logger.warning("update_browse_history_edge_setup not fully implemented")

    # --- Placeholder for complex operations ---

    async def _set_default_browser_setup(self, browser: str):
        """Set default browser (limited support)."""
        logger.warning("set_default_browser_setup has limited support")

    async def _googledrive_setup(self, **config):
        """Google Drive operations (requires pydrive auth)."""
        logger.warning("googledrive_setup requires manual pydrive configuration")

    async def _login_setup(self, **config):
        """Login to website (requires Playwright)."""
        logger.warning("login_setup requires Playwright integration")

    async def _act_setup(self, action_seq: List[Union[Dict[str, Any], str]]):
        """Execute action sequence (not implemented)."""
        raise NotImplementedError("act_setup not implemented")

    async def _replay_setup(self, trajectory: str):
        """Replay trajectory (not implemented)."""
        raise NotImplementedError("replay_setup not implemented")

    async def _tidy_desktop_setup(self, **config):
        """Tidy desktop (not implemented)."""
        raise NotImplementedError("tidy_desktop_setup not implemented")
