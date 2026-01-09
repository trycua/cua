# Documentation Generators

This directory contains auto-documentation generators for the CUA libraries. These generators ensure that documentation stays synchronized with source code.

## Architecture

```
scripts/docs-generators/
├── config.json         # Central configuration for all generators
├── runner.ts           # Main orchestrator that runs generators
├── lume.ts             # Lume (Swift) generator
├── cua-cli.ts          # CUA CLI (TypeScript) generator (planned)
├── mcp-server.ts       # MCP Server (Python) generator (planned)
├── python-sdk.ts       # Python SDK generator (planned)
├── typescript-sdk.ts   # TypeScript SDK generator (planned)
└── README.md           # This file
```

## Quick Start

```bash
# Generate all documentation
npm run docs:generate

# Check for drift (CI mode)
npm run docs:check

# List all configured generators
npm run docs:list

# Generate specific library docs
npx tsx scripts/docs-generators/runner.ts --library lume
```

## How It Works

1. **config.json** defines all libraries that need documentation generation:
   - Source paths to watch
   - Output paths for generated docs
   - Generator scripts to run
   - Build commands (if needed)

2. **runner.ts** orchestrates generation:
   - Reads config.json
   - Detects changed files (in CI)
   - Runs appropriate generators
   - Reports success/failure

3. **Library-specific generators** (e.g., `lume.ts`):
   - Extract metadata from source code
   - Generate MDX documentation
   - Handle language-specific concerns

## Adding a New Library

1. **Add configuration** to `config.json`:
   ```json
   {
     "generators": {
       "my-library": {
         "name": "My Library",
         "language": "python",
         "sourcePath": "libs/my-library/src",
         "docsOutputPath": "docs/content/docs/cua/reference/my-library",
         "generatorScript": "scripts/docs-generators/my-library.ts",
         "watchPaths": ["libs/my-library/src/**/*.py"],
         "enabled": true
       }
     }
   }
   ```

2. **Create generator script** (e.g., `my-library.ts`):
   ```typescript
   #!/usr/bin/env npx tsx
   // Follow the pattern in lume.ts
   ```

3. **Update CI workflow** if needed (paths are usually auto-detected from config)

## Extraction Methods

Different libraries use different extraction methods:

| Language | Method | Description |
|----------|--------|-------------|
| Swift | `dump-docs` | Built-in command that outputs JSON |
| TypeScript | `yargs-parse` | Parse yargs CLI definitions |
| Python | `argparse-introspection` | Introspect argparse commands |
| Python | `sphinx-autodoc` | Generate from docstrings |
| TypeScript | `typedoc` | Generate from TSDoc comments |

## CI Integration

The `.github/workflows/docs-sync-check.yml` workflow:

1. Detects which libraries have changes
2. Only runs generators for changed libraries
3. Fails if documentation is out of sync
4. Provides helpful fix instructions

## Generator Status

| Generator | Status | Notes |
|-----------|--------|-------|
| lume | ✅ Implemented | CLI + HTTP API |
| cua-cli | ⏸️ Planned | Needs yargs introspection |
| mcp-server | ⏸️ Planned | Needs MCP tool extraction |
| computer-sdk-python | ⏸️ Planned | Needs Sphinx/pydoc |
| computer-sdk-typescript | ⏸️ Planned | Needs TypeDoc |
| agent-sdk-python | ⏸️ Planned | Needs Sphinx/pydoc |
| agent-sdk-typescript | ⏸️ Planned | Needs TypeDoc |

## Development

To test generators locally:

```bash
# Run with verbose output
DEBUG=1 npx tsx scripts/docs-generators/runner.ts

# Generate only one library
npx tsx scripts/docs-generators/runner.ts --library lume

# Check without modifying files
npx tsx scripts/docs-generators/runner.ts --check
```
