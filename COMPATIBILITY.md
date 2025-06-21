# C/ua Compatibility Matrix

## Table of Contents
- [Host OS Compatibility](#host-os-compatibility)
  - [macOS Host](#macos-host)
  - [Ubuntu/Linux Host](#ubuntulinux-host)
  - [Windows Host](#windows-host)
- [VM Emulation Support](#vm-emulation-support)
- [Model Provider Compatibility](#model-provider-compatibility)

---

## Host OS Compatibility

*This section shows compatibility based on your **host operating system** (the OS you're running C/ua on).*

### macOS Host

| Installation Method | Requirements | Lume | Cloud | Notes |
|-------------------|-------------|------|-------|-------|
| **playground-docker.sh** | Docker Desktop | ✅ Full | ✅ Full | Recommended for quick setup |
| **Dev Container** | VS Code/WindSurf + Docker | ✅ Full | ✅ Full | Best for development |
| **PyPI packages** | Python 3.12+ | ✅ Full | ✅ Full | Most flexible |

**macOS Host Requirements:**
- macOS 15+ (Sequoia) for local VM support
- Apple Silicon (M1/M2/M3/M4) recommended for best performance
- Docker Desktop for containerized installations

---

### Ubuntu/Linux Host

| Installation Method | Requirements | Lume | Cloud | Notes |
|-------------------|-------------|------|-------|-------|
| **playground-docker.sh** | Docker Engine | ✅ Full | ✅ Full | Recommended for quick setup |
| **Dev Container** | VS Code/WindSurf + Docker | ✅ Full | ✅ Full | Best for development |
| **PyPI packages** | Python 3.12+ | ✅ Full | ✅ Full | Most flexible |

**Ubuntu/Linux Host Requirements:**
- Ubuntu 20.04+ or equivalent Linux distribution
- Docker Engine or Docker Desktop
- Python 3.12+ for PyPI installation

---

### Windows Host

| Installation Method | Requirements | Lume | Winsandbox | Cloud | Notes |
|-------------------|-------------|------|------------|-------|-------|
| **playground-docker.sh** | Docker Desktop + WSL2 | ❌ Not supported | ❌ Not supported | ✅ Full | Requires WSL2 |
| **Dev Container** | VS Code/WindSurf + Docker + WSL2 | ❌ Not supported | ❌ Not supported | ✅ Full | Requires WSL2 |
| **PyPI packages** | Python 3.12+ | ❌ Not supported | ✅ Full | ✅ Full |  |

**Windows Host Requirements:**
- Windows 10/11 with WSL2 enabled for shell script execution
- Docker Desktop with WSL2 backend
- Windows Sandbox feature enabled (for Winsandbox support)
- Python 3.12+ installed in WSL2 or Windows
- **Note**: Lume CLI is not available on Windows - use Cloud or Winsandbox providers

---

## VM Emulation Support

*This section shows which **virtual machine operating systems** each provider can emulate.*

| Provider | macOS VM | Ubuntu/Linux VM | Windows VM | Notes |
|----------|----------|-----------------|------------|-------|
| **Lume** | ✅ Full support | ⚠️ Limited support | ⚠️ Limited support | macOS: native; Ubuntu/Linux/Windows: need custom image |
| **Cloud** | 🚧 Coming soon | ✅ Full support | 🚧 Coming soon | Currently Ubuntu only, macOS/Windows in development |
| **Winsandbox** | ❌ Not supported | ❌ Not supported | ✅ Windows only | Windows 10/11 environments only |

---

## Model Provider Compatibility

*This section shows which **AI model providers** are supported on each host operating system.*

| Provider | macOS Host | Ubuntu/Linux Host | Windows Host | Notes |
|----------|------------|-------------------|--------------|-------|
| **Anthropic** | ✅ Full support | ✅ Full support | ✅ Full support | Cloud-based API |
| **OpenAI** | ✅ Full support | ✅ Full support | ✅ Full support | Cloud-based API |
| **Ollama** | ✅ Full support | ✅ Full support | ✅ Full support | Local model serving |
| **OpenAI Compatible** | ✅ Full support | ✅ Full support | ✅ Full support | Any OpenAI-compatible API endpoint |
| **MLX VLM** | ✅ macOS only | ❌ Not supported | ❌ Not supported | Apple Silicon required. PyPI installation only. |
