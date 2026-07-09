# Contributing to Cua

We deeply appreciate your interest in contributing to Cua! Whether you're reporting bugs, suggesting enhancements, improving docs, or submitting pull requests, your contributions help improve the project for everyone.

## Reporting Bugs

If you've encountered a bug in the project, we encourage you to report it. Please follow these steps:

1. **Check the Issue Tracker**: Before submitting a new bug report, please check our issue tracker to see if the bug has already been reported.
2. **Create a New Issue**: If the bug hasn't been reported, create a new issue with:
   - A clear title and detailed description
   - Steps to reproduce the issue
   - Expected vs actual behavior
   - Your environment (macOS version, cua version)
   - Any relevant logs or error messages
3. **Label Your Issue**: Label your issue as a `bug` to help maintainers identify it quickly.

## Suggesting Enhancements

We're always looking for suggestions to make Cua better. If you have an idea:

1. **Check Existing Issues**: See if someone else has already suggested something similar.
2. **Create a New Issue**: If your enhancement is new, create an issue describing:
   - The problem your enhancement solves
   - How your enhancement would work
   - Any potential implementation details
   - Why this enhancement would benefit Cua users

## Code Formatting

We follow strict code formatting guidelines to ensure consistency across the codebase. Before submitting any code:

1. **Review Our Format Guide**: Please review our [Code Formatting Standards](Development.md#code-formatting-standards) section in the Getting Started guide.
2. **Configure Your IDE**: We recommend using the workspace settings provided in `.vscode/` for automatic formatting.
3. **Run Formatting Tools**: Always run the formatting tools before submitting a PR:
   ```bash
   # For Python code
   uv run black .
   uv run isort .
   uv run ruff check --fix .
   ```
4. **Validate Your Code**: Ensure your code passes all checks:
   ```bash
   uv run mypy .
   ```
5. Every time you try to commit code, a pre-commit hook will automatically run the formatting and validation tools. If any issues are found, the commit will be blocked until they are resolved. Please make sure to address any issues reported by the pre-commit hook before attempting to commit again. Once all issues are resolved, you can proceed with your commit.

## Documentation

Documentation improvements are always welcome. You can:

- Fix typos or unclear explanations
- Add examples and use cases
- Improve API documentation
- Add tutorials or guides

For detailed instructions on setting up your development environment and submitting code contributions, please see our [Developer-Guide](Development.md).

Feel free to join our [Discord community](https://discord.com/invite/mVnXXpdE85) to discuss ideas or get help with your contributions.
## Windows (Anaconda) Setup

> **Relevant to**: Windows 10 / 11 with Anaconda or Miniconda environments.

### PyTorch / OMP Duplicate Library Crash

When running the test suite on Windows with an Anaconda environment that has
`numpy` installed, you may see the following fatal crash:

```
OMP: Error #15: Initializing libiomp5md.dll, but found vcomp140.dll already initialized.
Fatal Python error: Aborted
```

**Root cause**: Anaconda ships its own OpenMP runtime (via `numpy`/`mkl`).
PyTorch bundles a separate OpenMP runtime.  When both are loaded in the same
process, the Intel OpenMP layer aborts to avoid undefined behaviour.

**Quick fix** – set the environment variable before running pytest:

```bat
:: Windows Command Prompt
set KMP_DUPLICATE_LIB_OK=TRUE
pytest tests/ -v
```

```powershell
# PowerShell
$env:KMP_DUPLICATE_LIB_OK="TRUE"
pytest tests/ -v
```

This variable is already set automatically inside the test suite's
`conftest.py` (via `os.environ.setdefault`), so unit tests that mock torch
will never hit this crash.  The manual step above is only needed when
running **integration tests** that import real torch.

**Permanent fix (optional)** – add the variable to your conda environment's
activation script so it is always set when the environment is active:

```bat
conda env config vars set KMP_DUPLICATE_LIB_OK=TRUE -n <your-env-name>
```

### Running Tests on Windows

```bat
:: 1. Clone and enter the repo
git clone https://github.com/trycua/cua.git
cd cua\libs\python\agent

:: 2. Install in editable mode (use pip or uv)
pip install -e ".[uitars-hf]"   :: if you need local HuggingFace models
pip install -e "."               :: base install

:: 3. Install test dependencies
pip install pytest pytest-asyncio

:: 4. Run the test suite
set KMP_DUPLICATE_LIB_OK=TRUE
pytest tests/ -v
```

The unit tests for `HuggingFaceLocalAdapter` use module-level mocks for
`torch` and `transformers`, so **you do not need a GPU or a full PyTorch
installation** to run `tests/test_huggingfacelocal_adapter.py`.
## Windows (Anaconda) Setup

> **Relevant to**: Windows 10 / 11 with Anaconda or Miniconda environments.

### PyTorch / OMP Duplicate Library Crash

When running the test suite on Windows with an Anaconda environment that has
`numpy` installed, you may see the following fatal crash:

```text
OMP: Error #15: Initializing libiomp5md.dll, but found vcomp140.dll already initialized.
Fatal Python error: Aborted
```

**Root cause**: Anaconda ships its own OpenMP runtime (via `numpy`/`mkl`).
PyTorch bundles a separate OpenMP runtime.  When both are loaded in the same
process, the Intel OpenMP layer aborts to avoid undefined behaviour.

**Quick fix** – set the environment variable before running pytest:

```bat
:: Windows Command Prompt
set KMP_DUPLICATE_LIB_OK=TRUE
pytest tests/ -v
```

```powershell
# PowerShell
$env:KMP_DUPLICATE_LIB_OK="TRUE"
pytest tests/ -v
```

This variable is already set automatically inside the test suite's
`conftest.py` (via `os.environ.setdefault`), so unit tests that mock torch
will never hit this crash.  The manual step above is only needed when
running **integration tests** that import real torch.

**Permanent fix (optional)** – add the variable to your conda environment's
activation script so it is always set when the environment is active:

```bat
conda env config vars set KMP_DUPLICATE_LIB_OK=TRUE -n <your-env-name>
```

### Running Tests on Windows

```bat
:: 1. Clone and enter the repo
git clone https://github.com/trycua/cua.git
cd cua\libs\python\agent

:: 2. Install in editable mode (choose ONE):
pip install -e "."               :: base install
pip install -e ".[uitars-hf]"   :: base + local HuggingFace models (heavy)

:: 3. Install test dependencies
pip install pytest pytest-asyncio

:: 4. Run the test suite
set KMP_DUPLICATE_LIB_OK=TRUE
pytest tests/ -v
```

The unit tests for `HuggingFaceLocalAdapter` use module-level mocks for
`torch` and `transformers`, so **you do not need a GPU or a full PyTorch
installation** to run `tests/test_huggingfacelocal_adapter.py`.
