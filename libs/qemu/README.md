# CUA Windows Container

Containerized Windows 11 virtual desktop for Computer-Using Agents (CUA). Utilizes QEMU/KVM with Windows 11 and computer-server pre-installed for remote computer control.

## Features

- Windows 11 Enterprise running in QEMU/KVM
- Pre-installed CUA computer-server for remote computer control
- noVNC access for visual desktop interaction
- Automated setup via unattended installation
- Support for both dev (shared folder) and azure (OEM folder) deployment modes
- Python 3.10 with CUA computer-server
- Essential tools pre-installed (Chrome, LibreOffice, VLC, GIMP, VSCode, Thunderbird)

## Quick Start

### 1. Download and Prepare setup.iso

**Download Windows 11 Evaluation ISO:**

1. Visit [Microsoft Evaluation Center](https://info.microsoft.com/ww-landing-windows-11-enterprise.html)
2. Accept the Terms of Service
3. Download **Windows 11 Enterprise Evaluation (90-day trial, English, United States)** ISO file [~6GB]
4. After downloading, rename the file to `setup.iso`
5. Copy it to the directory `src/vm/image/`

This ISO is used for automated Windows installation on first run.

### 2. Build the Image

```bash
# Build for dev mode (uses shared folder)
docker build --build-arg DEPLOY_MODE=dev -t cua-windows:dev .

# Or build for azure mode (uses OEM folder)
docker build --build-arg DEPLOY_MODE=azure -t cua-windows:azure .
```

### 3. First Run - Create Golden Image

On first run, the container will install Windows from scratch and create a golden image. This takes 15-20 minutes.

```bash
# Create storage directory
mkdir -p ./storage

# Run with setup.iso to create golden image
docker run -it --rm \
    --device=/dev/kvm \
    --platform linux/amd64 \
    --name cua-windows \
    --mount type=bind,source=$(pwd)/src/vm/image/setup.iso,target=/custom.iso \
    -v $(pwd)/storage:/storage \
    -p 8006:8006 \
    -p 5000:5000 \
    -e RAM_SIZE=4G \
    -e CPU_CORES=4 \
    -e DISK_SIZE=20G \
    cua-windows:dev
```

**What happens during first run:**

1. Windows 11 installs automatically using unattended configuration
2. Setup scripts run to install Python, tools, and CUA computer-server
3. Golden image is saved to `/storage` directory
4. On-logon task starts CUA computer-server on boot

### 4. Subsequent Runs - Use Golden Image

After the golden image is created, subsequent runs boot much faster (2-5 minutes):

```bash
# Run without setup.iso - uses existing golden image
docker run -it --rm \
    --device=/dev/kvm \
    --platform linux/amd64 \
    --name cua-windows \
    -v $(pwd)/storage:/storage \
    -p 8006:8006 \
    -p 5000:5000 \
    -e RAM_SIZE=4G \
    -e CPU_CORES=4 \
    cua-windows:dev
```

**Access points:**

- **Computer Server API**: `http://localhost:5000`
- **noVNC Browser**: `http://localhost:8006`

## Container Configuration

### Ports

- **5000**: CUA computer-server API endpoint
- **8006**: noVNC web interface for visual desktop access

### Environment Variables

- `RAM_SIZE`: RAM allocated to Windows VM (default: "8G", recommended: "4G" for WSL2)
- `CPU_CORES`: CPU cores allocated to VM (default: "8")
- `DISK_SIZE`: VM disk size (default: "30G", minimum: "20G")
- `YRES`: Screen height (default: "900")
- `XRES`: Screen width (default: "1440")
- `VERSION`: Windows version (default: "win11x64-enterprise-eval")
- `DEPLOY_MODE`: Deployment mode - "dev" or "azure" (set during build)

### Volumes

- `/storage`: Persistent VM storage (golden image, disk, firmware)
- `/custom.iso`: Mount point for setup.iso (only needed for first run)

### Deployment Modes

**Dev Mode (`DEPLOY_MODE=dev`):**

- Setup files mounted at `/shared` in container
- Accessible to Windows VM via `\\host.lan\Data`
- Useful for development and testing

**Azure Mode (`DEPLOY_MODE=azure`):**

- Setup files copied to `/oem` in container
- Accessible to Windows VM via `C:\oem`
- Used for production deployment on Azure ML

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Docker Container (Linux host)                          │
│                                                         │
│  • Port forwarding: localhost:5000 → EMULATOR_IP:5000   │
│  • Exposes: 5000 (API), 8006 (noVNC)                    │
│                                                         │
│  ┌────────────────────────────────────────────────────┐ │
│  │  QEMU VM (Windows 11)                              │ │
│  │                                                    │ │
│  │  • CUA computer-server listens on 5000             │ │
│  │                                                    │ │
│  └────────────────────────────────────────────────────┘ │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

**Communication Flow:**

1. External client → `localhost:5000` (host)
2. Docker port mapping → Container's `localhost:5000`
3. socat port forwarding → `20.20.20.21:5000` (VM)
4. CUA computer-server in Windows VM processes request

## Development

### Modifying Setup Scripts

Setup scripts are in `src/vm/setup/`:

- `setup.ps1`: Main setup script (installs software, configures Windows)
- `on-logon.ps1`: Runs on user logon (starts computer-server)
- `install.bat`: Entry point called by Windows setup

After modifying, rebuild the image:

```bash
docker build --build-arg DEPLOY_MODE=dev -t cua-windows:dev .
```

### Customizing Windows Configuration

Unattended installation files are in `src/vm/unattend-files/`:

- `dev_win11x64-enterprise-eval.xml`: For dev mode
- `azure_win11x64-enterprise-eval.xml`: For Azure mode

These XML files control:

- User account creation
- Auto-logon settings
- First-run commands
- Timezone and locale

## Credits

- Built on [Dockur Windows](https://github.com/dockur/windows) base image
- Inspired by [Windows Agent Arena](https://github.com/microsoft/WindowsAgentArena)
