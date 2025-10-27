#!/usr/bin/env python3
"""
Tests for the generate_release_notes.py script.
"""

import sys
import tempfile
import unittest
from pathlib import Path

# Add parent directory to path to import the script
sys.path.insert(0, str(Path(__file__).parent.parent))

from generate_release_notes import ReleaseNotesGenerator


class TestReleaseNotesGenerator(unittest.TestCase):
    """Test cases for ReleaseNotesGenerator."""

    def setUp(self):
        """Set up test fixtures."""
        self.temp_dir = tempfile.mkdtemp()
        self.workspace_root = Path(self.temp_dir)

    def create_pyproject_toml(self, package_dir: str, version: str, is_dynamic: bool = False):
        """
        Helper to create a pyproject.toml file for testing.

        Args:
            package_dir: Package directory path relative to workspace root
            version: Version string
            is_dynamic: Whether to use dynamic versioning
        """
        package_path = self.workspace_root / package_dir
        package_path.mkdir(parents=True, exist_ok=True)
        pyproject_path = package_path / "pyproject.toml"

        if is_dynamic:
            # Create __init__.py with version
            init_file = package_path / package_dir.split("/")[-1] / "__init__.py"
            init_file.parent.mkdir(parents=True, exist_ok=True)
            init_file.write_text(f'__version__ = "{version}"\n')

            pyproject_content = f'''[project]
name = "{package_dir.split('/')[-1]}"
dynamic = ["version"]

[tool.pdm.version]
source = "file"
path = "{package_dir.split('/')[-1]}/__init__.py"
'''
        else:
            pyproject_content = f'''[project]
name = "{package_dir.split('/')[-1]}"
version = "{version}"
'''

        pyproject_path.write_text(pyproject_content)

    def test_get_package_version_static(self):
        """Test getting version from static pyproject.toml."""
        self.create_pyproject_toml("libs/python/agent", "0.4.35")
        generator = ReleaseNotesGenerator(self.workspace_root)

        version = generator.get_package_version("cua-agent")
        self.assertEqual(version, "0.4.35")

    def test_get_package_version_dynamic(self):
        """Test getting version from dynamic pyproject.toml."""
        self.create_pyproject_toml("libs/python/pylume", "0.2.1", is_dynamic=True)
        generator = ReleaseNotesGenerator(self.workspace_root)

        version = generator.get_package_version("pylume")
        self.assertEqual(version, "0.2.1")

    def test_get_package_version_not_found(self):
        """Test behavior when package directory doesn't exist."""
        generator = ReleaseNotesGenerator(self.workspace_root)

        version = generator.get_package_version("nonexistent-package")
        self.assertIsNone(version)

    def test_generate_release_notes_pylume(self):
        """Test generating release notes for pylume package."""
        self.create_pyproject_toml("libs/python/pylume", "0.2.1", is_dynamic=True)
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("pylume", "0.2.1")

        # Check key components are present
        self.assertIn("# pylume v0.2.1", release_notes)
        self.assertIn("Python SDK for lume", release_notes)
        self.assertIn("## Dependencies", release_notes)
        self.assertIn("lume binary:", release_notes)
        self.assertIn("## Installation", release_notes)
        self.assertIn("pip install pylume==0.2.1", release_notes)

    def test_generate_release_notes_cua_computer(self):
        """Test generating release notes for cua-computer package."""
        self.create_pyproject_toml("libs/python/computer", "0.4.10")
        self.create_pyproject_toml("libs/python/pylume", "0.2.1", is_dynamic=True)
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("cua-computer", "0.4.10")

        # Check key components
        self.assertIn("# cua-computer v0.4.10", release_notes)
        self.assertIn("Computer control library", release_notes)
        self.assertIn("## Dependencies", release_notes)
        self.assertIn("pylume: v0.2.1", release_notes)
        self.assertIn("pip install cua-computer==0.4.10", release_notes)

    def test_generate_release_notes_cua_agent_with_extras(self):
        """Test generating release notes for cua-agent with extras."""
        self.create_pyproject_toml("libs/python/agent", "0.4.35")
        self.create_pyproject_toml("libs/python/computer", "0.4.10")
        self.create_pyproject_toml("libs/python/som", "0.1.3")
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("cua-agent", "0.4.35")

        # Check extras installation options
        self.assertIn("# cua-agent v0.4.35", release_notes)
        self.assertIn("## Installation Options", release_notes)
        self.assertIn("Basic installation with Anthropic", release_notes)
        self.assertIn("pip install cua-agent[anthropic]==0.4.35", release_notes)
        self.assertIn("With SOM (recommended)", release_notes)
        self.assertIn("pip install cua-agent[som]==0.4.35", release_notes)
        self.assertIn("All features", release_notes)
        self.assertIn("pip install cua-agent[all]==0.4.35", release_notes)

        # Check dependencies
        self.assertIn("## Dependencies", release_notes)
        self.assertIn("cua-computer: v0.4.10", release_notes)
        self.assertIn("cua-som: v0.1.3", release_notes)

    def test_generate_release_notes_cua_som(self):
        """Test generating release notes for cua-som package."""
        self.create_pyproject_toml("libs/python/som", "0.1.3")
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("cua-som", "0.1.3")

        # Check key components
        self.assertIn("# cua-som v0.1.3", release_notes)
        self.assertIn("Computer Vision and OCR", release_notes)
        self.assertIn("pip install cua-som==0.1.3", release_notes)

    def test_generate_release_notes_computer_server(self):
        """Test generating release notes for cua-computer-server."""
        self.create_pyproject_toml("libs/python/computer-server", "0.1.27")
        self.create_pyproject_toml("libs/python/computer", "0.4.10")
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("cua-computer-server", "0.1.27")

        # Check key components
        self.assertIn("# cua-computer-server v0.1.27", release_notes)
        self.assertIn("FastAPI-based server", release_notes)
        self.assertIn("## Usage", release_notes)
        self.assertIn("cua-computer-server", release_notes)
        self.assertIn("cua-computer: v0.4.10", release_notes)

    def test_generate_release_notes_mcp_server(self):
        """Test generating release notes for cua-mcp-server."""
        self.create_pyproject_toml("libs/python/mcp-server", "0.1.15")
        self.create_pyproject_toml("libs/python/computer", "0.4.10")
        self.create_pyproject_toml("libs/python/agent", "0.4.35")
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("cua-mcp-server", "0.1.15")

        # Check key components
        self.assertIn("# cua-mcp-server v0.1.15", release_notes)
        self.assertIn("MCP (Model Context Protocol)", release_notes)
        self.assertIn("## Claude Desktop Integration", release_notes)
        self.assertIn("claude_desktop_config.json", release_notes)
        self.assertIn("cua-mcp-server", release_notes)
        self.assertIn('"mcpServers":', release_notes)

    def test_generate_release_notes_auto_version(self):
        """Test generating release notes without providing version."""
        self.create_pyproject_toml("libs/python/som", "0.1.3")
        generator = ReleaseNotesGenerator(self.workspace_root)

        # Don't provide version - should read from pyproject.toml
        release_notes = generator.generate_release_notes("cua-som")

        self.assertIn("# cua-som v0.1.3", release_notes)

    def test_generate_release_notes_unknown_package(self):
        """Test error handling for unknown package."""
        generator = ReleaseNotesGenerator(self.workspace_root)

        with self.assertRaises(ValueError) as context:
            generator.generate_release_notes("unknown-package", "1.0.0")

        self.assertIn("Unknown package", str(context.exception))

    def test_generate_release_notes_no_version(self):
        """Test error handling when version cannot be determined."""
        generator = ReleaseNotesGenerator(self.workspace_root)

        with self.assertRaises(ValueError) as context:
            generator.generate_release_notes("pylume")

        self.assertIn("Could not determine version", str(context.exception))

    def test_get_package_dependencies(self):
        """Test getting package dependencies with versions."""
        self.create_pyproject_toml("libs/python/agent", "0.4.35")
        self.create_pyproject_toml("libs/python/computer", "0.4.10")
        self.create_pyproject_toml("libs/python/som", "0.1.3")
        generator = ReleaseNotesGenerator(self.workspace_root)

        dependencies = generator.get_package_dependencies("cua-agent")

        self.assertEqual(dependencies["cua-computer"], "0.4.10")
        self.assertEqual(dependencies["cua-som"], "0.1.3")

    def test_get_package_dependencies_missing(self):
        """Test getting dependencies when some are missing."""
        self.create_pyproject_toml("libs/python/agent", "0.4.35")
        # Don't create computer or som
        generator = ReleaseNotesGenerator(self.workspace_root)

        dependencies = generator.get_package_dependencies("cua-agent")

        self.assertIsNone(dependencies["cua-computer"])
        self.assertIsNone(dependencies["cua-som"])

    def test_release_notes_structure(self):
        """Test that release notes have proper markdown structure."""
        self.create_pyproject_toml("libs/python/som", "0.1.3")
        generator = ReleaseNotesGenerator(self.workspace_root)

        release_notes = generator.generate_release_notes("cua-som", "0.1.3")

        # Check markdown structure
        lines = release_notes.split("\n")

        # Should start with H1
        self.assertTrue(lines[0].startswith("# "))

        # Should have proper spacing
        self.assertEqual(lines[1], "")  # Empty line after title

        # Should end with single newline
        self.assertTrue(release_notes.endswith("\n"))
        self.assertFalse(release_notes.endswith("\n\n"))


if __name__ == "__main__":
    unittest.main()
