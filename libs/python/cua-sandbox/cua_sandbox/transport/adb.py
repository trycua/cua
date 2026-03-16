"""ADBTransport — communicates with Android emulator via ADB.

Screenshots via `adb exec-out screencap -p`, shell via `adb shell`,
screen size via `adb shell wm size`.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Any, Dict, Optional

from cua_sandbox.transport.base import Transport


class ADBTransport(Transport):
    """Transport backed by ADB (Android Debug Bridge)."""

    def __init__(self, serial: str = "emulator-5554", sdk_root: Optional[str] = None):
        self._serial = serial
        self._sdk_root = sdk_root
        self._adb: Optional[str] = None

    def _find_adb(self) -> str:
        if self._sdk_root:
            candidate = Path(self._sdk_root) / "platform-tools" / "adb"
            if candidate.exists():
                return str(candidate)
        found = shutil.which("adb")
        if found:
            return found
        raise FileNotFoundError("adb not found. Install the Android SDK or set ANDROID_HOME.")

    async def connect(self) -> None:
        self._adb = self._find_adb()
        env = self._env()
        subprocess.run([self._adb, "start-server"], capture_output=True, env=env)

    async def disconnect(self) -> None:
        pass

    def _env(self) -> dict:
        env = os.environ.copy()
        if self._sdk_root:
            env["ANDROID_SDK_ROOT"] = self._sdk_root
        return env

    def _adb_cmd(self, *args: str, timeout: float = 15) -> subprocess.CompletedProcess:
        assert self._adb is not None, "Transport not connected"
        return subprocess.run(
            [self._adb, "-s", self._serial, *args],
            capture_output=True,
            env=self._env(),
            timeout=timeout,
        )

    async def send(self, action: str, **params: Any) -> Any:
        if action in ("shell", "execute", "run_command"):
            cmd = params.get("command", "")
            result = self._adb_cmd("shell", cmd)
            return {
                "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace"),
                "returncode": result.returncode,
            }
        raise ValueError(f"Unknown action: {action}")

    async def screenshot(self) -> bytes:
        assert self._adb is not None, "Transport not connected"
        # exec-out screencap -p returns PNG directly to stdout
        result = self._adb_cmd("exec-out", "screencap", "-p", timeout=15)
        if result.returncode == 0 and len(result.stdout) > 100 and result.stdout[:4] == b"\x89PNG":
            return result.stdout
        # Fallback: screencap to file then pull
        self._adb_cmd("shell", "screencap", "-p", "/sdcard/screenshot.png")
        result = self._adb_cmd("exec-out", "cat", "/sdcard/screenshot.png", timeout=15)
        if result.returncode == 0 and result.stdout[:4] == b"\x89PNG":
            return result.stdout
        raise RuntimeError(f"ADB screenshot failed: {result.stderr.decode(errors='replace')}")

    async def get_screen_size(self) -> Dict[str, int]:
        result = self._adb_cmd("shell", "wm", "size")
        # Output: "Physical size: 1080x1920"
        output = result.stdout.decode().strip()
        for line in output.splitlines():
            if "size" in line.lower():
                parts = line.split(":")[-1].strip()
                w, h = parts.split("x")
                return {"width": int(w), "height": int(h)}
        raise RuntimeError(f"Could not parse screen size: {output}")

    async def get_environment(self) -> str:
        return "android"
