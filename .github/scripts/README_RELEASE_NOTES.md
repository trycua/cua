# Release Notes Generator

A testable Python script for generating release notes that reads from sources of truth (pyproject.toml files) instead of using hardcoded bash scripts.

## Overview

This script replaces the previous bash-based release notes generation with a maintainable, testable Python implementation that:

- Reads package versions directly from `pyproject.toml` files (source of truth)
- Handles both static and dynamic versioning (e.g., pylume)
- Automatically resolves dependency versions from the monorepo
- Generates consistent, package-specific release notes
- Is fully tested with comprehensive unit tests

## Usage

### Basic Usage

```bash
# Generate release notes for a package (auto-detects version)
python3 .github/scripts/generate_release_notes.py cua-agent

# Specify a version explicitly
python3 .github/scripts/generate_release_notes.py cua-agent --version 0.4.35

# Save to a file
python3 .github/scripts/generate_release_notes.py cua-agent --output release_notes.md

# Specify workspace root (if running from different directory)
python3 .github/scripts/generate_release_notes.py cua-agent --workspace-root /path/to/repo
```

### Supported Packages

- `pylume` - Python SDK for lume
- `cua-agent` - CUA agent with extras support
- `cua-computer` - Computer control library
- `cua-som` - Computer vision and OCR library
- `cua-computer-server` - FastAPI-based server
- `cua-mcp-server` - MCP server integration

### Examples

**Generate release notes for cua-agent:**

```bash
python3 .github/scripts/generate_release_notes.py cua-agent
```

Output:

````markdown
# cua-agent v0.4.35

## Dependencies

- cua-computer: v0.4.10
- cua-som: v0.1.3

## Installation Options

### Basic installation with Anthropic

```bash
pip install cua-agent[anthropic]==0.4.35
```
````

### With SOM (recommended)

```bash
pip install cua-agent[som]==0.4.35
```

### All features

```bash
pip install cua-agent[all]==0.4.35
```

````

**Generate release notes for pylume:**
```bash
python3 .github/scripts/generate_release_notes.py pylume
````

Output:

````markdown
# pylume v0.2.1

## Python SDK for lume - run macOS and Linux VMs on Apple Silicon

This package provides Python bindings for the lume virtualization tool.

## Dependencies

- lume binary: v${LUME_VERSION}

## Installation

```bash
pip install pylume==0.2.1
```
````

````

## Integration with GitHub Actions

Replace the bash script in your workflow with:

```yaml
- name: Generate Release Notes
  run: |
    python3 .github/scripts/generate_release_notes.py \
      ${{ inputs.package_name }} \
      --version ${VERSION} \
      --output release_notes.md

    echo "Release notes created:"
    cat release_notes.md
````

### Complete Workflow Example

```yaml
- name: Generate Release Notes
  run: |
    # The script automatically reads versions from pyproject.toml
    python3 .github/scripts/generate_release_notes.py \
      ${{ inputs.package_name }} \
      --output release_notes.md

- name: Create GitHub Release
  uses: softprops/action-gh-release@v1
  with:
    body_path: release_notes.md
    files: dist/*
```

## Running Tests

The script includes comprehensive unit tests:

```bash
# Run all tests
python3 .github/scripts/tests/test_generate_release_notes.py

# Run with verbose output
python3 .github/scripts/tests/test_generate_release_notes.py -v

# Run specific test
python3 -m unittest \
  .github.scripts.tests.test_generate_release_notes.TestReleaseNotesGenerator.test_generate_release_notes_cua_agent_with_extras
```

## How It Works

### Version Detection

The script supports two versioning methods:

1. **Static Version** (most packages):

   ```toml
   [project]
   version = "0.4.35"
   ```

2. **Dynamic Version** (pylume):

   ```toml
   [project]
   dynamic = ["version"]

   [tool.pdm.version]
   source = "file"
   path = "pylume/__init__.py"
   ```

   The script reads `__version__ = "0.2.1"` from the source file.

### Dependency Resolution

Dependencies are automatically resolved from the monorepo:

1. Each package has a `dependencies_from_pyproject` list in the config
2. The script reads the version of each dependency from its pyproject.toml
3. Versions are included in the release notes automatically

Example for `cua-agent`:

- Reads `cua-computer` version from `/libs/python/computer/pyproject.toml`
- Reads `cua-som` version from `/libs/python/som/pyproject.toml`
- Includes both versions in the Dependencies section

### Package Configuration

Package-specific content is defined in the `PACKAGE_CONFIGS` dictionary:

```python
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
            # ... more options
        ],
    },
}
```

## Adding New Packages

To add support for a new package:

1. **Add package directory mapping** in `get_package_version()`:

   ```python
   package_dir_map = {
       # ... existing packages
       "new-package": "libs/python/new-package",
   }
   ```

2. **Add package configuration** in `PACKAGE_CONFIGS`:

   ```python
   "new-package": {
       "title": "Package Title",
       "description": "Package description",
       "dependencies_from_pyproject": ["dep1", "dep2"],
       "has_extras": False,
   }
   ```

3. **Add tests** in `test_generate_release_notes.py`:

   ```python
   def test_generate_release_notes_new_package(self):
       """Test generating release notes for new-package."""
       self.create_pyproject_toml("libs/python/new-package", "1.0.0")
       generator = ReleaseNotesGenerator(self.workspace_root)

       release_notes = generator.generate_release_notes("new-package", "1.0.0")

       self.assertIn("# new-package v1.0.0", release_notes)
   ```

## Benefits Over Bash Script

### 1. **Testability**

- 15 comprehensive unit tests
- Tests for version detection, dependency resolution, and markdown generation
- Easy to add tests for new packages

### 2. **Maintainability**

- Clear Python code vs complex bash conditionals
- Centralized package configuration
- Type hints for better IDE support

### 3. **Source of Truth**

- Versions read from pyproject.toml, not hardcoded
- Dependencies automatically resolved
- Handles both static and dynamic versioning

### 4. **Error Handling**

- Clear error messages
- Validates package names
- Checks for missing versions

### 5. **Extensibility**

- Easy to add new packages
- Simple to add new sections to release notes
- Can be extended for other formats (JSON, HTML, etc.)

## Dependencies

The script requires:

- Python 3.11+ (for `tomllib`) or Python 3.7+ with `tomli` package
- No other dependencies (uses only standard library)

For Python < 3.11, install:

```bash
pip install tomli
```

## Troubleshooting

### Version not found

```
Error: Could not determine version for pylume
```

**Solution:** Ensure the package's `pyproject.toml` exists and has a version field.

### Unknown package

```
Error: Unknown package: my-package
```

**Solution:** Add the package to `PACKAGE_CONFIGS` and `package_dir_map`.

### Dynamic version not detected

**Solution:** Check that:

1. `pyproject.toml` has `dynamic = ["version"]`
2. `tool.pdm.version.path` points to the correct file
3. The source file has `__version__ = "x.y.z"`
