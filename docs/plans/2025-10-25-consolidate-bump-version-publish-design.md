# Consolidate Bump Version and Publish Workflows

**Date:** 2025-10-25
**Status:** Approved

## Overview

Consolidate the logic from 7 individual PyPI publish workflows into the bump-version.yml workflow. This creates a single entry point for versioning and publishing Python packages while maintaining the reusable publish workflow pattern.

## Current State

### Structure
- **bump-version.yml**: Handles version bumping with bump2version for 7 Python packages
- **7 individual publish workflows**: Each package has its own workflow triggered by tags
  - pypi-publish-agent.yml
  - pypi-publish-computer.yml
  - pypi-publish-computer-server.yml
  - pypi-publish-core.yml
  - pypi-publish-mcp-server.yml
  - pypi-publish-som.yml
  - pypi-publish-pylume.yml
- **pypi-reusable-publish.yml**: Shared publishing logic

### Workflow Flow
1. User manually runs bump-version.yml
2. Version gets bumped in pyproject.toml
3. Changes committed and pushed
4. User manually creates tag
5. Tag triggers individual publish workflow
6. Individual workflow calls reusable publish workflow

## Design Goals

1. **Single workflow execution**: Bump and publish in one workflow run
2. **Reduce maintenance burden**: Fewer workflow files to maintain
3. **Maintain reusable logic**: Keep pypi-reusable-publish.yml intact
4. **Simplify dependency management**: Remove automatic dependency updates to PyPI latest versions

## New Architecture

### Workflow Structure

**bump-version.yml** will execute in two jobs:

1. **bump-version job**:
   - Accept service selection (existing 7 Python packages)
   - Run bump2version to update pyproject.toml
   - Extract new version
   - Map service to package metadata
   - Commit and push changes
   - Create and push package-specific tag
   - Output package metadata for publish job

2. **publish job**:
   - Call pypi-reusable-publish.yml with package-specific parameters
   - Publish to PyPI
   - Create GitHub release

### Package Mapping

| Service | package_name | package_dir | is_lume_package | base_package_name |
|---------|-------------|-------------|-----------------|-------------------|
| cua-agent | agent | libs/python/agent | false | cua-agent |
| cua-computer | computer | libs/python/computer | false | cua-computer |
| cua-computer-server | computer-server | libs/python/computer-server | false | cua-computer-server |
| cua-core | core | libs/python/core | false | cua-core |
| cua-mcp-server | mcp-server | libs/python/mcp-server | false | cua-mcp-server |
| cua-som | som | libs/python/som | false | cua-som |
| pylume | pylume | libs/python/pylume | **true** | pylume |

### Tag Naming Convention

Follows existing pattern: `{service}-v{version}`

Examples:
- `agent-v0.2.0`
- `computer-v1.5.3`
- `pylume-v2.1.0`

## Implementation Changes

### Files to Modify

**bump-version.yml**:
- Add version extraction step
- Add package metadata mapping step
- Add tag creation and push step
- Add publish job that calls pypi-reusable-publish.yml

### Files to Delete

- .github/workflows/pypi-publish-agent.yml
- .github/workflows/pypi-publish-computer.yml
- .github/workflows/pypi-publish-computer-server.yml
- .github/workflows/pypi-publish-core.yml
- .github/workflows/pypi-publish-mcp-server.yml
- .github/workflows/pypi-publish-som.yml
- .github/workflows/pypi-publish-pylume.yml

### Files to Keep

- .github/workflows/pypi-reusable-publish.yml (no changes)

## Breaking Changes

### Dependency Updates Removed

**Previous behavior**: Some packages (agent, computer, mcp-server) automatically updated their internal dependencies to latest PyPI versions before publishing.

**New behavior**: Dependencies remain at versions specified in pyproject.toml. Manual updates required if desired.

**Rationale**: Simplifies workflow, gives explicit control over dependency versions, avoids unexpected version changes.

## Benefits

1. **Single entry point**: One workflow for bump + publish
2. **Fewer files**: 8 files reduced to 2 (bump-version.yml + reusable)
3. **Consistent process**: All packages follow same path
4. **Faster iteration**: Less duplication to maintain
5. **Explicit dependencies**: No automatic version updates

## Testing Strategy

1. Test bump-version workflow with each package type
2. Verify tag creation and format
3. Verify publish job receives correct parameters
4. Verify PyPI publication succeeds
5. Verify GitHub release creation
6. Special validation for pylume (is_lume_package: true)

## Rollout Plan

1. Create feature branch with isolated worktree
2. Implement changes to bump-version.yml
3. Test with one package (start with cua-core as it has no special dependencies)
4. Verify end-to-end: bump → commit → tag → publish → release
5. Delete individual publish workflows
6. Merge to main
