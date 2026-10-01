"""File-based shared memory for cross-agent learnings.

Agents read memory at start, append learnings at end.
Organized by OS and a general file for cross-cutting patterns.
"""

from __future__ import annotations

import logging
import time
from pathlib import Path

logger = logging.getLogger(__name__)

MEMORY_DIR_NAME = "memory"

INITIAL_GENERAL = """\
# Shared Memory — General Patterns

Cross-cutting learnings discovered by creation agents.

## Package Manager Tips
- Always run `apt-get update` before `apt-get install` on Linux
- Use `-y` flag for non-interactive installs
- On Ubuntu 24.04 (Noble), some packages renamed (e.g. libasound2 → libasound2t64)

## Common Pitfalls
- First-run wizards block headless testing — dismiss via config files or CLI flags
- Some apps require DISPLAY to be set even for --version checks
- Snap packages may need `--classic` flag
"""

INITIAL_LINUX = """\
# Shared Memory — Linux

## APT
- Always `apt-get update` first
- Use `DEBIAN_FRONTEND=noninteractive` to suppress dialogs
- For PPAs: `add-apt-repository -y ppa:...` then `apt-get update`

## Snap
- `snap install <pkg>` or `snap install <pkg> --classic`
- Snap may not be available in all containers

## Flatpak
- `flatpak install -y flathub <app-id>`
- Requires `flatpak` package installed first

## AppImage
- Download, chmod +x, run directly
- May need `libfuse2` on Ubuntu 22.04+
"""

INITIAL_WINDOWS = """\
# Shared Memory — Windows

## Winget
- `winget install --id <PackageId> --accept-package-agreements --accept-source-agreements -h`
- The `-h` flag = silent install

## Chocolatey
- `choco install <pkg> -y --no-progress`

## Direct Download
- Use `Invoke-WebRequest -Uri <url> -OutFile <path>` in PowerShell
- For MSI: `msiexec /i <path> /quiet /norestart`
- For EXE: `Start-Process -Wait -FilePath <path> -ArgumentList '/S'` (NSIS) or `/VERYSILENT` (Inno)
"""

INITIAL_MACOS = """\
# Shared Memory — macOS

## Homebrew
- `brew install <formula>` for CLI tools
- `brew install --cask <cask>` for GUI apps
- Run `brew update` periodically

## DMG
- `hdiutil attach <dmg>`, copy .app to /Applications, `hdiutil detach`

## PKG
- `sudo installer -pkg <path> -target /`
"""

INITIAL_ANDROID = """\
# Shared Memory — Android

## APK Install
- `adb install <path.apk>`
- For updates: `adb install -r <path.apk>`

## F-Droid
- Many open-source apps available via F-Droid repos
- APK URLs follow pattern: https://f-droid.org/repo/<package>_<version>.apk
"""


def init_memory(base_dir: Path) -> Path:
    """Initialize the shared memory directory with seed files."""
    mem_dir = base_dir / MEMORY_DIR_NAME
    mem_dir.mkdir(parents=True, exist_ok=True)

    seeds = {
        "general.md": INITIAL_GENERAL,
        "linux.md": INITIAL_LINUX,
        "windows.md": INITIAL_WINDOWS,
        "macos.md": INITIAL_MACOS,
        "android.md": INITIAL_ANDROID,
    }

    for name, content in seeds.items():
        path = mem_dir / name
        if not path.exists():
            path.write_text(content, encoding="utf-8")
            logger.info("Initialized memory: %s", path)

    return mem_dir


def read_memory(base_dir: Path, target_os: str) -> str:
    """Read relevant memory for an agent working on target_os."""
    mem_dir = base_dir / MEMORY_DIR_NAME
    parts = []

    general = mem_dir / "general.md"
    if general.exists():
        parts.append(general.read_text(encoding="utf-8"))

    os_file = mem_dir / f"{target_os}.md"
    if os_file.exists():
        parts.append(os_file.read_text(encoding="utf-8"))

    return "\n\n---\n\n".join(parts) if parts else "(no shared memory yet)"


def append_learning(base_dir: Path, target_os: str, app_name: str, learning: str) -> None:
    """Append a learning from an agent to the shared memory."""
    mem_dir = base_dir / MEMORY_DIR_NAME
    mem_dir.mkdir(parents=True, exist_ok=True)

    os_file = mem_dir / f"{target_os}.md"
    timestamp = time.strftime("%Y-%m-%d %H:%M")

    entry = f"\n\n## {app_name} ({timestamp})\n{learning}\n"

    with open(os_file, "a", encoding="utf-8") as f:
        f.write(entry)
