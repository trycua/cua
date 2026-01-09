# Windows Arena Adapter

The Windows Arena adapter enables running Windows desktop automation benchmarks within cua-bench. It provides **173 tasks** across **12 application domains** including Chrome, LibreOffice, VS Code, VLC, File Explorer, and more.

## Requirements

- **x86_64 Linux** with KVM support (nested virtualization)
- **Docker** installed and running
- **~50GB disk space** for the base image and ISO
- Not supported on macOS/ARM (requires x86_64 + KVM)

## Quick Start

### 1. First-Time Setup

Create the base image (takes ~45-60 minutes):

```bash
# Download Windows ISO and create base image
python -m tasks.winarena_adapter.cli vm setup --download-iso

# Or provide your own ISO
python -m tasks.winarena_adapter.cli vm setup --iso /path/to/windows.iso
```

### 2. Check Status

```bash
python -m tasks.winarena_adapter.cli vm status
```

Output:

```
Windows Arena Status
========================================
Docker:       ✓ Running
Image:        ✓ trycua/winarena:latest
Windows ISO:  ✓ Found
Golden Image: ✓ Ready
KVM:          ✓ Available
Container:    ○ Not running
```

### 3. Run Benchmarks

```bash
# List all available tasks
python -m tasks.winarena_adapter.cli task list --verbose

# Run a specific task variant
cb run task tasks/winarena_adapter --variant-id <variant-index>
```

## CLI Commands

The CLI is organized into two command groups: `vm` for VM management and `task` for benchmark tasks.

### VM Commands

#### `vm setup`

Create the Windows VM base image:

```bash
python -m tasks.winarena_adapter.cli vm setup [options]

Options:
  --iso PATH          Path to Windows 11 ISO file
  --download-iso      Download Windows 11 ISO from Microsoft (~6GB)
  --detach, -d        Run in background (detached mode)
  --force             Force recreation even if base image exists
  --ram SIZE          VM RAM size (default: 8G)
  --cpus NUM          VM CPU cores (default: 8)
  --browser-port PORT noVNC port (default: 8006)
  --rdp-port PORT     RDP port (default: 3390)
```

#### `vm start`

Start the Windows VM for testing or development:

```bash
python -m tasks.winarena_adapter.cli vm start [options]

Options:
  --ram SIZE          VM RAM size (default: 8G)
  --cpus NUM          VM CPU cores (default: 8)
  --browser-port PORT noVNC port (default: 8006)
  --rdp-port PORT     RDP port (default: 3390)
  --container-name    Container name (default: winarena)
```

#### `vm stop`

Stop the running Windows VM container:

```bash
python -m tasks.winarena_adapter.cli vm stop [options]

Options:
  --container-name    Container name (default: winarena)
```

#### `vm status`

Check the setup status:

```bash
python -m tasks.winarena_adapter.cli vm status
```

### Task Commands

#### `task list`

List all available benchmark tasks:

```bash
python -m tasks.winarena_adapter.cli task list [--verbose]
```

#### `task run`

Run the Windows Arena container with the benchmark client:

```bash
python -m tasks.winarena_adapter.cli task run [options]

Options:
  --interactive, -i   Start interactive shell
  --no-client         Don't start the benchmark client
  --ram SIZE          VM RAM size (default: 8G)
  --cpus NUM          VM CPU cores (default: 8)
  --browser-port PORT noVNC port (default: 8006)
  --rdp-port PORT     RDP port (default: 3390)
  --container-name    Container name (default: winarena)
```

## Architecture

### Base Image

The base image is a pre-configured Windows 11 VM snapshot stored at `~/.local/share/cua-bench/images/`:

```
~/.local/share/cua-bench/
├── images/
│   └── windows-qemu/    # Windows base image
│       ├── data.img     # 30GB Windows disk image
│       ├── windows.boot # Ready marker file
│       ├── windows.vars # UEFI variables
│       └── ...
├── workers/             # Per-worker overlay directories
│   ├── 0/              # Worker 0's changes (Copy-on-Write)
│   ├── 1/              # Worker 1's changes
│   └── ...
└── windows.iso          # Original ISO (can be deleted after setup)
```

### Copy-on-Write (CoW) Strategy

During benchmark execution:

1. **Base image remains read-only** - never modified during tasks
2. **Each worker gets an overlay** - changes are written to worker-specific storage
3. **Fast reset between tasks** - just discard the overlay, no 30GB copy needed
4. **Parallel execution** - multiple workers can run simultaneously

### Container Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Docker Container (trycua/winarena:latest)                      │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Benchmark Client (Python)                                │  │
│  │  - Task setup and execution                               │  │
│  │  - AI Agent (Navi/Claude)                                 │  │
│  │  - Evaluation metrics                                     │  │
│  └──────────────────────┬────────────────────────────────────┘  │
│                         │ HTTP (172.30.0.2:5000)                │
│  ┌──────────────────────▼────────────────────────────────────┐  │
│  │  Windows 11 VM (QEMU/KVM)                                 │  │
│  │  - CUA Computer Server (Flask API)                        │  │
│  │  - PyAutoGUI automation                                   │  │
│  │  - Pre-installed apps (Chrome, LibreOffice, VLC, etc.)    │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Task Domains

| Domain             | Tasks | Description                              |
| ------------------ | ----- | ---------------------------------------- |
| LibreOffice Calc   | 24    | Spreadsheet operations, charts, formulas |
| VS Code            | 23    | IDE settings, extensions, keybindings    |
| VLC Media Player   | 21    | Media playback, settings                 |
| File Explorer      | 19    | File management, folder operations       |
| LibreOffice Writer | 19    | Document editing, formatting             |
| Chrome             | 17    | Browser tabs, bookmarks, extensions      |
| MS Edge            | 13    | Browser settings, shortcuts              |
| Settings           | 5     | Windows system settings                  |
| Clock              | 4     | Alarms, timers                           |
| Windows Calculator | 3     | Calculator operations                    |
| Microsoft Paint    | 3     | Image editing                            |
| Notepad            | 2     | Text file operations                     |

## Monitoring

### Browser Access (noVNC)

During base image creation or task execution, view the Windows VM at:

```
http://localhost:8006
```

### Container Logs

```bash
# Follow logs
docker logs -f winarena

# Check status
docker ps -f name=winarena
```

### RDP Access

Connect via RDP for interactive debugging:

```
Host: localhost
Port: 3390
User: Docker
Password: (none)
```

## Troubleshooting

### Golden image creation fails

1. Check Docker is running: `docker info`
2. Verify KVM is available: `ls -la /dev/kvm`
3. Check disk space: `df -h ~/.local/share/cua-bench`
4. View logs: `docker logs winarena-setup`

### Connection to CUA Server fails

The Windows VM needs time to boot and start the CUA Computer Server. Wait for the log message:

```
VM is up and running, and the CUA Computer Server is ready to use!
```

### VLC/tool installation fails

Tool downloads use mirror lists in `infra/vm/setup/tools_config.json`. If a download fails, the setup script tries alternate mirrors.

## Development

For development and Azure Batch deployment details, see:

- [infra/README.md](infra/README.md) - Detailed implementation guide
- [infra/docs/](infra/docs/) - Development tips and agent documentation

### Setup Scripts and Live Updates

The setup scripts (`setup.ps1`, `tools_config.json`, etc.) control how the Windows VM is configured during base image creation.

**Local CLI (vm setup, vm start, task run):**

- Setup folder is **mounted** at `/shared` inside the container
- Changes to `infra/vm/setup/` take effect immediately without rebuilding
- Edit `tools_config.json` to update download mirrors, then re-run `vm setup`

**Azure Batch (production):**

- Setup scripts are **baked into the Docker image** at build time
- Changes require rebuilding and pushing the Docker image
- This ensures reproducible, versioned production runs

```
infra/vm/setup/
├── setup.ps1           # Main PowerShell setup script
├── tools_config.json   # Tool download mirrors (VLC, Chrome, etc.)
├── setup-tools.psm1    # PowerShell module for tool installation
├── install.bat         # Batch installer entry point
├── on-logon.ps1        # Script run on Windows logon
└── server/             # CUA Computer Server (Python Flask API)
```

To update tool mirrors or setup behavior:

1. Edit files in `infra/vm/setup/`
2. Sync to remote VM: `./sync-to-remote.sh`
3. Re-run `vm setup` (local setup folder is auto-mounted)

### Syncing to Remote VM

For testing on a remote x86_64 VM with KVM:

```bash
# Edit sync-to-remote.sh with your VM details
./sync-to-remote.sh

# Watch for changes and auto-sync
./sync-to-remote.sh --watch
```

### Rebuilding Docker Image

For Azure Batch production runs, rebuild the image after changing setup scripts:

```bash
# On the remote x86_64 VM
cd infra/scripts
./build-container-image.sh

# Push to registry
docker push trycua/winarena:latest
```
