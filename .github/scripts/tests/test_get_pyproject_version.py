"""
Comprehensive tests for get_pyproject_version.py script using unittest.

This test suite covers:
- Version matching validation
- Error handling for missing versions
- Invalid input handling
- File not found scenarios
- Malformed TOML handling
"""

import sys
import tempfile
import unittest
from io import StringIO
from pathlib import Path
from unittest.mock import patch

# Add parent directory to path to import the module
sys.path.insert(0, str(Path(__file__).parent.parent))

# Import after path is modified
import get_pyproject_version


class TestGetPyprojectVersion(unittest.TestCase):
    """Test suite for get_pyproject_version.py functionality."""

    def setUp(self):
        """Reset sys.argv before each test."""
        self.original_argv = sys.argv.copy()

    def tearDown(self):
        """Restore sys.argv after each test."""
        sys.argv = self.original_argv

    def create_pyproject_toml(self, version: str) -> Path:
        """Helper to create a temporary pyproject.toml file with a given version."""
        temp_file = tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False)
        temp_file.write(
            f"""
[project]
name = "test-project"
version = "{version}"
description = "A test project"
"""
        )
        temp_file.close()
        return Path(temp_file.name)

    def create_pyproject_toml_no_version(self) -> Path:
        """Helper to create a pyproject.toml without a version field."""
        temp_file = tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False)
        temp_file.write(
            """
[project]
name = "test-project"
description = "A test project without version"
"""
        )
        temp_file.close()
        return Path(temp_file.name)

    def create_pyproject_toml_no_project(self) -> Path:
        """Helper to create a pyproject.toml without a project section."""
        temp_file = tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False)
        temp_file.write(
            """
[tool.poetry]
name = "test-project"
version = "1.0.0"
"""
        )
        temp_file.close()
        return Path(temp_file.name)

    def create_malformed_toml(self) -> Path:
        """Helper to create a malformed TOML file."""
        temp_file = tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False)
        temp_file.write(
            """
[project
name = "test-project
version = "1.0.0"
"""
        )
        temp_file.close()
        return Path(temp_file.name)

    # Test: Successful version match
    def test_matching_versions(self):
        """Test that matching versions result in success."""
        pyproject_file = self.create_pyproject_toml("1.2.3")

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.2.3"]

            # Capture stdout
            captured_output = StringIO()
            with patch("sys.stdout", captured_output):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 0)
            self.assertIn("✅ Version consistency check passed: 1.2.3", captured_output.getvalue())
        finally:
            pyproject_file.unlink()

    # Test: Version mismatch
    def test_version_mismatch(self):
        """Test that mismatched versions result in failure with appropriate error message."""
        pyproject_file = self.create_pyproject_toml("1.2.3")

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.2.4"]

            # Capture stderr
            captured_error = StringIO()
            with patch("sys.stderr", captured_error):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 1)
            error_output = captured_error.getvalue()
            self.assertIn("❌ Version mismatch detected!", error_output)
            self.assertIn("pyproject.toml version: 1.2.3", error_output)
            self.assertIn("Expected version: 1.2.4", error_output)
            self.assertIn("Please update pyproject.toml to version 1.2.4", error_output)
        finally:
            pyproject_file.unlink()

    # Test: Missing version in pyproject.toml
    def test_missing_version_field(self):
        """Test handling of pyproject.toml without a version field."""
        pyproject_file = self.create_pyproject_toml_no_version()

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.0.0"]

            captured_error = StringIO()
            with patch("sys.stderr", captured_error):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 1)
            self.assertIn("❌ ERROR: No version found in pyproject.toml", captured_error.getvalue())
        finally:
            pyproject_file.unlink()

    # Test: Missing project section
    def test_missing_project_section(self):
        """Test handling of pyproject.toml without a project section."""
        pyproject_file = self.create_pyproject_toml_no_project()

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.0.0"]

            captured_error = StringIO()
            with patch("sys.stderr", captured_error):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 1)
            self.assertIn("❌ ERROR: No version found in pyproject.toml", captured_error.getvalue())
        finally:
            pyproject_file.unlink()

    # Test: File not found
    def test_file_not_found(self):
        """Test handling of non-existent pyproject.toml file."""
        sys.argv = ["get_pyproject_version.py", "/nonexistent/pyproject.toml", "1.0.0"]

        with self.assertRaises(SystemExit) as cm:
            get_pyproject_version.main()

        self.assertEqual(cm.exception.code, 1)

    # Test: Malformed TOML
    def test_malformed_toml(self):
        """Test handling of malformed TOML file."""
        pyproject_file = self.create_malformed_toml()

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.0.0"]

            with self.assertRaises(SystemExit) as cm:
                get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 1)
        finally:
            pyproject_file.unlink()

    # Test: Incorrect number of arguments - too few
    def test_too_few_arguments(self):
        """Test that providing too few arguments results in usage error."""
        sys.argv = ["get_pyproject_version.py", "pyproject.toml"]

        captured_error = StringIO()
        with patch("sys.stderr", captured_error):
            with self.assertRaises(SystemExit) as cm:
                get_pyproject_version.main()

        self.assertEqual(cm.exception.code, 1)
        self.assertIn(
            "Usage: python get_pyproject_version.py <pyproject_path> <expected_version>",
            captured_error.getvalue(),
        )

    # Test: Incorrect number of arguments - too many
    def test_too_many_arguments(self):
        """Test that providing too many arguments results in usage error."""
        sys.argv = ["get_pyproject_version.py", "pyproject.toml", "1.0.0", "extra"]

        captured_error = StringIO()
        with patch("sys.stderr", captured_error):
            with self.assertRaises(SystemExit) as cm:
                get_pyproject_version.main()

        self.assertEqual(cm.exception.code, 1)
        self.assertIn(
            "Usage: python get_pyproject_version.py <pyproject_path> <expected_version>",
            captured_error.getvalue(),
        )

    # Test: No arguments
    def test_no_arguments(self):
        """Test that providing no arguments results in usage error."""
        sys.argv = ["get_pyproject_version.py"]

        captured_error = StringIO()
        with patch("sys.stderr", captured_error):
            with self.assertRaises(SystemExit) as cm:
                get_pyproject_version.main()

        self.assertEqual(cm.exception.code, 1)
        self.assertIn(
            "Usage: python get_pyproject_version.py <pyproject_path> <expected_version>",
            captured_error.getvalue(),
        )

    # Test: Version with pre-release tags
    def test_version_with_prerelease_tags(self):
        """Test matching versions with pre-release tags like alpha, beta, rc."""
        pyproject_file = self.create_pyproject_toml("1.2.3-rc.1")

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.2.3-rc.1"]

            captured_output = StringIO()
            with patch("sys.stdout", captured_output):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 0)
            self.assertIn(
                "✅ Version consistency check passed: 1.2.3-rc.1", captured_output.getvalue()
            )
        finally:
            pyproject_file.unlink()

    # Test: Version with build metadata
    def test_version_with_build_metadata(self):
        """Test matching versions with build metadata."""
        pyproject_file = self.create_pyproject_toml("1.2.3+build.123")

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.2.3+build.123"]

            captured_output = StringIO()
            with patch("sys.stdout", captured_output):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 0)
            self.assertIn(
                "✅ Version consistency check passed: 1.2.3+build.123", captured_output.getvalue()
            )
        finally:
            pyproject_file.unlink()

    # Test: Various semantic version formats
    def test_semantic_version_0_0_1(self):
        """Test semantic version 0.0.1."""
        self._test_version_format("0.0.1")

    def test_semantic_version_1_0_0(self):
        """Test semantic version 1.0.0."""
        self._test_version_format("1.0.0")

    def test_semantic_version_10_20_30(self):
        """Test semantic version 10.20.30."""
        self._test_version_format("10.20.30")

    def test_semantic_version_alpha(self):
        """Test semantic version with alpha tag."""
        self._test_version_format("1.2.3-alpha")

    def test_semantic_version_beta(self):
        """Test semantic version with beta tag."""
        self._test_version_format("1.2.3-beta.1")

    def test_semantic_version_rc_with_build(self):
        """Test semantic version with rc and build metadata."""
        self._test_version_format("1.2.3-rc.1+build.456")

    def _test_version_format(self, version: str):
        """Helper method to test various semantic version formats."""
        pyproject_file = self.create_pyproject_toml(version)

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), version]

            captured_output = StringIO()
            with patch("sys.stdout", captured_output):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 0)
            self.assertIn(
                f"✅ Version consistency check passed: {version}", captured_output.getvalue()
            )
        finally:
            pyproject_file.unlink()

    # Test: Empty version string
    def test_empty_version_string(self):
        """Test handling of empty version string."""
        pyproject_file = self.create_pyproject_toml("")

        try:
            sys.argv = ["get_pyproject_version.py", str(pyproject_file), "1.0.0"]

            captured_error = StringIO()
            with patch("sys.stderr", captured_error):
                with self.assertRaises(SystemExit) as cm:
                    get_pyproject_version.main()

            self.assertEqual(cm.exception.code, 1)
            # Empty string is falsy, so it should trigger error
            self.assertIn("❌", captured_error.getvalue())
        finally:
            pyproject_file.unlink()


class TestSuiteInfo(unittest.TestCase):
    """Test suite metadata."""

    def test_suite_info(self):
        """Display test suite information."""
        print("\n" + "=" * 70)
        print("Test Suite: get_pyproject_version.py")
        print("Framework: unittest (Python built-in)")
        print("TOML Library: tomllib (Python 3.11+ built-in)")
        print("=" * 70)
        self.assertTrue(True)


if __name__ == "__main__":
    # Run tests with verbose output
    unittest.main(verbosity=2)
