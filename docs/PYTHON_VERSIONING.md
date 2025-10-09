# Python Package Versioning and Release Process

## Overview

The Python SDK packages in this repository now use **`pyproject.toml` as the source of truth** for versioning. When you update the version in a package's `pyproject.toml` file and push to the main branch, the system automatically creates git tags and triggers the release process.

## Quick Start

To release a new version of a package:

1. **Update the version** in the package's `pyproject.toml`:
   ```toml
   [project]
   name = "cua-core"
   version = "0.1.9"  # Changed from 0.1.8
   ```

2. **Commit and push** to the main branch:
   ```bash
   git add libs/python/core/pyproject.toml
   git commit -m "Bump cua-core version to 0.1.9"
   git push origin main
   ```

3. **Automated process** kicks in:
   - Auto-tag workflow detects the version change
   - Creates git tag `core-v0.1.9`
   - Triggers the publish workflow
   - Builds and publishes to PyPI
   - Creates GitHub release

## How It Works

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Developer updates pyproject.toml version                    │
│  and pushes to main branch                                   │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│  Auto-tag Workflow (.github/workflows/                       │
│  auto-tag-from-pyproject.yml)                                │
│  - Detects changed pyproject.toml files                      │
│  - Extracts package name and version                         │
│  - Creates git tag (e.g., core-v0.1.9)                       │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│  Package Publish Workflow (e.g.,                             │
│  .github/workflows/pypi-publish-core.yml)                    │
│  - Triggered by tag push                                     │
│  - Reads version from pyproject.toml                         │
│  - Builds package with PDM                                   │
│  - Publishes to PyPI                                         │
│  - Creates GitHub release                                    │
└─────────────────────────────────────────────────────────────┘
```

### Workflow Files

1. **Auto-tag Workflow** (`.github/workflows/auto-tag-from-pyproject.yml`)
   - Monitors: Changes to `libs/python/*/pyproject.toml` on main branch
   - Action: Creates git tags automatically when version changes

2. **Package Publish Workflows** (`.github/workflows/pypi-publish-*.yml`)
   - Triggered by: Git tags (e.g., `core-v*`, `agent-v*`)
   - Action: Build and publish to PyPI
   - Version source: Reads from `pyproject.toml` (tag version is for validation)

3. **Reusable Publish Workflow** (`.github/workflows/pypi-reusable-publish.yml`)
   - Shared workflow used by all package publish workflows
   - Handles building, publishing, and release creation

## Packages

The following packages are managed by this system:

| Package | PyPI Name | Directory | Workflow |
|---------|-----------|-----------|----------|
| Core | `cua-core` | `libs/python/core` | `pypi-publish-core.yml` |
| Computer | `cua-computer` | `libs/python/computer` | `pypi-publish-computer.yml` |
| SOM | `cua-som` | `libs/python/som` | `pypi-publish-som.yml` |
| Agent | `cua-agent` | `libs/python/agent` | `pypi-publish-agent.yml` |
| Pylume | `pylume` | `libs/python/pylume` | `pypi-publish-pylume.yml` |
| Computer Server | `cua-computer-server` | `libs/python/computer-server` | `pypi-publish-computer-server.yml` |
| MCP Server | `cua-mcp-server` | `libs/python/mcp-server` | `pypi-publish-mcp-server.yml` |

## Version Numbering

We follow [Semantic Versioning](https://semver.org/) (SemVer):

- **MAJOR.MINOR.PATCH** (e.g., `1.2.3`)
- **MAJOR**: Breaking changes
- **MINOR**: New features (backward compatible)
- **PATCH**: Bug fixes (backward compatible)

### Dependency Version Constraints

Related packages use caret version constraints:

```toml
dependencies = [
    "cua-core>=0.1.8,<0.2.0",     # Allow patch updates, not minor
    "cua-computer>=0.4.0,<0.5.0",  # Allow patch updates, not minor
]
```

## Manual Triggers

### Option 1: Manual Workflow Dispatch

You can manually trigger a release without creating a tag:

1. Go to **Actions** → Select package workflow (e.g., "Publish Core Package")
2. Click **"Run workflow"**
3. Enter version (e.g., `0.1.9`)
4. Click **"Run workflow"**

### Option 2: Manual Tag Creation

You can still create tags manually:

```bash
git tag core-v0.1.9
git push origin core-v0.1.9
```

The publish workflow will still read the version from `pyproject.toml` for validation.

## Special Cases

### Agent Package

The agent package has special dependency management:
- Automatically updates to latest versions of `cua-core`, `cua-computer`, and `cua-som` during release
- See `.github/workflows/pypi-publish-agent.yml` for details

### Pylume Package

The pylume package includes the lume binary:
- Downloads latest lume binary during build
- Verifies binary is included in wheel
- See `.github/workflows/pypi-publish-pylume.yml` for details

## Troubleshooting

### Tag Already Exists

If you need to republish a version:
```bash
# Delete local tag
git tag -d core-v0.1.9

# Delete remote tag
git push origin :refs/tags/core-v0.1.9

# Update pyproject.toml and push again
```

### Version Not Detected

Check that:
1. The version field in `pyproject.toml` is formatted correctly:
   ```toml
   version = "0.1.9"  # Correct
   version = '0.1.9'  # Also works
   version = 0.1.9    # Wrong - needs quotes
   ```

2. The `pyproject.toml` file is in the correct location:
   ```
   libs/python/<package-name>/pyproject.toml
   ```

### Workflow Failed

1. Check the **Actions** tab for error details
2. Common issues:
   - TOML parsing error: Check `pyproject.toml` syntax
   - PyPI upload failed: Check version doesn't already exist on PyPI
   - Build failed: Check dependencies and build configuration

## Migration Notes

### What Changed

**Before:**
- Git tags were the source of truth
- Workflow extracted version from tag
- Updated `pyproject.toml` during build

**After:**
- `pyproject.toml` is the source of truth
- Workflow reads version from `pyproject.toml`
- Git tags are created automatically

### Backward Compatibility

The system maintains backward compatibility:
- Manual tag creation still works
- Manual workflow dispatch still works
- `pyproject.toml` version is always used for the actual build

## Best Practices

1. **Update version in one place**: Only update the version in `pyproject.toml`
2. **Commit message**: Use clear commit messages like "Bump cua-core to 0.1.9"
3. **Test locally**: Test your changes with `pdm build` before pushing
4. **Dependencies**: Update dependent package versions when releasing breaking changes
5. **Changelog**: Consider maintaining a CHANGELOG.md for each package

## Examples

### Example 1: Patch Release

Fixing a bug in cua-core:

```bash
# Edit the code
vim libs/python/core/core/telemetry.py

# Update version
vim libs/python/core/pyproject.toml
# Change: version = "0.1.8" → "0.1.9"

# Commit and push
git add .
git commit -m "Fix telemetry bug - bump to 0.1.9"
git push origin main

# Automated:
# - Tag created: core-v0.1.9
# - Package published to PyPI
```

### Example 2: Minor Release

Adding a new feature to cua-agent:

```bash
# Edit the code
vim libs/python/agent/agent/agent.py

# Update version
vim libs/python/agent/pyproject.toml
# Change: version = "0.4.0" → "0.5.0"

# Commit and push
git add .
git commit -m "Add new agent feature - bump to 0.5.0"
git push origin main

# Automated:
# - Tag created: agent-v0.5.0
# - Dependencies updated automatically
# - Package published to PyPI
```

### Example 3: Multiple Packages

Releasing core and computer together:

```bash
# Update both packages
vim libs/python/core/pyproject.toml    # 0.1.8 → 0.1.9
vim libs/python/computer/pyproject.toml # 0.4.0 → 0.4.1

# Commit both
git add .
git commit -m "Bump core to 0.1.9 and computer to 0.4.1"
git push origin main

# Automated:
# - Tag created: core-v0.1.9
# - Tag created: computer-v0.4.1
# - Both packages published
```

## Reference

### File Locations

- Auto-tag workflow: `.github/workflows/auto-tag-from-pyproject.yml`
- Reusable publish: `.github/workflows/pypi-reusable-publish.yml`
- Package workflows: `.github/workflows/pypi-publish-*.yml`
- Package files: `libs/python/*/pyproject.toml`

### Required Secrets

- `PYPI_TOKEN`: PyPI API token for publishing packages
- `GITHUB_TOKEN`: Automatically provided by GitHub Actions

### Permissions

Workflows require:
- `contents: write` - For creating tags and releases
- `actions: read` - For reading workflow status
