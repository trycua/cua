# Getting Started

## Project Structure

The project is organized as a monorepo with these main packages:

- `libs/core/` - Base package with telemetry support
- `libs/computer/` - Computer-use interface (CUI) library
- `libs/agent/` - AI agent library with multi-provider support
- `libs/som/` - Set-of-Mark parser
- `libs/computer-server/` - Server component for VM
- `libs/lume/` - Lume CLI

These packages are part of a uv workspace which manages a shared virtual environment and dependencies.

## Local Development Setup

1. Install Lume CLI:

   ```bash
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
   ```

2. Clone the repository:

   ```bash
   git clone https://github.com/trycua/cua.git
   cd cua
   ```

3. Create a `.env.local` file in the root directory with your API keys:

   ```bash
   # Required for Anthropic provider
   ANTHROPIC_API_KEY=your_anthropic_key_here

   # Required for OpenAI provider
   OPENAI_API_KEY=your_openai_key_here
   ```

4. Install Node.js dependencies for Prettier and other scripts:

   ```bash
   # Install pnpm if you don't have it
   npm install -g pnpm

   # Install all JS/TS dependencies
   pnpm install
   ```

5. Install Python dependencies and workspace packages:

   ```bash
   # First install uv if you don't have it
   pip install uv

    # Then install all Python dependencies
   uv sync
   ```

6. Open the workspace in VSCode or Cursor:

   ```bash
   # For Cua Python development
   code .vscode/py.code-workspace

   # For Lume (Swift) development
   code .vscode/lume.code-workspace
   ```

7. Install Pre-commit hooks:

   This ensures code formatting and validation run automatically on each commit.

   ```bash
   uv run pre-commit install
   ```

Using the workspace file is strongly recommended as it:

- Sets up correct Python environments for each package
- Configures proper import paths
- Enables debugging configurations
- Maintains consistent settings across packages

## Lume Development

Refer to the [Lume README](./libs/lume/Development.md) for instructions on how to develop the Lume CLI.

## Python Development

### Setup

Install all of workspace dependencies with a single command:

```bash
uv sync
```

This installs all dependencies in the virtual environment `.venv`.

Each Cua package is installed in editable mode, which means changes to the source code are immediately reflected in the installed package.

The `.venv` environment is also configured as the default VS Code Python interpreter in `.vscode/settings.json`.

### Running Python Scripts

To run Python scripts in the workspace, use the `uv run` command:

```bash
uv run python examples/agent_examples.py
```

Or activate the virtual environment manually:

```bash
source .venv/bin/activate
python examples/agent_examples.py
```

## Running Examples

The Python workspace includes launch configurations for all packages:

- "Run Computer Examples" - Runs computer examples
- "Run Agent Examples" - Runs agent examples
- "SOM" configurations - Various settings for running SOM

To run examples from VSCode / Cursor:

1. Press F5 or use the Run/Debug view
2. Select the desired configuration

The workspace also includes compound launch configurations:

- "Run Computer Examples + Server" - Runs both the Computer Examples and Server simultaneously

## Code Formatting Standards

The Cua project follows strict code formatting standards to ensure consistency across all packages.

### Python Code Formatting

#### Tools

The project uses the following tools for code formatting and linting:

- **[Black](https://black.readthedocs.io/)**: Code formatter
- **[isort](https://pycqa.github.io/isort/)**: Import sorter
- **[Ruff](https://beta.ruff.rs/docs/)**: Fast linter and formatter
- **[MyPy](https://mypy.readthedocs.io/)**: Static type checker

These tools are automatically installed when you set up the development environment.

#### Configuration

The formatting configuration is defined in the root `pyproject.toml` file:

```toml
[tool.black]
line-length = 100
target-version = ["py311"]

[tool.ruff]
fix = true
line-length = 100
target-version = "py311"

[tool.ruff.lint]
select = ["E", "F", "B", "I"]
ignore = [
    "E501", "E402", "I001", "I002", "B007", "B023", "B024", "B027", "B028",
    "B904", "B905", "E711", "E712", "E722", "E731", "F401", "F403", "F405",
    "F811", "F821", "F841"
]
fix = true

[tool.ruff.format]
docstring-code-format = true

[tool.mypy]
check_untyped_defs = true
disallow_untyped_defs = true
ignore_missing_imports = true
python_version = "3.11"
show_error_codes = true
strict = true
warn_return_any = true
warn_unused_ignores = false

[tool.isort]
profile = "black"
```

#### Key Formatting Rules

- **Line Length**: Maximum of 100 characters
- **Python Version**: Code should be compatible with Python 3.11+
- **Imports**: Automatically sorted (using Ruff's "I" rule)
- **Type Hints**: Required for all function definitions (strict mypy mode)

#### IDE Integration

The repository includes VSCode workspace configurations that enable automatic formatting. When you open the workspace files (as recommended in the setup instructions), the correct formatting settings are automatically applied.

##### Python-specific settings

These are configured in `.vscode/settings.json`:

```json
{
  "python.defaultInterpreterPath": "${workspaceFolder}/.venv/bin/python",
  "editor.formatOnSave": true,
  "editor.codeActionsOnSave": {
    "source.organizeImports": "explicit",
    "source.fixAll": "explicit"
  },
  "[python]": {
    "editor.defaultFormatter": "ms-python.black-formatter"
  },
  "python.formatting.provider": "black",
  "ruff.configuration": "${workspaceFolder}/pyproject.toml",
  "mypy-type-checker.args": ["--config-file", "${workspaceFolder}/pyproject.toml"],
  "mypy-type-checker.path": ["${workspaceFolder}"]
}
```

##### **JS/TS-specific settings**

```json
"[javascript][typescript][typescriptreact][javascriptreact]": {
  "editor.defaultFormatter": "esbenp.prettier-vscode"
}
```

- Ensures Prettier is used for all JS/TS files for consistent formatting.

Recommended VS Code Extensions

- **Black Formatter** – `ms-python.black-formatter`
- **Ruff** – `charliermarsh.ruff`
- **Pylance** – `ms-python.vscode-pylance`
- **isort** – `ms-python.isort`
- **Prettier** – `esbenp.prettier-vscode`
- **Mypy Type Checker** – `ms-python.mypy-type-checker`

> VSCode will automatically suggest installing the recommended extensions when you open the workspace.

#### Manual Formatting

To manually format code:

```bash
# Format all Python files using Black
uv run black .

# Sort imports using isort
uv run isort .

# Run Ruff linter with auto-fix
uv run ruff check .

# Run type checking with MyPy
uv run mypy .
```

#### Pre-commit Validation

Before submitting a pull request, ensure your code passes all formatting checks:

**Option 1: Run all hooks via pre-commit (all in a single command)**

```bash
# Run hooks on staged files (recommended for quick checks)
uv run pre-commit run
```

- Automatically runs Black, Ruff, isort, Mypy, Prettier, and any other configured hooks.

**Option 2: Run individual tools manually**

```bash
# Python checks
uv run black --check .
uv run isort --check .
uv run ruff check .
uv run mypy .

# JavaScript/TypeScript checks
uv run prettier --check "**/*.{ts,tsx,js,jsx,json,md,yaml,yml}"

# TypeScript typecheck
node ./scripts/typescript-typecheck.js
```

### JavaScript / TypeScript Formatting (Prettier)

The project uses **Prettier** to ensure consistent formatting across all JS/TS/JSON/Markdown/YAML files.

#### Installation

All Node.js dependencies are managed via `pnpm`. Make sure you have run:

```bash
# Install pnpm if you don't have it
npm install -g pnpm

# Install project dependencies
pnpm install
```

This installs Prettier and other JS/TS dependencies defined in `package.json`.

#### Usage

- **Check formatting** (without making changes):

```bash
pnpm prettier:check
```

- **Automatically format files**:

```bash
pnpm prettier:format
```

#### Type Checking (TypeScript)

- Run the TypeScript type checker:

```bash
node ./scripts/typescript-typecheck.js
```

#### VSCode Integration

- The workspace config ensures Prettier is used automatically for JS/TS/JSON/Markdown/YAML files.
- Recommended extension: Prettier – Code Formatter
- Ensure `editor.formatOnSave` is enabled in VSCode for automatic formatting.

### Swift Code (Lume)

For Swift code in the `libs/lume` directory:

- Follow the [Swift API Design Guidelines](https://www.swift.org/documentation/api-design-guidelines/)
- Use SwiftFormat for consistent formatting
- Code will be automatically formatted on save when using the lume workspace

## Releasing Packages

Cua uses an automated GitHub Actions workflow to bump package versions and publish to PyPI.

> **Note:** The main branch is currently not protected. If branch protection is enabled in the future, the github-actions bot must be added to the bypass list for these workflows to commit directly.

### Version Bump and Publish Workflow

All packages are managed through a single consolidated workflow: [Bump Version](https://github.com/trycua/cua/actions/workflows/bump-version.yml)

This workflow handles both version bumping and PyPI publishing in a single execution.

**Supported packages:**

- cua-agent
- cua-computer
- cua-computer-server
- cua-core
- cua-mcp-server
- cua-som
- pylume

**How to use:**

1. Navigate to the [Bump Version workflow](https://github.com/trycua/cua/actions/workflows/bump-version.yml)
2. Click the "Run workflow" button in the GitHub UI
3. Select the **service/package** you want to bump from the first dropdown
4. Select the **bump type** (patch/minor/major) from the second dropdown
5. Click "Run workflow" to start the process
6. The workflow will automatically:
   - Bump the version in pyproject.toml
   - Commit changes and push to main
   - Create and push a package-specific tag (e.g., core-v0.1.9)
   - Publish the package to PyPI
   - Create a GitHub release

### Rolling Back a Version Bump and Release

If you need to revert a version bump and release, follow these steps:

> **Important:** Rolling back only reverts the git commit and tag. If the package was already published to PyPI, you cannot delete or replace that version. You will need to publish a new version instead.

**Step 1: Find the version bump commit**

```bash
# List recent commits
git log --oneline | grep "Bump"

# Example output:
# a1b2c3d Bump cua-core to v0.1.9
```

**Step 2: Revert the commit**

```bash
# Revert the specific commit
git revert <commit-hash>

# Example:
# git revert a1b2c3d
```

**Step 3: Delete the git tag**

```bash
# List tags to find the version tag
git tag -l

# Delete the tag locally (use the correct package-specific format)
git tag -d core-v0.1.9

# Delete the tag remotely
git push origin :refs/tags/core-v0.1.9
```

**Step 4: Push the revert**

```bash
git push origin main
```

**Per-package tag patterns:**

Each package uses its own tag format defined in `.bumpversion.cfg`:

- **cua-core**: `core-v{version}` (e.g., `core-v0.1.9`)
- **cua-computer**: `computer-v{version}` (e.g., `computer-v0.4.7`)
- **cua-agent**: `agent-v{version}` (e.g., `agent-v0.4.35`)
- **cua-som**: `som-v{version}` (e.g., `som-v0.1.3`)
- **pylume**: `pylume-v{version}` (e.g., `pylume-v0.2.1`)
- **cua-computer-server**: `computer-server-v{version}` (e.g., `computer-server-v0.1.27`)
- **cua-mcp-server**: `mcp-server-v{version}` (e.g., `mcp-server-v0.1.14`)

### Local Testing (Advanced)

The Makefile targets are kept for local testing only:

```bash
# Test version bump locally (dry run)
make dry-run-patch-core

# View current versions
make show-versions
```

**Note:** For production releases, always use the GitHub Actions workflows above instead of running Makefile commands directly.
