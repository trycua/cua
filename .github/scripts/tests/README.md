# Tests for .github/scripts

This directory contains comprehensive tests for the GitHub workflow scripts using Python's built-in testing framework.

## Requirements

**No external dependencies required!**

This test suite uses:

- `unittest` - Python's built-in testing framework
- `tomllib` - Python 3.11+ built-in TOML parser

For Python < 3.11, the `toml` package is used as a fallback.

## Running Tests

### Run all tests

```bash
cd .github/scripts/tests
python3 -m unittest discover -v
```

### Run a specific test file

```bash
python3 -m unittest test_get_pyproject_version -v
```

### Run a specific test class

```bash
python3 -m unittest test_get_pyproject_version.TestGetPyprojectVersion -v
```

### Run a specific test method

```bash
python3 -m unittest test_get_pyproject_version.TestGetPyprojectVersion.test_matching_versions -v
```

### Run tests directly from the test file

```bash
python3 test_get_pyproject_version.py
```

## Test Structure

### test_get_pyproject_version.py

Comprehensive tests for `get_pyproject_version.py` covering:

- ✅ **Version matching**: Tests successful version validation
- ✅ **Version mismatch**: Tests error handling when versions don't match
- ✅ **Missing version**: Tests handling of pyproject.toml without version field
- ✅ **Missing project section**: Tests handling of pyproject.toml without project section
- ✅ **File not found**: Tests handling of non-existent files
- ✅ **Malformed TOML**: Tests handling of invalid TOML syntax
- ✅ **Argument validation**: Tests proper argument count validation
- ✅ **Semantic versioning**: Tests various semantic version formats
- ✅ **Pre-release tags**: Tests versions with alpha, beta, rc tags
- ✅ **Build metadata**: Tests versions with build metadata
- ✅ **Edge cases**: Tests empty versions and other edge cases

**Total Tests**: 17+ test cases covering all functionality

## Best Practices Implemented

1. **Fixture Management**: Uses `setUp()` and `tearDown()` for clean test isolation
2. **Helper Methods**: Provides reusable helpers for creating test fixtures
3. **Temporary Files**: Uses `tempfile` for file creation with proper cleanup
4. **Comprehensive Coverage**: Tests happy paths, error conditions, and edge cases
5. **Clear Documentation**: Each test has a descriptive docstring
6. **Output Capture**: Uses `unittest.mock.patch` and `StringIO` to test stdout/stderr
7. **Exit Code Validation**: Properly tests script exit codes with `assertRaises(SystemExit)`
8. **Type Hints**: Uses type hints in helper methods for clarity
9. **PEP 8 Compliance**: Follows Python style guidelines
10. **Zero External Dependencies**: Uses only Python standard library

## Continuous Integration

These tests can be integrated into GitHub Actions workflows with no additional dependencies:

```yaml
- name: Run .github scripts tests
  run: |
    cd .github/scripts/tests
    python3 -m unittest discover -v
```

## Test Output Example

```
test_empty_version_string (test_get_pyproject_version.TestGetPyprojectVersion)
Test handling of empty version string. ... ok
test_file_not_found (test_get_pyproject_version.TestGetPyprojectVersion)
Test handling of non-existent pyproject.toml file. ... ok
test_malformed_toml (test_get_pyproject_version.TestGetPyprojectVersion)
Test handling of malformed TOML file. ... ok
test_matching_versions (test_get_pyproject_version.TestGetPyprojectVersion)
Test that matching versions result in success. ... ok
test_missing_project_section (test_get_pyproject_version.TestGetPyprojectVersion)
Test handling of pyproject.toml without a project section. ... ok
test_missing_version_field (test_get_pyproject_version.TestGetPyprojectVersion)
Test handling of pyproject.toml without a version field. ... ok
test_no_arguments (test_get_pyproject_version.TestGetPyprojectVersion)
Test that providing no arguments results in usage error. ... ok
test_semantic_version_0_0_1 (test_get_pyproject_version.TestGetPyprojectVersion)
Test semantic version 0.0.1. ... ok
test_semantic_version_1_0_0 (test_get_pyproject_version.TestGetPyprojectVersion)
Test semantic version 1.0.0. ... ok
test_semantic_version_10_20_30 (test_get_pyproject_version.TestGetPyprojectVersion)
Test semantic version 10.20.30. ... ok
test_semantic_version_alpha (test_get_pyproject_version.TestGetPyprojectVersion)
Test semantic version with alpha tag. ... ok
test_semantic_version_beta (test_get_pyproject_version.TestGetPyprojectVersion)
Test semantic version with beta tag. ... ok
test_semantic_version_rc_with_build (test_get_pyproject_version.TestGetPyprojectVersion)
Test semantic version with rc and build metadata. ... ok
test_too_few_arguments (test_get_pyproject_version.TestGetPyprojectVersion)
Test that providing too few arguments results in usage error. ... ok
test_too_many_arguments (test_get_pyproject_version.TestGetPyprojectVersion)
Test that providing too many arguments results in usage error. ... ok
test_version_mismatch (test_get_pyproject_version.TestGetPyprojectVersion)
Test that mismatched versions result in failure with appropriate error message. ... ok
test_version_with_build_metadata (test_get_pyproject_version.TestGetPyprojectVersion)
Test matching versions with build metadata. ... ok
test_version_with_prerelease_tags (test_get_pyproject_version.TestGetPyprojectVersion)
Test matching versions with pre-release tags like alpha, beta, rc. ... ok

----------------------------------------------------------------------
Ran 18 tests in 0.XXXs

OK
```
