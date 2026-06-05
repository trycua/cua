# Development Guide: Python cua-driver Wrapper

## Overview

This package is a thin Python wrapper around the cua-driver Rust binary. Each platform-specific wheel bundles the appropriate pre-compiled binary.

## Local Development

### Prerequisites

- Python 3.10+
- pip and build tools

### Setup

```bash
cd libs/cua-driver/python

# Create virtual environment
python -m venv venv
source venv/bin/activate  # or `venv\Scripts\activate` on Windows

# Install build dependencies
pip install build hatchling pytest
```

### Building the Wheel

The build process downloads the platform-specific binary and bundles it:

```bash
# Build for current platform (downloads binary from GitHub releases)
python build_wheel.py --version 0.5.1

# Skip download and use local binary (for testing)
# First, manually place binary in src/cua_driver/bin/
python build_wheel.py --skip-download
```

This creates a `.whl` file in `dist/` with the bundled binary.

### Testing Locally

```bash
# Install the built wheel
pip install dist/cua_driver-*.whl

# Test the CLI
cua-driver --version
cua-driver --help

# Test MCP server (Ctrl+C to exit)
cua-driver mcp
```

### Running Tests

```bash
# Run tests (requires built wheel with binary)
pytest tests/

# Run with coverage
pytest --cov=cua_driver tests/
```

## Release Process

### Automatic (Recommended)

When a new `cua-driver-rs-v*` tag is pushed, the CI workflow automatically:
1. Detects the new release
2. Builds platform-specific wheels (macOS, Linux, Windows)
3. Publishes to PyPI

### Manual

```bash
# Trigger workflow manually via GitHub Actions
# Go to: https://github.com/trycua/cua/actions/workflows/cd-py-cua-driver.yml
# Click "Run workflow" and enter the version (e.g., "0.5.1")
```

## Architecture

### Package Structure

```
libs/cua-driver/python/
├── pyproject.toml              # Package metadata
├── README.md                   # PyPI readme
├── build_wheel.py              # Build script (downloads binary)
├── src/
│   └── cua_driver/
│       ├── __init__.py         # Package entry point
│       ├── wrapper.py          # Subprocess wrapper
│       ├── __main__.py         # CLI entry point
│       └── bin/                # Binary goes here (gitignored)
│           └── cua-driver[.exe]
└── tests/
    └── test_wrapper.py         # Unit tests
```

### How It Works

1. **Build time**: `build_wheel.py` downloads the appropriate binary from GitHub releases and places it in `src/cua_driver/bin/`
2. **Install time**: The wheel includes the binary as package data
3. **Runtime**: The wrapper locates the bundled binary and executes it with stdio passthrough

### Binary Selection

The build script detects the platform and downloads the matching artifact:

| Platform | Artifact |
|----------|----------|
| macOS (any arch) | `cua-driver-rs-{VERSION}-darwin-universal-binary.tar.gz` |
| Linux x86_64 | `cua-driver-rs-{VERSION}-linux-x86_64-binary.tar.gz` |
| Windows x86_64 | `cua-driver-rs-{VERSION}-windows-x86_64-binary.zip` |
| Windows ARM64 | `cua-driver-rs-{VERSION}-windows-arm64-binary.zip` |

## Version Management

The package version is automatically synchronized with cua-driver-rs releases:

1. `pyproject.toml` has a default version (e.g., `version = "0.5.1"`)
2. On release, the CI workflow updates it to match the cua-driver-rs tag
3. The Python package version always matches the bundled binary version

## Troubleshooting

### Binary not found error

```
FileNotFoundError: cua-driver binary not found at .../bin/cua-driver
```

**Solution**: The wheel wasn't built correctly. Run `python build_wheel.py` to download and bundle the binary.

### Platform mismatch

If you build a wheel on one platform and try to install it on another, it won't work. Each platform needs its own wheel.

**Solution**: Use the CI workflow to build wheels for all platforms, or build locally on each target platform.

### Testing without building

Some tests require the binary to be bundled. If you see `pytest.skip` messages, run:

```bash
python build_wheel.py --version 0.5.1
pip install -e .
pytest tests/
```

## CI Workflow

The `.github/workflows/cd-py-cua-driver.yml` workflow:

1. **Trigger**: Automatically when `cua-driver-rs-v*` tag is pushed, or manually
2. **Build**: Creates wheels on macOS, Linux, and Windows runners
3. **Publish**: Uploads all wheels to PyPI using trusted publishing

### PyPI Configuration

The workflow uses PyPI's trusted publishing (no API token needed):
- Environment: `pypi`
- Publisher: GitHub Actions workflow
- Configure at: https://pypi.org/manage/account/publishing/

## Platform-Specific Notes

### macOS

- Uses the universal binary (ARM64 + x86_64)
- Binary is automatically made executable (`chmod +x`)

### Linux

- Currently x86_64 only
- Requires glibc 2.31+ (Ubuntu 20.04+)

### Windows

- Supports both x86_64 and ARM64
- `.exe` extension is automatically handled
- No special permissions needed

## Future Improvements

- [ ] Add wheel checksums verification
- [ ] Support offline installation (bundle binary in PyPI package data)
- [ ] Add `cua-driver upgrade` command
- [ ] Linux ARM64 support (when cua-driver-rs adds it)
- [ ] Automated testing in CI (install wheel and run tests)
