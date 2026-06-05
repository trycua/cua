# Plan: Python PyPI Wrapper for cua-driver-rs

## Goal
Create a thin Python wrapper package (`cua-driver`) that automatically downloads the appropriate pre-built Rust binary and pipes stdio through to it, synchronized with cua-driver-rs releases.

## Architecture

### Package Structure
```
libs/cua-driver/python/
├── pyproject.toml           # Package metadata, version sync
├── README.md                # PyPI readme
├── src/
│   └── cua_driver/
│       ├── __init__.py      # Main entry point
│       ├── binary.py        # Binary download & management
│       ├── wrapper.py       # Subprocess stdio wrapper
│       └── __main__.py      # CLI entry point: python -m cua_driver
```

### Core Components

#### 1. **Binary Downloader (`binary.py`)**
- **Download logic**: Fetch from GitHub releases based on:
  - Platform: `sys.platform` → `darwin`/`linux`/`win32`
  - Architecture: `platform.machine()` → `x86_64`/`arm64`/`aarch64`
  - Release URL pattern: `https://github.com/trycua/cua/releases/download/cua-driver-rs-v{VERSION}/{ARTIFACT}`
  
- **Artifact selection**:
  - macOS: `cua-driver-rs-{VERSION}-darwin-universal-binary.tar.gz` (single file)
  - Linux: `cua-driver-rs-{VERSION}-linux-x86_64-binary.tar.gz`
  - Windows x64: `cua-driver-rs-{VERSION}-windows-x86_64-binary.zip`
  - Windows ARM: `cua-driver-rs-{VERSION}-windows-arm64-binary.zip`

- **Cache location**: `~/.cua-driver/bin/cua-driver-{VERSION}/cua-driver{.exe}`
- **Checksum verification**: Fetch `checksums.txt` from release, validate SHA256
- **Lazy download**: Only fetch on first run or version mismatch

#### 2. **Subprocess Wrapper (`wrapper.py`)**
- Execute binary with `subprocess.run()`:
  - Pass all CLI args through: `sys.argv[1:]`
  - Pipe stdin/stdout/stderr directly (no buffering)
  - Preserve exit code
  - Handle Ctrl+C gracefully
  
```python
def run_cua_driver(binary_path: str, args: list[str]) -> int:
    """Execute cua-driver binary with stdio passthrough."""
    try:
        result = subprocess.run(
            [binary_path, *args],
            stdin=sys.stdin,
            stdout=sys.stdout,
            stderr=sys.stderr,
        )
        return result.returncode
    except KeyboardInterrupt:
        return 130  # Standard SIGINT exit code
```

#### 3. **Entry Point (`__main__.py`)**
```python
from .binary import ensure_binary
from .wrapper import run_cua_driver

def main():
    binary_path = ensure_binary()  # Download if missing
    exit_code = run_cua_driver(binary_path, sys.argv[1:])
    sys.exit(exit_code)
```

### Version Synchronization

#### Release Workflow Integration
Extend `.github/workflows/cd-rust-cua-driver.yml` with a new job:

```yaml
publish-pypi:
  needs: [release]
  if: startsWith(github.ref, 'refs/tags/cua-driver-rs-v')
  runs-on: ubuntu-latest
  steps:
    - name: Determine version
      id: version
      run: |
        VERSION="${GITHUB_REF#refs/tags/cua-driver-rs-v}"
        echo "version=$VERSION" >> "$GITHUB_OUTPUT"
    
    - name: Update pyproject.toml version
      run: |
        sed -i "s/^version = .*/version = \"${{ steps.version.outputs.version }}\"/" \
          libs/cua-driver/python/pyproject.toml
    
    - name: Build Python package
      run: |
        cd libs/cua-driver/python
        pip install build
        python -m build
    
    - name: Publish to PyPI
      uses: pypa/gh-action-pypi-publish@release/v1
      with:
        packages-dir: libs/cua-driver/python/dist/
        password: ${{ secrets.PYPI_API_TOKEN }}
```

#### Version File
Keep version in sync via `libs/cua-driver/python/pyproject.toml`:
```toml
[project]
name = "cua-driver"
version = "0.5.1"  # AUTO-UPDATED by CD workflow
description = "Python wrapper for cua-driver-rs - cross-platform MCP server"
```

### Installation Flow

#### User installs via pip:
```bash
pip install cua-driver
```

#### First run:
```bash
cua-driver mcp
```
1. Python wrapper starts
2. Checks `~/.cua-driver/bin/cua-driver-{VERSION}/cua-driver`
3. If missing:
   - Downloads from GitHub release
   - Extracts to cache dir
   - Verifies checksum
   - Makes executable (chmod +x on Unix)
4. Executes binary with stdio passthrough
5. MCP server starts, communicates over stdio as expected

### Benefits

1. **pip-installable**: Pythonic install path for Python users
2. **No build required**: Ships no Rust compilation, just downloads
3. **Platform-agnostic**: Single `pip install` works on all platforms
4. **Automatic updates**: New PyPI release = new binary version
5. **Transparent**: Behaves identically to direct binary usage
6. **Minimal overhead**: ~10ms startup penalty for subprocess spawn

### Testing Strategy

1. **Unit tests**: Mock download, verify subprocess call
2. **Integration tests**: End-to-end with real binary (GitHub release)
3. **Platform matrix**: Test on macOS/Linux/Windows in CI
4. **MCP smoke test**: Verify stdio protocol works through wrapper

### Edge Cases

- **Offline installs**: Fail gracefully with clear error if download fails
- **Proxy support**: Respect HTTP_PROXY/HTTPS_PROXY env vars
- **Corrupted cache**: Re-download if checksum mismatch
- **Partial downloads**: Atomic write (download to .tmp, rename on success)
- **Concurrent installs**: Use file locks to prevent race conditions

### Deployment Timeline

1. **Phase 1**: Implement wrapper package (1-2 days)
2. **Phase 2**: Add CI workflow integration (1 day)
3. **Phase 3**: Test with next cua-driver-rs release
4. **Phase 4**: Announce on PyPI + update docs

### Future Enhancements

- **Version pinning**: `pip install cua-driver==0.5.1` downloads specific binary
- **Pre-download command**: `cua-driver download` to fetch without running
- **Upgrade command**: `cua-driver upgrade` to fetch latest
- **Binary fallback**: Bundle binary in wheel for air-gapped installs
