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

    async def _adb_cmd_async(self, *args: str, timeout: float = 15) -> subprocess.CompletedProcess:
        """Run _adb_cmd in a thread executor so the event loop stays responsive."""
        import asyncio
        import functools

        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(
            None, functools.partial(self._adb_cmd, *args, timeout=timeout)
        )

    async def send(self, action: str, **params: Any) -> Any:
        if action in ("shell", "execute", "run_command"):
            cmd = params.get("command", "")
            result = await self._adb_cmd_async("shell", cmd)
            return {
                "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace"),
                "returncode": result.returncode,
            }
        if action == "multitouch_gesture":
            return self._multitouch_gesture(**params)
        raise ValueError(f"Unknown action: {action}")

    def _multitouch_gesture(
        self,
        fingers: list,
        screen_w: int,
        screen_h: int,
        duration_ms: int = 400,
        steps: int = 0,
    ) -> Any:
        """Inject multi-touch via MT Protocol B sendevent (local ADB).

        Calls ``adb root`` to restart adbd as root so plain ``sendevent``
        works without ``su`` inside the Android shell.
        """
        import re
        import time

        # Restart adbd as root
        self._adb_cmd("root", timeout=10)
        time.sleep(1.0)  # wait for adbd to restart

        # Find touch device
        dev_result = self._adb_cmd(
            "shell",
            "for f in /dev/input/event*; do "
            "  getevent -p \"$f\" 2>/dev/null | grep -q '0035' "
            '    && echo "$f" && break; '
            "done",
        )
        dev = dev_result.stdout.decode(errors="replace").strip() or "/dev/input/event1"

        # Detect axis max
        axis_max = 32767
        gp_result = self._adb_cmd("shell", f"getevent -p {dev} 2>/dev/null | grep '0035'")
        gp_out = gp_result.stdout.decode(errors="replace")
        m = re.search(r"max\s+(\d+)", gp_out)
        if m:
            axis_max = int(m.group(1))

        n_steps = steps if steps > 0 else max(5, duration_ms // 20)
        step_delay = (duration_ms / 1000) / n_steps

        EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
        SYN_REPORT, BTN_TOUCH = 0, 330
        SLOT, TID, MTX, MTY, PRESS = 47, 57, 53, 54, 58
        TID_NONE = 4294967295

        def px_to_raw(px: int, dim: int) -> int:
            return max(0, min(axis_max, int(px * axis_max / dim)))

        def se(t: int, c: int, v: int) -> str:
            return f"sendevent {dev} {t} {c} {v}"

        cmds: list[str] = []
        for idx, finger in enumerate(fingers):
            x1, y1 = finger["start"]
            cmds += [
                se(EV_ABS, SLOT, idx),
                se(EV_ABS, TID, idx),
                se(EV_ABS, MTX, px_to_raw(x1, screen_w)),
                se(EV_ABS, MTY, px_to_raw(y1, screen_h)),
                se(EV_ABS, PRESS, 64),
            ]
        cmds += [se(EV_KEY, BTN_TOUCH, 1), se(EV_SYN, SYN_REPORT, 0)]

        for i in range(1, n_steps + 1):
            t = i / n_steps
            for idx, finger in enumerate(fingers):
                x1, y1 = finger["start"]
                x2, y2 = finger["end"]
                cmds += [
                    se(EV_ABS, SLOT, idx),
                    se(EV_ABS, MTX, px_to_raw(int(x1 + (x2 - x1) * t), screen_w)),
                    se(EV_ABS, MTY, px_to_raw(int(y1 + (y2 - y1) * t), screen_h)),
                ]
            cmds += [se(EV_SYN, SYN_REPORT, 0), f"sleep {step_delay:.3f}"]

        for idx in range(len(fingers)):
            cmds += [se(EV_ABS, SLOT, idx), se(EV_ABS, TID, TID_NONE)]
        cmds += [se(EV_KEY, BTN_TOUCH, 0), se(EV_SYN, SYN_REPORT, 0)]

        result = self._adb_cmd("shell", " && ".join(cmds), timeout=60)
        return {
            "stdout": result.stdout.decode(errors="replace"),
            "returncode": result.returncode,
        }

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._adb is not None, "Transport not connected"
        # exec-out screencap -p returns PNG directly to stdout
        result = await self._adb_cmd_async("exec-out", "screencap", "-p", timeout=15)
        if result.returncode == 0 and len(result.stdout) > 100 and result.stdout[:4] == b"\x89PNG":
            png_data = result.stdout
        else:
            # Fallback: screencap to file then pull
            await self._adb_cmd_async("shell", "screencap", "-p", "/sdcard/screenshot.png")
            result = await self._adb_cmd_async(
                "exec-out", "cat", "/sdcard/screenshot.png", timeout=15
            )
            if result.returncode == 0 and result.stdout[:4] == b"\x89PNG":
                png_data = result.stdout
            else:
                raise RuntimeError(
                    f"ADB screenshot failed: {result.stderr.decode(errors='replace')}"
                )

        if format.lower() in ("jpeg", "jpg"):
            from io import BytesIO

            from PIL import Image as PILImage

            img = PILImage.open(BytesIO(png_data)).convert("RGB")
            buf = BytesIO()
            img.save(buf, format="JPEG", quality=quality, optimize=True)
            return buf.getvalue()

        return png_data

    async def get_screen_size(self) -> Dict[str, int]:
        result = await self._adb_cmd_async("shell", "wm", "size")
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
