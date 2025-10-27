#!/usr/bin/env python3
"""
Generate release notes for packages by reading from sources of truth.

This script reads version information from pyproject.toml files and generates
markdown release notes with package-specific content and dependency information.
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Dict, Optional

try:
    import tomllib
except ImportError:
    import tomli as tomllib


class ReleaseNotesGenerator:
    """Generate release notes for different package types."""

    # Package metadata
    PACKAGE_CONFIGS = {
        "pylume": {
            "title": "Python SDK for lume - run macOS and Linux VMs on Apple Silicon",
            "description": "This package provides Python bindings for the lume virtualization tool.",
            "dependencies_from_pyproject": ["lume"],
            "has_extras": False,
        },
        "cua-computer": {
            "title": "Computer control library for the Computer Universal Automation (CUA) project",
            "description": None,
            "dependencies_from_pyproject": ["pylume"],
            "has_extras": False,
        },
        "cua-agent": {
            "title": None,
            "description": None,
            "dependencies_from_pyproject": ["cua-computer", "cua-som"],
            "has_extras": True,
            "extras_info": {
                "description": "Installation Options",
                "options": [
                    {
                        "name": "Basic installation with Anthropic",
                        "command": "pip install cua-agent[anthropic]=={version}",
                    },
                    {
                        "name": "With SOM (recommended)",
                        "command": "pip install cua-agent[som]=={version}",
                    },
                    {
                        "name": "All features",
                        "command": "pip install cua-agent[all]=={version}",
                    },
                ],
            },
        },
        "cua-som": {
            "title": "Computer Vision and OCR library for detecting and analyzing UI elements",
            "description": "This package provides enhanced UI understanding capabilities through computer vision and OCR.",
            "dependencies_from_pyproject": [],
            "has_extras": False,
        },
        "cua-computer-server": {
            "title": "Computer Server for the Computer Universal Automation (CUA) project",
            "description": "A FastAPI-based server implementation for computer control.",
            "dependencies_from_pyproject": ["cua-computer"],
            "has_extras": False,
            "usage": [
                "# Run the server",
                "cua-computer-server",
            ],
        },
        "cua-mcp-server": {
            "title": "MCP Server for the Computer-Use Agent (CUA)",
            "description": "This package provides MCP (Model Context Protocol) integration for CUA agents, allowing them to be used with Claude Desktop, Cursor, and other MCP clients.",
            "dependencies_from_pyproject": ["cua-computer", "cua-agent"],
            "has_extras": False,
            "usage": [
                "# Run the MCP server directly",
                "cua-mcp-server",
            ],
            "additional_sections": [
                {
                    "title": "Claude Desktop Integration",
                    "content": [
                        "Add to your Claude Desktop configuration (~/.config/claude-desktop/claude_desktop_config.json or OS-specific location):",
                        '```json',
                        '"mcpServers": {',
                        '  "cua-agent": {',
                        '    "command": "cua-mcp-server",',
                        '    "args": [],',
                        '    "env": {',
                        '      "CUA_AGENT_LOOP": "OMNI",',
                        '      "CUA_MODEL_PROVIDER": "ANTHROPIC",',
                        '      "CUA_MODEL_NAME": "claude-3-opus-20240229",',
                        '      "ANTHROPIC_API_KEY": "your-api-key",',
                        '      "PYTHONIOENCODING": "utf-8"',
                        '    }',
                        '  }',
                        '}',
                        '```',
                    ],
                },
            ],
        },
    }

    def __init__(self, workspace_root: Path):
        """
        Initialize the release notes generator.

        Args:
            workspace_root: Path to the workspace root directory
        """
        self.workspace_root = workspace_root

    def get_package_version(self, package_name: str) -> Optional[str]:
        """
        Get the version of a package from its pyproject.toml.

        Args:
            package_name: Name of the package (e.g., 'cua-agent', 'pylume')

        Returns:
            Version string or None if not found
        """
        # Map package name to directory structure
        package_dir_map = {
            "pylume": "libs/python/pylume",
            "cua-agent": "libs/python/agent",
            "cua-computer": "libs/python/computer",
            "cua-som": "libs/python/som",
            "cua-computer-server": "libs/python/computer-server",
            "cua-mcp-server": "libs/python/mcp-server",
            "cua-core": "libs/python/core",
        }

        if package_name not in package_dir_map:
            return None

        package_path = self.workspace_root / package_dir_map[package_name]
        pyproject_path = package_path / "pyproject.toml"

        if not pyproject_path.exists():
            return None

        try:
            with open(pyproject_path, "rb") as f:
                pyproject_data = tomllib.load(f)

            # Check if version is static (directly in project section)
            if "version" in pyproject_data.get("project", {}):
                return pyproject_data["project"]["version"]

            # Check if version is dynamic
            if "dynamic" in pyproject_data.get("project", {}) and "version" in pyproject_data["project"]["dynamic"]:
                # Get the version source file
                pdm_version = pyproject_data.get("tool", {}).get("pdm", {}).get("version", {})
                if pdm_version.get("source") == "file":
                    version_file_path = package_path / pdm_version["path"]
                    if version_file_path.exists():
                        content = version_file_path.read_text()
                        match = re.search(r'__version__\s*=\s*["\']([^"\']+)["\']', content)
                        if match:
                            return match.group(1)

        except Exception as e:
            print(f"Error reading version for {package_name}: {e}", file=sys.stderr)
            return None

        return None

    def get_package_dependencies(self, package_name: str) -> Dict[str, Optional[str]]:
        """
        Get dependency versions for a package from its pyproject.toml.

        Args:
            package_name: Name of the package

        Returns:
            Dictionary mapping dependency names to their versions
        """
        config = self.PACKAGE_CONFIGS.get(package_name, {})
        deps_to_check = config.get("dependencies_from_pyproject", [])

        dependencies = {}
        for dep_name in deps_to_check:
            version = self.get_package_version(dep_name)
            dependencies[dep_name] = version

        return dependencies

    def generate_release_notes(self, package_name: str, version: Optional[str] = None) -> str:
        """
        Generate release notes for a package.

        Args:
            package_name: Name of the package
            version: Version string (if None, will be read from pyproject.toml)

        Returns:
            Markdown-formatted release notes
        """
        if package_name not in self.PACKAGE_CONFIGS:
            raise ValueError(f"Unknown package: {package_name}")

        # Get version from source of truth if not provided
        if version is None:
            version = self.get_package_version(package_name)
            if version is None:
                raise ValueError(f"Could not determine version for {package_name}")

        config = self.PACKAGE_CONFIGS[package_name]
        lines = []

        # Title
        lines.append(f"# {package_name} v{version}")
        lines.append("")

        # Package description
        if config.get("title"):
            lines.append(f"## {config['title']}")
            lines.append("")

        if config.get("description"):
            lines.append(config["description"])
            lines.append("")

        # Dependencies section
        dependencies = self.get_package_dependencies(package_name)
        if dependencies:
            lines.append("## Dependencies")
            for dep_name, dep_version in dependencies.items():
                # Special handling for lume binary (external dependency)
                if dep_name == "lume":
                    # For lume, we'd need to read from another source (GitHub release, etc.)
                    # For now, use placeholder that can be replaced
                    lines.append(f"* lume binary: v${{LUME_VERSION}}")
                else:
                    version_str = f"v{dep_version}" if dep_version else "latest"
                    lines.append(f"* {dep_name}: {version_str}")
            lines.append("")

        # Installation extras (for cua-agent)
        if config.get("has_extras"):
            extras_info = config.get("extras_info", {})
            lines.append(f"## {extras_info.get('description', 'Installation Options')}")
            lines.append("")

            for option in extras_info.get("options", []):
                lines.append(f"### {option['name']}")
                lines.append("```bash")
                lines.append(option["command"].format(version=version))
                lines.append("```")
                lines.append("")

        # Usage section
        if config.get("usage"):
            lines.append("## Usage")
            lines.append("```bash")
            for usage_line in config["usage"]:
                lines.append(usage_line)
            lines.append("```")
            lines.append("")

        # Additional sections (for mcp-server)
        if config.get("additional_sections"):
            for section in config["additional_sections"]:
                lines.append(f"## {section['title']}")
                for content_line in section["content"]:
                    lines.append(content_line)
                lines.append("")

        # Standard installation section (unless package has extras)
        if not config.get("has_extras"):
            lines.append("## Installation")
            lines.append("```bash")
            lines.append(f"pip install {package_name}=={version}")
            lines.append("```")
            lines.append("")

        return "\n".join(lines).rstrip() + "\n"


def main():
    """Main entry point for the script."""
    parser = argparse.ArgumentParser(
        description="Generate release notes for packages from sources of truth"
    )
    parser.add_argument(
        "package_name",
        help="Name of the package (e.g., pylume, cua-agent, cua-computer)",
    )
    parser.add_argument(
        "--version",
        help="Version string (optional, will be read from pyproject.toml if not provided)",
    )
    parser.add_argument(
        "--workspace-root",
        type=Path,
        default=Path.cwd(),
        help="Path to workspace root (default: current directory)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output file path (default: stdout)",
    )

    args = parser.parse_args()

    try:
        generator = ReleaseNotesGenerator(args.workspace_root)
        release_notes = generator.generate_release_notes(args.package_name, args.version)

        if args.output:
            args.output.write_text(release_notes)
            print(f"Release notes written to {args.output}", file=sys.stderr)
        else:
            print(release_notes)

        return 0

    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
