from __future__ import annotations

import json
import subprocess
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[4]
DRIVER_ROOT = ROOT / "libs/cua-driver"
PLUGIN_ROOT = DRIVER_ROOT / "plugins/cua-driver"
SKILL_SOURCE = DRIVER_ROOT / "rust/Skills/cua-driver"


def read_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def test_generated_agent_plugin_is_current() -> None:
    subprocess.run(
        [
            "python3",
            "libs/cua-driver/scripts/generate-agent-plugin.py",
            "--check",
        ],
        cwd=ROOT,
        check=True,
    )


def test_vendor_manifests_share_identity_and_release_version() -> None:
    version = (DRIVER_ROOT / "rust/VERSION").read_text(encoding="utf-8").strip()
    manifests = [
        read_json(PLUGIN_ROOT / ".claude-plugin/plugin.json"),
        read_json(PLUGIN_ROOT / ".grok-plugin/plugin.json"),
        read_json(PLUGIN_ROOT / ".codex-plugin/plugin.json"),
    ]

    for manifest in manifests:
        assert manifest["name"] == "cua-driver"
        assert manifest["version"] == version
        assert manifest["repository"] == "https://github.com/trycua/cua"
        assert manifest["license"] == "MIT"

    codex = manifests[-1]
    assert codex["skills"] == "./skills/"
    assert codex["mcpServers"] == "./.mcp.json"


def test_plugin_uses_local_cua_driver_stdio_mcp() -> None:
    assert read_json(PLUGIN_ROOT / ".mcp.json") == {
        "mcpServers": {
            "cua-driver": {
                "command": "cua-driver",
                "args": ["mcp"],
            }
        }
    }


def test_plugin_skill_is_generated_from_canonical_skill() -> None:
    packaged_skill = PLUGIN_ROOT / "skills/cua-driver"
    expected_files = {
        "SKILL.md",
        "MACOS.md",
        "WINDOWS.md",
        "LINUX.md",
        "BROWSER.md",
        "RECORDING.md",
        "EMBEDDING.md",
    }
    assert {path.name for path in packaged_skill.iterdir()} == expected_files

    canonical_skill = (SKILL_SOURCE / "SKILL.md").read_text(encoding="utf-8")
    packaged_contents = (packaged_skill / "SKILL.md").read_text(encoding="utf-8")
    canonical_without_version = "\n".join(
        line
        for line in canonical_skill.splitlines()
        if not line.startswith("version: ")
    ) + "\n"
    assert packaged_contents == canonical_without_version

    frontmatter = yaml.safe_load(packaged_contents.split("---", 2)[1])
    assert set(frontmatter) <= {
        "name",
        "description",
        "license",
        "allowed-tools",
        "metadata",
    }
    assert frontmatter["name"] == "cua-driver"

    for filename in expected_files - {"SKILL.md", "WINDOWS.md"}:
        assert (packaged_skill / filename).read_bytes() == (
            SKILL_SOURCE / filename
        ).read_bytes()

    packaged_windows = (packaged_skill / "WINDOWS.md").read_text(encoding="utf-8")
    assert "https://cua.ai/docs/how-to-guides/driver/install" in packaged_windows
    assert "irm https://cua.ai/driver/install.ps1 | iex" not in packaged_windows


def test_marketplace_readme_requires_separate_native_install() -> None:
    readme = (PLUGIN_ROOT / "README.md").read_text(encoding="utf-8")
    assert "does not install or update" in readme
    assert "curl" not in readme
    assert "cua-driver doctor" in readme

    package_text = "\n".join(
        path.read_text(encoding="utf-8")
        for path in PLUGIN_ROOT.rglob("*")
        if path.is_file()
    )
    assert "curl" not in package_text
    assert "irm https://cua.ai/driver/install.ps1 | iex" not in package_text
