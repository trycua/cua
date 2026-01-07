# Windows Arena on Azure (WAA) - Implementation Guide

This document provides a comprehensive overview of the Windows Arena Azure implementation, including the evolution from Azure ML to Azure Batch, the development workflow with remote VMs, and the complete architecture of the system.

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture Evolution](#architecture-evolution)
3. [System Components](#system-components)
4. [Dev Mode vs Production Mode](#dev-mode-vs-production-mode)
5. [Development Workflow](#development-workflow)
6. [Azure Batch Implementation](#azure-batch-implementation)
7. [Container Architecture](#container-architecture)
8. [Windows VM Integration](#windows-vm-integration)
9. [Configuration Management](#configuration-management)
10. [Benchmark Tasks](#benchmark-tasks)
11. [Evaluation System](#evaluation-system)
12. [Troubleshooting & Lessons Learned](#troubleshooting--lessons-learned)

---

## Overview

Windows Arena Azure (WAA) is a benchmarking system that runs AI agents against Windows desktop tasks. The system runs a Windows 11 VM inside a Docker container using QEMU/KVM, with a Python client orchestrating benchmark tasks against the VM via a custom HTTP API (CUA Computer Server).

### Key Components

```
┌─────────────────────────────────────────────────────────────────┐
│  Azure Batch / Local Docker                                      │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Linux Container (trycua/winarena:latest)                 │  │
│  │  ┌─────────────────────────────────────────────────────┐  │  │
│  │  │  Client (run.py)                                    │  │  │
│  │  │  - NaviAgent / ClaudeAgent                          │  │  │
│  │  │  - Vision-language models (GPT-4o, Claude, etc.)    │  │  │
│  │  │  - Set-of-Marks (SoM) processing                    │  │  │
│  │  └──────────────────────┬──────────────────────────────┘  │  │
│  │                         │ HTTP (172.30.0.2:5000)          │  │
│  │  ┌──────────────────────▼──────────────────────────────┐  │  │
│  │  │  Windows 11 VM (QEMU/KVM)                           │  │  │
│  │  │  - CUA Computer Server (Flask, port 5000)           │  │  │
│  │  │  - PyAutoGUI, UIA automation                        │  │  │
│  │  │  - Task execution environment                       │  │  │
│  │  └─────────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Architecture Evolution

### Phase 1: Azure ML (Deprecated)

The original implementation used Azure ML for running benchmarks:

- **Script:** `run_azure.py` (deleted)
- **Infrastructure:** Azure ML Compute Instances
- **Configuration:** `config.json` with Azure ML workspace settings
- **Issues:**
  - Complex Azure ML SDK integration
  - Expensive compute resources
  - Limited container customization options

### Phase 2: Azure Batch (Current)

Migrated to Azure Batch for simpler, more cost-effective container execution:

- **Script:** `scripts/run_azure_batch.py`
- **Infrastructure:** Azure Batch pools with container support
- **Configuration:** `.env.local` for secrets
- **Benefits:**
  - Direct Docker container execution
  - Privileged mode support (required for QEMU/KVM)
  - Better cost control with auto-cleanup pools
  - Simpler API and debugging

### Key Migration Changes

| Aspect | Azure ML | Azure Batch |
|--------|----------|-------------|
| Config | `config.json` (JSON) | `.env.local` (env vars) |
| Scripts | `run_azure.py`, `azure_files/` | `run_azure_batch.py` |
| VM Mode | Azure mode (OEM scripts) | Dev mode (Samba share) |
| Image | Separate azure/dev builds | Unified dev mode build |

---

## System Components

### Directory Structure

```
denpasar-v1/
├── scripts/
│   ├── run_azure_batch.py      # Azure Batch orchestration
│   ├── run.sh                   # Docker run script (production)
│   ├── run-local.sh             # Docker run script (development)
│   ├── build-container-image.sh # Docker build script
│   └── shared.sh                # Common utilities
├── src/win-arena-container/
│   ├── Dockerfile-WinArena      # Main container image
│   ├── Dockerfile-WinArena-Base # Base image with Python deps
│   ├── entry.sh                 # Main entrypoint
│   ├── entry_setup.sh           # VM startup script
│   ├── start_vm.sh              # QEMU launcher
│   ├── start_client.sh          # Benchmark client launcher
│   ├── client/                  # Python benchmark client
│   │   ├── run.py               # Main benchmark runner
│   │   ├── mm_agents/           # AI agents (Navi, Claude)
│   │   └── desktop_env/         # Environment controllers
│   └── vm/
│       ├── setup/               # Windows setup scripts
│       │   ├── setup.ps1        # PowerShell setup
│       │   ├── install.bat      # Batch installer
│       │   └── server/          # CUA Computer Server
│       ├── storage/             # VM disk images
│       └── unattend-files/      # Windows unattend configs
├── .env.local                   # Azure credentials (gitignored)
├── .env.example                 # Template for credentials
└── experiments.json             # Batch experiment definitions
```

---

## Dev Mode vs Production Mode

### Dev Mode (Development)

Used for developing and testing changes to VM setup scripts:

```bash
./run-local.sh --mode dev --prepare-image true
```

**Features:**
- **Samba Share:** Windows VM accesses `\\host.lan\Data` which maps to `/shared/` in the container
- **Live Sync:** Changes to `src/win-arena-container/vm/setup/` appear immediately in Windows
- **Interactive Access:** RDP (port 3390) and noVNC (port 8006) available
- **Fast Iteration:** No rebuild needed for script changes

**Mount Structure:**
```
Host                                  Container           Windows VM
vm/setup/ ────────────────────────> /shared/ ───────> \\host.lan\Data
vm/storage/ ──────────────────────> /storage
client/ ──────────────────────────> /client
```

### Production Mode (Azure Batch)

Used for running actual benchmarks at scale:

```bash
python run_azure_batch.py --exp_name gpt4o --num_workers 4
```

**Features:**
- **Pre-baked Image:** Setup scripts are copied into container at build time
- **No Interactive Access:** Fully automated execution
- **Parallel Workers:** Multiple tasks run simultaneously
- **Auto-cleanup:** Pools deleted after job completion

---

## Development Workflow

### Setting Up the Development Environment

1. **Clone and configure:**
   ```bash
   cp .env.example .env.local
   # Edit .env.local with your Azure credentials
   ```

2. **Build base image (once):**
   ```bash
   cd scripts
   ./build-container-image.sh --build-base-image true
   ```

3. **Start dev container:**
   ```bash
   ./run-local.sh --mode dev --prepare-image true --start-client false
   ```

4. **Access Windows VM:**
   - RDP: `localhost:3390` (user: `Admin`, no password)
   - Browser: `http://localhost:8006`

5. **Iterate on setup scripts:**
   - Edit files in `src/win-arena-container/vm/setup/`
   - Run from Windows: `\\host.lan\Data\install.bat`

### Building for Remote x86_64 VM

Since the container requires x86_64 architecture (KVM), build on a remote VM:

1. **Sync code to remote VM:**
   ```bash
   rsync -avz --exclude='.git' --exclude='vm/storage' \
     ./ user@remote-vm:/path/to/denpasar-v1/
   ```

2. **Build on remote:**
   ```bash
   ssh user@remote-vm
   cd /path/to/denpasar-v1/scripts
   ./build-container-image.sh
   ```

3. **Push to registry:**
   ```bash
   docker push trycua/winarena:latest
   # For ACR:
   az acr import --name winarenamlacr \
     --source docker.io/trycua/winarena:latest \
     --image winarena:latest
   ```

### Golden Image Strategy

Creating a golden image saves ~45 minutes per benchmark run:

1. **Create golden image:**
   ```bash
   ./run-local.sh --mode dev --prepare-image true
   # Wait for Windows setup to complete (~1 hour)
   # Manually verify everything is installed
   # Then gracefully shutdown:
   curl -X POST http://172.30.0.2:5000/cmd \
     -H "Content-Type: application/json" \
     -d '{"command": "run_command", "params": {"command": "shutdown /s /t 5"}}'
   ```

2. **Backup storage:**
   ```bash
   cp -r src/win-arena-container/vm/storage /backup/golden-storage
   ```

3. **Restore for benchmarks:**
   ```bash
   cp /backup/golden-storage/* src/win-arena-container/vm/storage/
   ```

---

## Azure Batch Implementation

### Pool Configuration

```python
# From run_azure_batch.py
pool = batchmodels.PoolAddParameter(
    id=pool_id,
    vm_size='Standard_D4s_v3',  # 4 vCPUs, 16GB RAM
    target_dedicated_nodes=num_workers,
    virtual_machine_configuration=batchmodels.VirtualMachineConfiguration(
        image_reference=batchmodels.ImageReference(
            publisher='microsoft-azure-batch',
            offer='ubuntu-server-container',
            sku='20-04-lts',
            version='latest'
        ),
        container_configuration=batchmodels.ContainerConfiguration(
            type='dockerCompatible',
            container_image_names=[]  # Pulled in start task
        ),
        os_disk=batchmodels.OSDisk(
            disk_size_gb=128  # Large enough for ~40GB container
        )
    )
)
```

### Start Task (Node Preparation)

Each node runs a start task that:
1. Reconfigures Docker to use OS disk (not temp disk)
2. Logs into Azure Container Registry
3. Pulls the ~40GB container image

```bash
# Configure Docker to use OS disk
mkdir -p /var/lib/docker_new
systemctl stop docker
cat > /etc/docker/daemon.json << 'EOF'
{"data-root": "/var/lib/docker_new", "storage-driver": "overlay2"}
EOF
systemctl start docker

# Pull container image
docker pull winarenamlacr.azurecr.io/winarena:latest
```

### Task Execution

Each task runs in a privileged container:

```python
container_settings = batchmodels.TaskContainerSettings(
    image_name='winarenamlacr.azurecr.io/winarena:latest',
    container_run_options='--privileged --shm-size=16g -v /mnt/input:/mnt/input'
)

command_line = '''
    azcopy copy "https://storage.blob.core.windows.net/container/storage/*?SAS" /storage/ --recursive
    /entry_setup.sh
    cd /client && python run.py --agent_name navi --model gpt-4o --emulator_ip 172.30.0.2
'''
```

### Storage Integration

**Blobfuse Mount (for small files):**
```python
mount_configuration=[
    batchmodels.MountConfiguration(
        azure_blob_file_system_configuration=batchmodels.AzureBlobFileSystemConfiguration(
            account_name=storage_account,
            container_name=container_input,
            relative_mount_path='input'
        )
    )
]
```

**azcopy (for large files like VM images):**
```bash
# Blobfuse has I/O issues with large files and QEMU's native AIO
# Use azcopy instead for reliable large file downloads
azcopy copy "https://account.blob.core.windows.net/container/storage/*?SAS" /storage/ --recursive
```

---

## Container Architecture

### Two-Stage Build

**Stage 1: Base Image (Dockerfile-WinArena-Base)**
```dockerfile
FROM python:3.12-slim
# Install system dependencies
RUN apt-get update && apt-get install -y \
    dos2unix libgl1 libevdev-dev tesseract-ocr
# Install Python dependencies
COPY requirements.txt /tmp/
RUN pip install -r /tmp/requirements.txt
```

**Stage 2: Application Image (Dockerfile-WinArena)**
```dockerfile
FROM trycua/winarena-base:latest
# Copy setup scripts to Samba share location
COPY src/win-arena-container/vm/setup/. /shared/
# Copy client application
COPY src/win-arena-container/client /client
# Copy entrypoint scripts
COPY src/win-arena-container/entry*.sh start*.sh /
# Install azcopy for Azure Blob downloads
RUN apt-get install -y fuse curl && \
    curl -L https://aka.ms/downloadazcopy-v10-linux | tar xz && \
    mv azcopy_linux*/azcopy /usr/local/bin/
```

### Environment Variables

```dockerfile
ENV YRES="900"          # VM display height
ENV XRES="1440"         # VM display width
ENV RAM_SIZE="8G"       # VM memory
ENV CPU_CORES="8"       # VM CPU cores
ENV DISK_SIZE="30G"     # VM disk size
ENV VERSION="win11x64-enterprise-eval"
ENV ARGUMENTS="-qmp tcp:0.0.0.0:7200,server,nowait"
```

---

## Windows VM Integration

### CUA Computer Server

The CUA Computer Server is a Flask-based HTTP API running inside the Windows VM:

**Endpoints:**
| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/status` | GET | Health check |
| `/cmd` | POST | Execute automation commands |

**Command Types:**
```python
# Screenshot
{"command": "screenshot", "params": {}}

# Click
{"command": "click", "params": {"x": 100, "y": 200, "button": "left"}}

# Type
{"command": "type", "params": {"text": "Hello World"}}

# Get accessibility tree
{"command": "get_accessibility_tree", "params": {}}

# Run Windows command
{"command": "run_command", "params": {"command": "notepad.exe"}}
```

### Networking

```
Docker Bridge (172.17.0.0/16)
    │
    └── Container Network
          │
          └── dnsmasq DHCP (172.30.0.0/24)
                │
                ├── 172.30.0.1  Gateway (Linux host)
                └── 172.30.0.2  Windows VM (DHCP assigned)
```

The client connects to `http://172.30.0.2:5000` to communicate with the Windows VM.

---

## Configuration Management

### .env.local Structure

```bash
# OpenAI API Key
OPENAI_API_KEY=sk-proj-...

# Azure Batch
AZURE_BATCH_ACCOUNT_NAME=winarenabatch
AZURE_BATCH_ACCOUNT_URL=winarenabatch.eastus.batch.azure.com
AZURE_BATCH_ACCOUNT_KEY=...

# Azure Storage
AZURE_STORAGE_ACCOUNT_NAME=winarenastorage
AZURE_STORAGE_ACCOUNT_KEY=...
AZURE_STORAGE_CONTAINER_INPUT=input-container
AZURE_STORAGE_CONTAINER_OUTPUT=output-container
AZURE_STORAGE_SAS=se=2025-12-31...

# Azure Container Registry
AZURE_ACR_SERVER=winarenamlacr.azurecr.io
AZURE_ACR_USERNAME=winarenamlacr
AZURE_ACR_PASSWORD=...
```

### experiments.json Structure

```json
{
  "gpt4o": {
    "agent": "navi",
    "docker_img_name": "winarenamlacr.azurecr.io/winarena:latest",
    "exp_name": "gpt4o",
    "num_workers": 4,
    "json_name": "evaluation_examples_windows/test_all.json",
    "model_name": "gpt-4o",
    "som_origin": "oss",
    "a11y_backend": "uia"
  }
}
```

---

## Benchmark Tasks

### Task Domains

The benchmark includes **173 tasks** across **12 application domains**:

| Domain | Task Count | Description |
|--------|-----------|-------------|
| LibreOffice Calc | 24 | Spreadsheet operations, charts, formulas |
| VS Code | 23 | IDE settings, extensions, keybindings |
| File Explorer | 19 | File management, folder operations |
| LibreOffice Writer | 19 | Document editing, formatting |
| VLC Media Player | 21 | Media playback, settings |
| Chrome | 17 | Browser tabs, bookmarks, extensions |
| MS Edge | 13 | Browser settings, shortcuts |
| Settings | 5 | Windows system settings |
| Clock | 4 | Alarms, timers |
| Windows Calculator | 3 | Calculator operations |
| Microsoft Paint | 3 | Image editing |
| Notepad | 2 | Text file operations |

### Task Definition Structure

Each task is defined in a JSON file with the following structure:

```json
{
  "id": "unique-task-id",
  "snapshot": "application-name",
  "instruction": "Natural language task description",
  "source": "URL or reference source",
  "config": [
    {
      "type": "launch|download|open|sleep|activate_window|execute|create_folder",
      "parameters": {...}
    }
  ],
  "trajectory": "path/to/trajectory",
  "related_apps": ["app-name"],
  "evaluator": {
    "func": "evaluation-function-name",
    "result": {...},
    "expected": {...},
    "postconfig": [...]
  }
}
```

### Configuration Types

The `config` array contains setup operations executed before the task:

| Type | Purpose | Example |
|------|---------|---------|
| `launch` | Start applications | Launch Chrome with remote debugging |
| `download` | Download test files | Get files from GitHub/cloud |
| `open` | Open existing files | Open documents for editing |
| `sleep` | Wait for UI | Pause for app to load |
| `activate_window` | Focus window | Bring app to foreground |
| `execute` | Run commands | Execute Python scripts |
| `create_folder` | Create directories | Setup test folder structure |

### Task File Locations

```
src/win-arena-container/client/evaluation_examples_windows/
├── test_all.json              # All 173 tasks
├── test_small.json            # Sample subset
├── test_custom.json           # Custom test set
└── examples/
    ├── chrome/                # 17 Chrome tasks
    ├── libreoffice_calc/      # 24 Calc tasks
    ├── libreoffice_writer/    # 19 Writer tasks
    ├── vs_code/               # 23 VS Code tasks
    ├── msedge/                # 13 Edge tasks
    ├── vlc/                   # 21 VLC tasks
    ├── file_explorer/         # 19 File Explorer tasks
    ├── notepad/               # 2 Notepad tasks
    ├── windows_calc/          # 3 Calculator tasks
    ├── microsoft_paint/       # 3 Paint tasks
    ├── settings/              # 5 Settings tasks
    └── clock/                 # 4 Clock tasks
```

---

## Evaluation System

### Evaluation Flow

```
1. Task Configuration Phase
   └─ Execute config[] operations (download files, launch apps)

2. Agent Execution Phase
   └─ Agent runs actions (max 15 steps per task)

3. Post-Configuration Phase
   └─ Run postconfig[] operations (save files, focus windows)

4. Evaluation Phase
   └─ Extract result using result type specification
   └─ Load expected values
   └─ Run evaluation function(s)
   └─ Return score (0.0 = fail, 1.0 = success)
```

### Evaluation Functions

**General Purpose:**
| Function | Purpose |
|----------|---------|
| `exact_match` | Binary equality check |
| `fuzzy_match` | Fuzzy string matching (RapidFuzz) |
| `literal_match` | Case-sensitive/insensitive literal comparison |
| `is_in_list` | Check if result is in expected list |
| `check_include_exclude` | Check for included/excluded substrings |
| `check_csv` | Verify CSV content matches rules |
| `check_json` | Compare JSON file content |
| `diff_text_file` | Text file similarity ratio |

**Application-Specific:**

| Domain | Function | Purpose |
|--------|----------|---------|
| Chrome/Edge | `is_expected_active_tab` | Verify active tab URL |
| Chrome/Edge | `is_expected_tabs` | Verify all open tabs |
| Chrome/Edge | `is_expected_bookmarks` | Check bookmarks |
| Chrome/Edge | `is_expected_installed_extensions` | Verify extensions |
| LibreOffice | `compare_table` | Compare spreadsheets (cells, charts, pivots) |
| LibreOffice | `compare_docx_files` | Compare Word documents |
| VS Code | `check_json_settings` | Verify settings.json |
| System | `is_file_saved_desktop` | Check file on desktop |
| System | `vm_file_exists_in_vm_folder` | Verify file existence |
| Media | `compare_images` | Image similarity comparison |

### Result Types

The `result` field specifies what to extract from the VM:

```json
// File from VM
{"type": "vm_file", "path": "C:\\Users\\Docker\\file.txt", "dest": "result.txt"}

// Check file existence
{"type": "vm_file_exists_in_vm_folder", "folder_name": "C:\\path", "file_name": "file.txt"}

// Cloud file download
{"type": "cloud_file", "path": "https://example.com/file.txt", "dest": "expected.txt"}

// Chrome profile name
{"type": "profile_name"}

// System timezone
{"type": "system_timezone"}
```

### Expected Value Types

```json
// Rule-based matching
{
  "type": "rule",
  "rules": {
    "expected": "expected_value"
  }
}

// File-based comparison
{
  "type": "cloud_file",
  "path": "https://example.com/gold_standard.xlsx",
  "dest": "gold_standard.xlsx"
}
```

### Matching Methods

Used within rule definitions:

| Method | Description |
|--------|-------------|
| `eq` | Equality |
| `ne` | Not equal |
| `contains` | Substring containment |
| `startswith` | Prefix match |
| `endswith` | Suffix match |
| `regex` | Regular expression |
| `fuzzy` | Fuzzy string matching |

### Spreadsheet Evaluation (`compare_table`)

LibreOffice Calc tasks support detailed comparison:

```json
"evaluator": {
  "func": "compare_table",
  "options": {
    "rules": [
      {"type": "sheet_data", "sheet_idx0": 0, "sheet_idx1": "EI0"},
      {"type": "chart", "sheet_idx0": 0, "chart_props": ["type", "title"]},
      {"type": "pivot_table", "sheet_idx0": 0},
      {"type": "conditional_formatting", "sheet_idx0": 0}
    ]
  }
}
```

**Comparison Types:**
- `sheet_data` - Cell values
- `sheet_print` - Displayed values
- `sheet_name` - Sheet names
- `chart` - Chart properties
- `pivot_table` - Pivot table structure
- `sparklines` - Inline charts
- `conditional_formatting` - Format rules
- `data_validation` - Input validation rules

### Example Task Definitions

**Chrome Profile Update:**
```json
{
  "instruction": "Change the username in chrome profiles to Thomas",
  "evaluator": {
    "func": "exact_match",
    "result": {"type": "profile_name"},
    "expected": {"type": "rule", "rules": {"expected": "Thomas"}}
  }
}
```

**Spreadsheet with Chart:**
```json
{
  "instruction": "Create monthly total sales row and line chart",
  "evaluator": {
    "func": "compare_table",
    "expected": {"type": "cloud_file", "path": "https://.../gold.xlsx", "dest": "gold.xlsx"},
    "result": {"type": "vm_file", "path": "C:\\...\\result.xlsx", "dest": "result.xlsx"},
    "options": {
      "rules": [
        {"type": "sheet_data", "sheet_idx0": 0, "sheet_idx1": "EI0"},
        {"type": "chart", "sheet_idx0": 0, "chart_props": ["type"]}
      ]
    }
  }
}
```

**File Creation:**
```json
{
  "instruction": "Create draft.txt in Documents with content 'This is a draft.'",
  "evaluator": {
    "func": ["exact_match", "compare_text_file"],
    "result": [
      {"type": "vm_file_exists_in_vm_folder", "folder_name": "C:\\Users\\Docker\\Documents", "file_name": "draft.txt"},
      {"type": "vm_file", "path": "C:\\Users\\Docker\\Documents\\draft.txt", "dest": "draft.txt"}
    ],
    "expected": [
      {"type": "rule", "rules": {"expected": 1.0}},
      {"type": "cloud_file", "path": "https://.../draft_gold.txt", "dest": "draft_gold.txt"}
    ]
  }
}
```

### Evaluator Implementation Files

```
src/win-arena-container/client/desktop_env/evaluators/
├── metrics/
│   ├── general.py        # Base evaluators (497 lines)
│   ├── table.py          # Spreadsheet comparison (521 lines)
│   ├── chrome.py         # Browser evaluation (419 lines)
│   ├── docs.py           # Document evaluation (879 lines)
│   ├── vscode.py         # IDE evaluation (281 lines)
│   ├── edge.py           # Edge browser (28 lines)
│   ├── basic_os.py       # System evaluation (68 lines)
│   ├── gimp.py           # Image editor (617 lines)
│   ├── vlc.py            # Media player (450 lines)
│   ├── slides.py         # Presentations (529 lines)
│   ├── pdf.py            # PDF content (31 lines)
│   └── utils.py          # Helpers (686 lines)
└── getters/              # Data extraction utilities
```

---

## Troubleshooting & Lessons Learned

### Issue 1: Blobfuse I/O Errors

**Problem:** Large files (like 30GB VM images) fail with I/O errors when read through blobfuse.

**Solution:** Use azcopy for large file downloads instead of relying on blobfuse mounts:
```bash
azcopy copy "https://account.blob.core.windows.net/container/storage/*?SAS" /storage/ --recursive
```

### Issue 2: Docker Temp Disk Full

**Problem:** Azure Batch nodes have small temp disks (~30GB) that fill up when pulling the ~40GB container image.

**Solution:** Configure Docker to use the OS disk (128GB managed disk) in the start task:
```bash
cat > /etc/docker/daemon.json << 'EOF'
{"data-root": "/var/lib/docker_new", "storage-driver": "overlay2"}
EOF
```

### Issue 3: Wrong Emulator IP

**Problem:** Client defaults to `20.20.20.21` instead of the actual VM IP `172.30.0.2`.

**Solution:** Explicitly pass `--emulator_ip 172.30.0.2` to the client.

### Issue 4: Windows Can't Access Samba Share

**Problem:** In Azure mode, setup scripts were copied to `/oem/` but Samba was configured to share `/shared/`.

**Solution:** Unified to dev mode which copies to `/shared/` for all deployments.

### Issue 5: Building on Wrong Architecture

**Problem:** Docker builds fail on macOS ARM when the image requires x86_64 (for KVM).

**Solution:** Build on a remote x86_64 VM:
```bash
rsync -avz ./ user@remote-vm:/path/
ssh user@remote-vm "cd /path/scripts && ./build-container-image.sh"
```

### Debugging Tips

1. **Check task logs in Azure Batch:**
   ```python
   output = batch_client.file.get_from_task(job_id, task_id, 'stdout.txt')
   ```

2. **SSH into running container:**
   ```bash
   ./run-local.sh --connect true
   ```

3. **Test CUA Server health:**
   ```bash
   curl http://172.30.0.2:5000/status
   ```

4. **View Windows VM via browser:**
   Open `http://localhost:8006` when running in dev mode.

---

## Quick Reference

### Common Commands

```bash
# Build container
./scripts/build-container-image.sh

# Run locally (dev mode)
./scripts/run-local.sh --mode dev --start-client false

# Run locally (production mode)
./scripts/run.sh --mode azure --agent navi --model gpt-4o

# Run on Azure Batch
python scripts/run_azure_batch.py --exp_name gpt4o --num_workers 4

# Sync to remote VM
rsync -avz --exclude='.git' --exclude='vm/storage' ./ user@vm:/path/
```

### Key Ports

| Port | Purpose |
|------|---------|
| 5000 | CUA Computer Server (internal) |
| 7200 | QEMU QMP Protocol |
| 3390 | RDP (dev mode only) |
| 8006 | noVNC browser access (dev mode only) |

### Key IPs

| IP | Purpose |
|----|---------|
| 172.30.0.1 | Linux container gateway |
| 172.30.0.2 | Windows VM (default DHCP) |

---

*Last updated: December 2024*
