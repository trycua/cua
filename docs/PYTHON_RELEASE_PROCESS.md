# Python Package Release Process

## Overview

The Python SDK packages use an automated release process where `pyproject.toml` is the **single source of truth** for package versions. When you update a version in `pyproject.toml` and push to the `main` branch, the release process is automatically triggered.

## How It Works

### 1. Version Detection (Automated)

When changes to `libs/python/*/pyproject.toml` are pushed to `main`:

1. The **version-tag-creator** workflow (`.github/workflows/version-tag-creator.yml`) runs automatically
2. It reads the `version` field from each package's `pyproject.toml`
3. It compares the version with existing git tags
4. If the version is new, it creates a tag like `core-v0.1.8`, `agent-v0.4.0`, etc.
5. The tag is automatically pushed to the repository

### 2. Package Publishing (Triggered by Tags)

Once a tag is created:

1. The package-specific workflow (e.g., `pypi-publish-core.yml`) is triggered by the tag
2. The workflow reads the version from `pyproject.toml` (not from the tag)
3. It builds the package using PDM
4. It publishes to PyPI using the configured `PYPI_TOKEN`
5. It creates a GitHub release with auto-generated notes

## Release Workflow

### Standard Release Process

1. **Update the version** in the package's `pyproject.toml`:
   ```toml
   [project]
   name = "cua-core"
   version = "0.1.9"  # Updated from 0.1.8
   ```

2. **Update dependencies** if needed (in the same or other packages):
   ```toml
   dependencies = [
       "cua-core>=0.1.9,<0.2.0",  # Updated dependency version
   ]
   ```

3. **Commit and push** to `main`:
   ```bash
   git add libs/python/core/pyproject.toml
   git commit -m "Bump cua-core to v0.1.9"
   git push origin main
   ```

4. **Automated process** takes over:
   - Tag `core-v0.1.9` is created automatically
   - Package is built and published to PyPI
   - GitHub release is created

### Multi-Package Release

When releasing packages with dependencies (e.g., `agent` depends on `core`, `computer`, `som`):

1. **Update base packages first** (core, computer, som):
   ```bash
   # Update core
   vim libs/python/core/pyproject.toml  # Bump version to 0.1.9
   git add libs/python/core/pyproject.toml
   git commit -m "Bump cua-core to v0.1.9"
   git push origin main
   # Wait for release to complete
   ```

2. **Update dependent packages** with new dependency versions:
   ```toml
   # In libs/python/agent/pyproject.toml
   [project]
   version = "0.4.1"  # Bump agent version
   dependencies = [
       "cua-computer>=0.4.0,<0.5.0",
       "cua-core>=0.1.9,<0.2.0",  # Updated to use new core version
   ]
   ```

3. **Commit and push**:
   ```bash
   git add libs/python/agent/pyproject.toml
   git commit -m "Bump cua-agent to v0.4.1 with updated dependencies"
   git push origin main
   ```

## Package Structure

The monorepo contains these Python packages:

| Package | PyPI Name | Directory | Description |
|---------|-----------|-----------|-------------|
| core | `cua-core` | `libs/python/core` | Base package with telemetry |
| pylume | `pylume` | `libs/python/pylume` | Python bindings for lume (includes binary) |
| computer | `cua-computer` | `libs/python/computer` | Computer-use interface library |
| som | `cua-som` | `libs/python/som` | Set-of-Mark parser |
| agent | `cua-agent` | `libs/python/agent` | AI agent with multi-provider support |
| computer-server | `cua-computer-server` | `libs/python/computer-server` | FastAPI server for computer control |
| mcp-server | `cua-mcp-server` | `libs/python/mcp-server` | MCP integration |

### Dependency Graph

```
cua-core
  ├── pylume
  │   └── cua-computer
  │       ├── cua-agent
  │       ├── cua-computer-server
  │       └── cua-mcp-server
  └── cua-som
      └── cua-agent (optional)
```

## Manual Intervention

### Manual Tag Creation (Not Recommended)

If you need to manually create a tag (not recommended):

```bash
git tag core-v0.1.9
git push origin core-v0.1.9
```

**Note**: The version in `pyproject.toml` will be used for the build, so the tag version must match!

### Manual Workflow Trigger

You can manually trigger a release from GitHub Actions UI:

1. Go to **Actions** → Select the package workflow (e.g., "Publish Core Package")
2. Click **Run workflow**
3. Enter the version (must match `pyproject.toml`)

### Skipping Automatic Releases

If you want to update `pyproject.toml` without triggering a release:

1. Use a branch other than `main`
2. Or add `[skip ci]` to your commit message (may not work for all scenarios)

## Version Constraints

### Semantic Versioning

Follow semantic versioning (SemVer):
- **MAJOR**: Breaking changes (e.g., `0.1.8` → `1.0.0`)
- **MINOR**: New features, backward compatible (e.g., `0.1.8` → `0.2.0`)
- **PATCH**: Bug fixes, backward compatible (e.g., `0.1.8` → `0.1.9`)

### Dependency Version Constraints

Use compatible release constraints:
```toml
dependencies = [
    "cua-core>=0.1.8,<0.2.0",  # Allow patches but not minor updates
]
```

## Troubleshooting

### Tag Already Exists

**Error**: `Tag core-v0.1.8 already exists`

**Solution**: You tried to push the same version twice. Bump the version in `pyproject.toml` to a new version.

### Version Mismatch

**Error**: Tag version doesn't match `pyproject.toml` version

**Solution**: The workflow will warn but use the `pyproject.toml` version. Make sure they match or let the automated workflow create tags.

### Failed PyPI Upload

**Error**: Version already exists on PyPI

**Solution**: You cannot overwrite a PyPI version. Bump to a new version.

### Dependency Not Available

**Error**: Package depends on a version not yet published

**Solution**: Wait for dependencies to publish first, then release dependent packages.

## Workflow Files

| Workflow | Purpose |
|----------|---------|
| `.github/workflows/version-tag-creator.yml` | Detects version changes and creates tags |
| `.github/workflows/pypi-reusable-publish.yml` | Reusable workflow for building and publishing |
| `.github/workflows/pypi-publish-*.yml` | Package-specific publish workflows |

## Best Practices

1. **Always update `pyproject.toml` first** - Don't create tags manually
2. **Test locally** before pushing to `main`:
   ```bash
   cd libs/python/core
   pdm build
   pdm install
   ```
3. **Follow dependency order** - Release base packages before dependent packages
4. **Use meaningful commit messages** - They appear in git history
5. **Version dependencies correctly** - Use compatible release constraints
6. **Monitor GitHub Actions** - Check that releases complete successfully

## Migration from Old Process

### Old Process (Tag-Based)
```bash
# Old way: Create tag first
git tag core-v0.1.9
git push origin core-v0.1.9
# Tag triggered workflow that modified pyproject.toml
```

### New Process (pyproject.toml-Based)
```bash
# New way: Update pyproject.toml first
vim libs/python/core/pyproject.toml  # Change version
git add libs/python/core/pyproject.toml
git commit -m "Bump cua-core to v0.1.9"
git push origin main
# Workflow detects change and creates tag automatically
```

## FAQ

**Q: Can I release multiple packages at once?**

A: Yes, but only if they don't depend on each other. Update multiple `pyproject.toml` files in a single commit and push. Each will get its own tag and release.

**Q: What if I need to rollback a release?**

A: You cannot delete PyPI versions. Instead, publish a new patch version with the fix.

**Q: How do I test the release process?**

A: Use a test PyPI instance or create a branch and test the tag creation workflow with `workflow_dispatch`.

**Q: What about prereleases (alpha, beta, rc)?**

A: Use version suffixes in `pyproject.toml`: `0.2.0a1`, `0.2.0b1`, `0.2.0rc1`
