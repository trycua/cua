# Development Guide

This guide covers setting up and developing the Cua monorepo.

## Project Structure

The project is organized as a monorepo with these main packages:

**Python Packages** (located in `libs/python/`):

- `core/` - Base package with telemetry support
- `computer/` - Computer-use interface (CUI) library
- `agent/` - AI agent library with multi-provider support
- `som/` - Set-of-Mark parser
- `computer-server/` - Server component for VM
- `mcp-server/` - MCP server implementation
- `bench-ui/` - Benchmark UI utilities

**Other Packages**:

- `libs/lume/` - Lume CLI (Swift)
- `libs/typescript/` - TypeScript packages including `cua-cli`
- `libs/cuabot/` - CuaBot multi-agent computer-use sandbox CLI

All Python packages are part of a [uv workspace](https://docs.astral.sh/uv/concepts/projects/workspaces/) which manages a shared virtual environment and dependencies.

## Quick Start

1. **Install Lume CLI** (required for local VM management):

   ```bash
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
   ```

2. **Clone the repository**:

   ```bash
   git clone https://github.com/trycua/cua.git
   cd cua
   ```

3. **Create `.env.local`** in the root directory with your API keys:

   ```bash
   ANTHROPIC_API_KEY=your_anthropic_key_here
   OPENAI_API_KEY=your_openai_key_here
   ```

4. **Install Node.js dependencies**:

   ```bash
   npm install -g pnpm  # if not already installed
   pnpm install
   ```

5. **Install Python dependencies**:

   ```bash
   pip install uv  # if not already installed
   uv sync
   ```

6. **Open workspace in VS Code/Cursor**:

   ```bash
   # For Python development
   code .vscode/py.code-workspace

   # For Lume (Swift) development
   code .vscode/lume.code-workspace
   ```

7. **Install pre-commit hooks**:
   ```bash
   uv run pre-commit install
   ```

<details>
<summary>Why use workspace files?</summary>

Using the workspace file is strongly recommended as it:

- Sets up correct Python environments for each package
- Configures proper import paths
- Enables debugging configurations
- Maintains consistent settings across packages

</details>

## Python Development

### Requirements

- **Python 3.12+** (see [`pyproject.toml`](./pyproject.toml) for exact requirements)
- **uv** - Python package manager
- **pnpm** - Node.js package manager

### Setup

Install all workspace dependencies with a single command:

```bash
uv sync
```

This installs all dependencies in the virtual environment `.venv`. Each Cua package is installed in editable mode, so changes to source code are immediately reflected.

The `.venv` environment is configured as the default VS Code Python interpreter in [`.vscode/settings.json`](.vscode/settings.json).

### Running Python Scripts

Use `uv run` to execute Python scripts:

```bash
uv run python examples/agent_examples.py
```

Alternatively, activate the virtual environment manually:

```bash
source .venv/bin/activate  # On Windows: .venv\Scripts\activate
python examples/agent_examples.py
```

### Running Examples

The Python workspace includes launch configurations for running examples:

- "Run Computer Examples" - Runs computer examples
- "Run Agent Examples" - Runs agent examples
- "SOM" configurations - Various settings for running SOM
- "Run Computer Examples + Server" - Runs both Computer Examples and Server simultaneously

To run examples from VS Code/Cursor:

1. Press F5 or use the Run/Debug view
2. Select the desired configuration

## Code Formatting

The Cua project follows strict code formatting standards to ensure consistency across all packages.

### Python Formatting

#### Tools

- **[Black](https://black.readthedocs.io/)** - Code formatter
- **[isort](https://pycqa.github.io/isort/)** - Import sorter
- **[Ruff](https://beta.ruff.rs/docs/)** - Fast linter and formatter
- **[MyPy](https://mypy.readthedocs.io/)** - Static type checker (configured but not enforced in pre-commit)

All tools are automatically installed when you run `uv sync`.

#### Configuration

Formatting configuration is defined in [`pyproject.toml`](./pyproject.toml). See the `[tool.black]`, `[tool.ruff]`, `[tool.mypy]`, and `[tool.isort]` sections for all settings.

#### Key Rules

- **Line Length**: Maximum of 100 characters
- **Python Version**: Code must be compatible with Python 3.12+
- **Imports**: Automatically sorted (using Ruff's "I" rule)
- **Type Hints**: Required for all function definitions (strict mypy mode)

<details>
<summary>IDE configuration details</summary>

##### Python-specific settings

Python-specific IDE settings are configured in [`.vscode/settings.json`](.vscode/settings.json), including:

- Python interpreter path
- Format on save
- Code actions on save
- Black formatter configuration
- Ruff and MyPy integration

##### JS/TS-specific settings

JavaScript/TypeScript formatting settings are also in [`.vscode/settings.json`](.vscode/settings.json), ensuring Prettier is used for all JS/TS files.

##### Recommended VS Code Extensions

- **Black Formatter** – `ms-python.black-formatter`
- **Ruff** – `charliermarsh.ruff`
- **Pylance** – `ms-python.vscode-pylance`
- **isort** – `ms-python.isort`
- **Prettier** – `esbenp.prettier-vscode`
- **Mypy Type Checker** – `ms-python.mypy-type-checker`

> VS Code will automatically suggest installing the recommended extensions when you open the workspace.

</details>

<details>
<summary>Manual formatting commands</summary>

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

</details>

#### Pre-commit Validation

Before submitting a pull request, ensure your code passes all formatting checks.

**Recommended: Run all hooks via pre-commit**

```bash
uv run pre-commit run
```

This automatically runs Black, Ruff, isort, Prettier, TypeScript type checking, and other configured hooks. See [`.pre-commit-config.yaml`](.pre-commit-config.yaml) for the complete list.

> **Note:** MyPy is currently disabled in pre-commit hooks due to untyped codebase, but it's still configured and can be run manually.

<details>
<summary>Run individual tools manually</summary>

```bash
# Python checks
uv run black --check .
uv run isort --check .
uv run ruff check .
uv run mypy .

# JavaScript/TypeScript checks
pnpm prettier:check
node ./scripts/typescript-typecheck.js
```

</details>

### JavaScript / TypeScript Formatting

The project uses **Prettier** to ensure consistent formatting across all JS/TS/JSON/Markdown/YAML files.

#### Installation

All Node.js dependencies are managed via `pnpm`:

```bash
npm install -g pnpm  # if not already installed
pnpm install
```

#### Usage

- **Check formatting** (without making changes):

  ```bash
  pnpm prettier:check
  ```

- **Automatically format files**:

  ```bash
  pnpm prettier:format
  ```

- **TypeScript type checking**:
  ```bash
  node ./scripts/typescript-typecheck.js
  ```

#### VS Code Integration

The workspace config ensures Prettier is used automatically for JS/TS/JSON/Markdown/YAML files. Ensure `editor.formatOnSave` is enabled in VS Code.

### Swift Code (Lume)

For Swift code in the `libs/lume` directory:

- Follow the [Swift API Design Guidelines](https://www.swift.org/documentation/api-design-guidelines/)
- Use SwiftFormat for consistent formatting
- Code will be automatically formatted on save when using the lume workspace

Refer to [`libs/lume/Development.md`](./libs/lume/Development.md) for detailed Lume development instructions.

## Releasing Packages

Cua uses an automated GitHub Actions workflow to bump package versions and publish to PyPI/NPM.

> **Note:** The main branch is currently not protected. If branch protection is enabled in the future, the github-actions bot must be added to the bypass list for these workflows to commit directly.

### Version Bump & Publish Workflow

All packages are managed through a single consolidated workflow: [Bump Version & Publish](https://github.com/trycua/cua/actions/workflows/release-bump-version.yml)

**Supported packages:**

**Python (PyPI):**

- `pypi/agent` - AI agent library
- `pypi/bench` - Benchmark toolkit for computer-use RL environments
- `pypi/computer` - Computer-use interface library
- `pypi/computer-server` - Server component for VM
- `pypi/core` - Base package with telemetry
- `pypi/mcp-server` - MCP server implementation
- `pypi/som` - Set-of-Mark parser

**JavaScript/TypeScript (NPM):**

- `npm/cli` - Cua command-line interface
- `npm/computer` - Computer client for TypeScript
- `npm/core` - Core TypeScript utilities
- `npm/cuabot` - Multi-agent computer-use sandbox

**Docker:**

- `docker/cuabot` - CuaBot container image

**How to use:**

1. Navigate to the [Bump Version & Publish workflow](https://github.com/trycua/cua/actions/workflows/release-bump-version.yml)
2. Click the "Run workflow" button in the GitHub UI
3. Select the **service/package** from the dropdown (e.g., `pypi/computer` or `npm/cli`)
4. Select the **bump type** (patch/minor/major) from the second dropdown
5. Click "Run workflow" to start the process

**What happens automatically:**

1. Version is bumped in the package configuration file
2. Changes are committed and pushed to main
3. Package is automatically published to PyPI or NPM
4. For `npm/cli`: Binaries are built and a GitHub release is created

> **Note:** For `pypi/computer`, the workflow also automatically bumps `pypi/agent` to maintain version compatibility.

<details>
<summary>Local Testing (Advanced)</summary>

The Makefile provides utility targets for local testing only:

```bash
# Test version bump locally (dry run)
make dry-run-patch-core

# View current versions
make show-versions
```

**Note:** For production releases, always use the GitHub Actions workflows above instead of running Makefile commands directly.

</details>

---

<details>
<summary>Per-package tag patterns</summary>

Each package uses its own tag format defined in `.bumpversion.cfg`:

- **cua-agent**: `agent-v{version}` (e.g., `agent-v0.4.35`)
- **cua-bench**: `bench-v{version}` (e.g., `bench-v0.1.0`)
- **cua-computer**: `computer-v{version}` (e.g., `computer-v0.4.7`)
- **cua-computer-server**: `computer-server-v{version}` (e.g., `computer-server-v0.1.27`)
- **cua-core**: `core-v{version}` (e.g., `core-v0.1.9`)
- **cua-mcp-server**: `mcp-server-v{version}` (e.g., `mcp-server-v0.1.14`)
- **cua-som**: `som-v{version}` (e.g., `som-v0.1.3`)
- **cuabot**: `cuabot-v{version}` (e.g., `cuabot-v1.0.0`)
- **cuabot (docker)**: `docker-cuabot-v{version}` (e.g., `docker-cuabot-v1.0.0`)

</details>

</details>
