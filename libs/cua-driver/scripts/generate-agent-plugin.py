#!/usr/bin/env python3
"""Generate the cross-vendor Cua Driver plugin package."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
DRIVER_ROOT = ROOT / "libs/cua-driver"
SKILL_SOURCE = DRIVER_ROOT / "rust/Skills/cua-driver"
PLUGIN_ROOT = DRIVER_ROOT / "plugins/cua-driver"
VERSION_FILE = DRIVER_ROOT / "rust/VERSION"

SKILL_FILES = (
    "SKILL.md",
    "MACOS.md",
    "WINDOWS.md",
    "LINUX.md",
    "BROWSER.md",
    "RECORDING.md",
    "EMBEDDING.md",
)

DESCRIPTION = (
    "Drive native macOS, Windows, and Linux applications through Cua Driver "
    "with snapshot-bound, verified UI actions."
)
SHORT_DESCRIPTION = "Drive native apps with verified Cua Driver actions."
LONG_DESCRIPTION = (
    "Connects an agent to the locally installed Cua Driver MCP server and "
    "teaches the snapshot, action, and verification workflow for safe desktop "
    "automation across macOS, Windows, and Linux."
)

WINDOWS_INSTALL_BLOCK = """   If missing, point the user at:
   ```powershell
   irm https://cua.ai/driver/install.ps1 | iex
   ```
   and stop.
"""
WINDOWS_PORTABLE_INSTALL_BLOCK = """   If missing, direct the user to the Cua Driver installation guide at
   `https://cua.ai/docs/how-to-guides/driver/install` and stop. The plugin must
   not download or execute the native installer.
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail when the checked-in plugin package differs from generated output.",
    )
    return parser.parse_args()


def read_version() -> str:
    version = VERSION_FILE.read_text(encoding="utf-8").strip()
    if re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", version) is None:
        raise SystemExit(f"invalid Cua Driver version in {VERSION_FILE}: {version!r}")
    return version


def write_json(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def common_manifest(version: str) -> dict[str, object]:
    return {
        "name": "cua-driver",
        "version": version,
        "description": DESCRIPTION,
        "author": {
            "name": "Cua AI, Inc.",
            "url": "https://cua.ai",
        },
        "homepage": "https://cua.ai/docs/cua-driver",
        "repository": "https://github.com/trycua/cua",
        "license": "MIT",
        "keywords": ["cua", "cua driver", "cua-driver", "computer use"],
    }


def portable_skill_contents(filename: str) -> str:
    source = SKILL_SOURCE / filename
    contents = source.read_text(encoding="utf-8")
    if filename == "SKILL.md":
        contents, replacements = re.subn(
            r"(?m)^version: .* # x-release-please-version\n",
            "",
            contents,
        )
        if replacements != 1:
            raise SystemExit(f"expected one release version in {source}")
    if filename == "WINDOWS.md":
        if contents.count(WINDOWS_INSTALL_BLOCK) != 1:
            raise SystemExit(f"expected one installer block in {source}")
        contents = contents.replace(
            WINDOWS_INSTALL_BLOCK,
            WINDOWS_PORTABLE_INSTALL_BLOCK,
        )
    return contents


def render_package(destination: Path) -> None:
    version = read_version()
    skill_destination = destination / "skills/cua-driver"
    skill_destination.mkdir(parents=True, exist_ok=True)
    for filename in SKILL_FILES:
        target = skill_destination / filename
        target.write_text(portable_skill_contents(filename), encoding="utf-8")

    mcp_manifest = {
        "mcpServers": {
            "cua-driver": {
                "command": "cua-driver",
                "args": ["mcp"],
            }
        }
    }
    write_json(destination / ".mcp.json", mcp_manifest)

    portable_manifest = common_manifest(version)
    write_json(destination / ".claude-plugin/plugin.json", portable_manifest)
    write_json(destination / ".grok-plugin/plugin.json", portable_manifest)

    codex_manifest = {
        **portable_manifest,
        "skills": "./skills/",
        "mcpServers": "./.mcp.json",
        "interface": {
            "displayName": "Cua Driver",
            "shortDescription": SHORT_DESCRIPTION,
            "longDescription": LONG_DESCRIPTION,
            "developerName": "Cua AI, Inc.",
            "category": "Developer Tools",
            "capabilities": ["Interactive", "Write"],
            "websiteURL": "https://cua.ai",
            "privacyPolicyURL": "https://cua.ai/privacy-policy",
            "termsOfServiceURL": "https://cua.ai/terms-of-service",
            "brandColor": "#20C997",
            "defaultPrompt": [
                "List the visible apps and windows without interacting with them.",
                "Inspect the selected app and explain what is visible.",
                "Complete this desktop task and verify each action from fresh state.",
            ],
        },
    }
    write_json(destination / ".codex-plugin/plugin.json", codex_manifest)


def files_under(root: Path) -> dict[str, bytes]:
    if not root.exists():
        return {}
    return {
        path.relative_to(root).as_posix(): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file() and path.name != "README.md"
    }


def check_package() -> int:
    with tempfile.TemporaryDirectory(prefix="cua-driver-plugin-") as temp_dir:
        generated_root = Path(temp_dir) / "cua-driver"
        render_package(generated_root)
        expected = files_under(generated_root)
        actual = files_under(PLUGIN_ROOT)
    if actual == expected:
        print("Cua Driver agent plugin package is up to date.")
        return 0

    missing = sorted(expected.keys() - actual.keys())
    extra = sorted(actual.keys() - expected.keys())
    changed = sorted(
        path for path in expected.keys() & actual.keys() if expected[path] != actual[path]
    )
    for label, paths in (("missing", missing), ("extra", extra), ("changed", changed)):
        for path in paths:
            print(f"{label}: {path}", file=sys.stderr)
    print(
        "Run `python3 libs/cua-driver/scripts/generate-agent-plugin.py`.",
        file=sys.stderr,
    )
    return 1


def main() -> None:
    args = parse_args()
    if args.check:
        raise SystemExit(check_package())
    render_package(PLUGIN_ROOT)
    print(f"Generated Cua Driver agent plugin: {PLUGIN_ROOT}")


if __name__ == "__main__":
    main()
