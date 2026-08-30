from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[4]
DRIVER_ROOT = ROOT / "libs/cua-driver"
PLUGIN_ROOT = DRIVER_ROOT / "plugins/cua-driver"
SKILL_SOURCE = DRIVER_ROOT / "rust/Skills/cua-driver"
GENERATOR_PATH = DRIVER_ROOT / "scripts/generate-agent-plugin.py"


def read_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def test_generated_agent_plugin_is_current() -> None:
    subprocess.run(
        [
            sys.executable,
            "libs/cua-driver/scripts/generate-agent-plugin.py",
            "--check",
        ],
        cwd=ROOT,
        check=True,
    )


def test_render_package_removes_stale_generated_files(tmp_path: Path) -> None:
    spec = importlib.util.spec_from_file_location("generate_agent_plugin", GENERATOR_PATH)
    assert spec is not None and spec.loader is not None
    generator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(generator)

    destination = tmp_path / "cua-driver"
    destination.mkdir()
    readme = destination / "README.md"
    readme.write_text("hand-written\n", encoding="utf-8")
    stale = destination / "skills/cua-driver/obsolete.md"
    stale.parent.mkdir(parents=True)
    stale.write_text("obsolete\n", encoding="utf-8")
    stale_readme = stale.parent / "README.md"
    stale_readme.write_text("obsolete readme\n", encoding="utf-8")

    assert "skills/cua-driver/README.md" in generator.files_under(destination)

    generator.render_package(destination)

    assert not stale.exists()
    assert not stale_readme.exists()
    assert readme.read_text(encoding="utf-8") == "hand-written\n"


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


def test_plugin_skill_uses_bundled_mcp_without_fallback() -> None:
    packaged_skill = PLUGIN_ROOT / "skills/cua-driver"
    packaged_contents = (packaged_skill / "SKILL.md").read_text(encoding="utf-8")
    frontmatter = yaml.safe_load(packaged_contents.split("---", 2)[1])
    openai = yaml.safe_load((packaged_skill / "agents/openai.yaml").read_text(encoding="utf-8"))
    package_skill_text = "\n".join(
        path.read_text(encoding="utf-8") for path in sorted(packaged_skill.rglob("*.md"))
    )
    normalized_skill_text = " ".join(package_skill_text.split())

    assert "explicitly invoked as `$cua-driver`" in frontmatter["description"]
    assert "plain-language computer-use requests" in frontmatter["description"]
    assert openai["interface"]["display_name"] == "Cua Driver"
    assert openai["policy"]["allow_implicit_invocation"] is False
    assert "Default transport is the bundled `cua-driver` MCP server" in packaged_contents
    assert "Do not shell out to `cua-driver`" in packaged_contents
    assert "Do not silently fall back" in packaged_contents
    assert "Default transport is the `cua-driver` CLI" not in package_skill_text
    assert "MCP tools (prefix `mcp__cua-driver__*`) only when" not in package_skill_text
    assert "Whenever a user asks to drive a native app" not in normalized_skill_text
    assert "Whenever a user asks to drive a native Windows app" not in normalized_skill_text
    assert (
        "Use Cua Driver when the outcome lives in an application's UI" not in normalized_skill_text
    )
    assert "After this Skill has been explicitly invoked" in normalized_skill_text
    assert "Extends an explicitly invoked Cua Driver plugin task" in normalized_skill_text
    assert package_skill_text.count("## Standalone CLI reference — not for plugin execution") == 2
    transport_notice = "Treat any `cua-driver ...` shell command as standalone documentation only"
    for filename in {
        "BROWSER.md",
        "EMBEDDING.md",
        "LINUX.md",
        "MACOS.md",
        "RECORDING.md",
        "SKILL.md",
        "WINDOWS.md",
    }:
        assert transport_notice in (packaged_skill / filename).read_text(encoding="utf-8")


def test_plugin_skill_bundles_action_result_contract() -> None:
    packaged_skill = PLUGIN_ROOT / "skills/cua-driver"
    packaged_contents = (packaged_skill / "SKILL.md").read_text(encoding="utf-8")
    contract = packaged_skill / "action-result-contract.md"

    assert "[`action-result-contract.md`](action-result-contract.md)" in packaged_contents
    assert "../../../docs/action-result-contract.md" not in packaged_contents
    assert contract.read_bytes() == (DRIVER_ROOT / "docs/action-result-contract.md").read_bytes()


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
        "action-result-contract.md",
        "agents/openai.yaml",
    }
    assert {
        path.relative_to(packaged_skill).as_posix()
        for path in packaged_skill.rglob("*")
        if path.is_file()
    } == expected_files

    packaged_contents = (packaged_skill / "SKILL.md").read_text(encoding="utf-8")
    frontmatter = yaml.safe_load(packaged_contents.split("---", 2)[1])
    assert set(frontmatter) <= {
        "name",
        "description",
        "license",
        "allowed-tools",
        "metadata",
    }
    assert frontmatter["name"] == "cua-driver"

    for filename in expected_files - {
        "SKILL.md",
        "WINDOWS.md",
        "action-result-contract.md",
        "agents/openai.yaml",
    }:
        assert (
            (packaged_skill / filename)
            .read_bytes()
            .endswith((SKILL_SOURCE / filename).read_bytes())
        )

    packaged_windows = (packaged_skill / "WINDOWS.md").read_text(encoding="utf-8")
    assert "https://cua.ai/docs/how-to-guides/driver/install" in packaged_windows
    assert "irm https://cua.ai/driver/install.ps1 | iex" not in packaged_windows


def test_marketplace_readme_requires_separate_native_install() -> None:
    readme = (PLUGIN_ROOT / "README.md").read_text(encoding="utf-8")
    assert "does not install or update" in readme
    assert "does not fall back" in readme
    assert "Use $cua-driver" in readme
    assert "curl" not in readme
    assert "cua-driver doctor" in readme

    package_text = "\n".join(
        path.read_text(encoding="utf-8") for path in PLUGIN_ROOT.rglob("*") if path.is_file()
    )
    assert "curl" not in package_text
    assert "irm https://cua.ai/driver/install.ps1 | iex" not in package_text
