# Python Package Release Process

## Overview

The Python SDK packages use `pyproject.toml` as the **source of truth** for version management. When you update a package version in `pyproject.toml` and push to the `main` branch, the release process is automated.

## How It Works

### 1. Version Update in pyproject.toml

Update the version in the package's `pyproject.toml` file:

```toml
[project]
name = "cua-core"
version = "0.1.9"  # Update this version
```

### 2. Automatic Tag Creation

When you push the updated `pyproject.toml` to the `main` branch:

1. The **Version Tag Creator** workflow (`.github/workflows/pypi-version-tag-creator.yml`) automatically detects the version change
2. It compares the current version with the previous commit's version
3. If changed, it creates a git tag in the format: `{package}-v{version}` (e.g., `core-v0.1.9`)
4. The tag is automatically pushed to the repository

### 3. Automatic Package Publishing

When the tag is created:

1. The package-specific publish workflow (e.g., `.github/workflows/pypi-publish-core.yml`) is triggered
2. The workflow performs a **version consistency check** to ensure the tag version matches `pyproject.toml`
3. If the check passes, the package is built and published to PyPI
4. A GitHub release is created with release notes

## Package Mapping

| Package Directory | Tag Prefix | PyPI Package Name |
|-------------------|------------|-------------------|
| `libs/python/core` | `core` | `cua-core` |
| `libs/python/pylume` | `pylume` | `pylume` |
| `libs/python/computer` | `computer` | `cua-computer` |
| `libs/python/som` | `som` | `cua-som` |
| `libs/python/agent` | `agent` | `cua-agent` |
| `libs/python/computer-server` | `computer-server` | `cua-computer-server` |
| `libs/python/mcp-server` | `mcp-server` | `cua-mcp-server` |

## Step-by-Step Release Guide

### Example: Releasing cua-core v0.1.9

1. **Update the version in pyproject.toml:**
   ```bash
   cd libs/python/core
   # Edit pyproject.toml and change version to "0.1.9"
   ```

2. **Commit and push to main:**
   ```bash
   git add libs/python/core/pyproject.toml
   git commit -m "Bump cua-core to v0.1.9"
   git push origin main
   ```

3. **Automatic process:**
   - The Version Tag Creator workflow runs automatically
   - Creates and pushes tag `core-v0.1.9`
   - The `pypi-publish-core.yml` workflow is triggered by the tag
   - Version consistency check ensures pyproject.toml version matches tag
   - Package is built and published to PyPI
   - GitHub release is created

4. **Monitor the release:**
   - Check the Actions tab in GitHub to see the workflow progress
   - Once complete, verify the package on PyPI: https://pypi.org/project/cua-core/

## Version Consistency Check

The publish workflow includes a **mandatory version consistency check** that:

- Extracts the version from `pyproject.toml`
- Compares it with the version from the git tag
- **Fails the build** if they don't match

This ensures that:
- You cannot publish a version that doesn't match `pyproject.toml`
- `pyproject.toml` remains the single source of truth
- Manual tag creation with incorrect versions will fail

### Example Error

If you manually create a tag `core-v0.2.0` but `pyproject.toml` has version `0.1.9`:

```
❌ Version mismatch detected!
   pyproject.toml version: 0.1.9
   Tag/input version: 0.2.0

The version in pyproject.toml must match the version being published.
Please update pyproject.toml to version 0.2.0 or use the correct tag.
```

## Manual Tag Creation (Not Recommended)

While the automated process is preferred, you can still manually create tags:

```bash
# This will work only if pyproject.toml has version "0.1.9"
git tag core-v0.1.9
git push origin core-v0.1.9
```

The version consistency check will still run and fail if the tag doesn't match `pyproject.toml`.

## Manual Workflow Dispatch (Advanced)

You can manually trigger the publish workflow from the GitHub Actions UI:

1. Go to the Actions tab
2. Select the package-specific workflow (e.g., "Publish Core Package")
3. Click "Run workflow"
4. Enter the version (must match `pyproject.toml`)

The version consistency check will still verify that the input version matches `pyproject.toml`.

## Troubleshooting

### Tag already exists

If the tag already exists, the Version Tag Creator workflow will skip creating it:

```
Tag core-v0.1.9 already exists, skipping
```

### Version consistency check fails

If the version check fails:

1. Check the version in `pyproject.toml`
2. Ensure the tag matches: `{package}-v{version}`
3. If needed, delete the incorrect tag and recreate it:
   ```bash
   git tag -d core-v0.1.9
   git push origin :refs/tags/core-v0.1.9
   ```

### Multiple packages updated at once

If you update multiple package versions in a single commit, the Version Tag Creator will:

1. Detect all version changes
2. Create tags for each package
3. Each package's publish workflow will run independently

## Best Practices

1. **Always update pyproject.toml first** - Don't manually create tags without updating the version
2. **One package per commit** - For clarity, update one package version per commit
3. **Use semantic versioning** - Follow semver conventions (MAJOR.MINOR.PATCH)
4. **Test before releasing** - Run tests locally before pushing version updates
5. **Monitor the workflows** - Check GitHub Actions to ensure successful publishing

## Dependencies Between Packages

Some packages depend on others (e.g., `agent` depends on `computer` and `som`). The `agent` package workflow automatically updates dependency versions to the latest available on PyPI.

When releasing dependent packages:

1. Release base packages first (e.g., `core`, `computer`, `som`)
2. Wait for them to be published to PyPI
3. Then release dependent packages (e.g., `agent`, `computer-server`, `mcp-server`)

The agent package workflow will automatically fetch and pin the latest versions of its dependencies.

## Helper Scripts

The release automation relies on two Python helper scripts located in `.github/scripts/`:

### detect_version_changes.py

Used by the Version Tag Creator workflow to:
- Compare versions between commits
- Detect which packages have version changes
- Output the appropriate git tags to create

**Usage:**
```bash
python .github/scripts/detect_version_changes.py
```

This script is automatically called by the workflow but can be run locally for testing.

### get_pyproject_version.py

Used by the publish workflow to verify version consistency.

**Usage:**
```bash
python .github/scripts/get_pyproject_version.py <pyproject_path> <expected_version>
```

**Example:**
```bash
python .github/scripts/get_pyproject_version.py libs/python/core/pyproject.toml 0.1.9
```

**Exit codes:**
- `0` - Versions match (success)
- `1` - Versions don't match or error occurred

This script ensures that the version in `pyproject.toml` matches what you're trying to publish.
