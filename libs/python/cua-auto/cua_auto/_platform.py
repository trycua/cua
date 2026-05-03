"""Platform detection helpers."""

import platform as _platform

_sys = _platform.system().lower()

IS_WINDOWS: bool = _sys == "windows"
IS_MACOS: bool = _sys == "darwin"
IS_LINUX: bool = _sys == "linux"

PLATFORM: str = "windows" if IS_WINDOWS else ("macos" if IS_MACOS else "linux")
