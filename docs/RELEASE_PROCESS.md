# Python Package Release Process

## Overview

The Python SDK packages use `pyproject.toml` (or `__init__.py` for pylume) as the **source of truth** for version management. When you update a package version and push to the `main` branch, the release process is automated.

**NEW**: We now support automated version bumping using `bump2version` to simplify the release process and reduce manual errors.

## Quick Start with bump2version (Recommended)

### Installation

`bump2version` is included as a dev dependency in the root `pyproject.toml`:

```bash
# Install dev dependencies (includes bump2version)
pip install -e ".[dev]"
# or with pdm
pdm install
```

### Usage

#### Option 1: Using the Makefile (Easiest!)

We provide a `Makefile` in the root directory with convenient targets for all packages:

```bash
# Show all available targets
make help

# Show current versions of all packages
make show-versions

# Bump a specific package
make bump-patch-core           # cua-core: 0.1.8 → 0.1.9
make bump-minor-computer       # cua-computer: 0.4.0 → 0.5.0
make bump-major-agent          # cua-agent: 0.4.0 → 1.0.0

# Dry run (test without making changes)
make dry-run-patch-core

# Push the changes
git push origin main
```

**Available packages:** `core`, `pylume`, `computer`, `som`, `agent`, `computer-server`, `mcp-server`

#### Option 2: Using bump2version directly

If you prefer to run `bump2version` directly:

```bash
# Navigate to the package directory
cd libs/python/core

# Bump the patch version (e.g., 0.1.8 → 0.1.9)
bump2version patch

# Or bump minor version (e.g., 0.1.9 → 0.2.0)
bump2version minor

# Or bump major version (e.g., 0.2.0 → 1.0.0)
bump2version major

# Push the commit (tags are created automatically by CI)
git push origin main
```

**What happens automatically:**
1. Updates the version in the appropriate file (`pyproject.toml` or `__init__.py`)
2. Creates a commit with the message "Bump {package-name} to v{new_version}"
3. After you push, the CI pipeline handles tag creation and PyPI publishing

### Configuration Files

Each package has a `.bumpversion.cfg` file that configures version bumping:

```ini
[bumpversion]
current_version = 0.1.8
commit = True
tag = False  # Tags are created by CI, not locally
message = Bump cua-core to v{new_version}

[bumpversion:file:pyproject.toml]
search = version = "{current_version}"
replace = version = "{new_version}"
```

**Note**: `tag = False` because our CI pipeline creates tags automatically. This prevents duplicate tags.

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

### Method 1: Using the Makefile (Easiest!)

Example: Releasing cua-core v0.1.9

```bash
# From the root directory
make bump-patch-core

# Push the commit
git push origin main
```

**What happens automatically:**
1. The Makefile runs `bump2version` in the package directory
2. `bump2version` updates `pyproject.toml` and creates a commit
3. The Version Tag Creator workflow detects the version change
4. Creates and pushes tag `core-v0.1.9`
5. The `pypi-publish-core.yml` workflow is triggered
6. Package is built and published to PyPI
7. GitHub release is created with release notes

**Monitor the release:**
- Check the Actions tab in GitHub to see the workflow progress
- Once complete, verify the package on PyPI: https://pypi.org/project/cua-core/

### Method 2: Using bump2version directly

Example: Releasing cua-core v0.1.9

```bash
# Navigate to package directory
cd libs/python/core

# Bump to next patch version (0.1.8 → 0.1.9)
bump2version patch

# Push the commit
git push origin main
```

**What happens automatically:**
1. `bump2version` updates `pyproject.toml` and creates a commit
2. The Version Tag Creator workflow detects the version change
3. Creates and pushes tag `core-v0.1.9`
4. The `pypi-publish-core.yml` workflow is triggered
5. Package is built and published to PyPI
6. GitHub release is created with release notes

**Monitor the release:**
- Check the Actions tab in GitHub to see the workflow progress
- Once complete, verify the package on PyPI: https://pypi.org/project/cua-core/

### Method 3: Manual Version Update

If you prefer to manually edit the version file:

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

1. **Use bump2version** - The automated tool reduces errors and ensures consistency
2. **Always update version files first** - Don't manually create tags without updating the version
3. **One package per commit** - For clarity, update one package version per commit
4. **Use semantic versioning** - Follow semver conventions (MAJOR.MINOR.PATCH)
   - **patch**: Bug fixes, minor changes (0.1.8 → 0.1.9)
   - **minor**: New features, backwards compatible (0.1.9 → 0.2.0)
   - **major**: Breaking changes (0.2.0 → 1.0.0)
5. **Test before releasing** - Run tests locally before pushing version updates
6. **Monitor the workflows** - Check GitHub Actions to ensure successful publishing
7. **Keep .bumpversion.cfg in sync** - If you manually update versions, also update the `.bumpversion.cfg` file

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

## bump2version Tips & Troubleshooting

### Dry Run

Test version bumping without making changes:

```bash
cd libs/python/core
bump2version --dry-run --verbose patch
```

### Custom Version

If you need to set a specific version (not following semver):

```bash
cd libs/python/core
bump2version --new-version 0.2.5 patch
```

### Configuration Out of Sync

If `.bumpversion.cfg` shows the wrong `current_version`:

1. Check the actual version in the version file (`pyproject.toml` or `__init__.py`)
2. Update `.bumpversion.cfg` to match:
   ```ini
   [bumpversion]
   current_version = 0.1.8  # Update this to match pyproject.toml
   ```
3. Run `bump2version` again

### Commit Already Exists

If you've already committed a version change manually:

1. Update `.bumpversion.cfg` to reflect the new version:
   ```bash
   cd libs/python/core
   # Edit .bumpversion.cfg and update current_version
   git add .bumpversion.cfg
   git commit --amend --no-edit
   git push origin main
   ```

### Special Case: pylume

The `pylume` package uses `__init__.py` for versioning instead of `pyproject.toml`:

```bash
cd libs/python/pylume
bump2version patch  # Updates pylume/__init__.py
git push origin main
```

The `.bumpversion.cfg` is already configured to handle this correctly.
