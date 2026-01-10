# Cua Linux Container

Containerized Ubuntu 22.04 LTS virtual desktop for Computer-Using Agents (Cua). Utilizes QEMU/KVM with Ubuntu Desktop and computer-server pre-installed for remote computer control.

## Features

- Ubuntu 22.04 LTS Desktop running in QEMU/KVM
- Automated installation via cloud-init autoinstall
- Pre-installed Cua computer-server for remote computer control
- Support for custom OEM scripts during setup
- noVNC access for visual desktop interaction

## Quick Start

### 1. Download Ubuntu Server ISO

**Download Ubuntu 22.04 LTS Server ISO:**

1. Visit & download the [server ISO](https://releases.ubuntu.com/22.04/ubuntu-22.04.5-live-server-amd64.iso)
2. After downloading, rename the file to `setup.iso`
3. Copy it to the directory `src/vm/image/`

This ISO is used for automated Ubuntu installation with cloud-init on first run.

### 2. Build the Image

```bash
docker build -t trycua/cua-qemu-linux:dev .
```

### 3. First Run - Create Golden Image

On first run, the container will install Ubuntu from scratch and create a golden image. This takes 15-30 minutes.

```bash
# Create storage directory
mkdir -p ./storage

# Run with setup.iso to create golden image
docker run -it --rm \
    --device=/dev/kvm \
    --name cua-qemu-linux \
    --mount type=bind,source=src/vm/image/setup.iso,target=/custom.iso \
    --cap-add NET_ADMIN \
    -v $(pwd)/storage:/storage \
    -p 8006:8006 \
    -p 5000:5000 \
    -e RAM_SIZE=8G \
    -e CPU_CORES=4 \
    -e DISK_SIZE=64G \
    trycua/cua-qemu-linux:dev
```

**What happens during first run:**

1. Ubuntu 22.04 Server installs automatically using cloud-init autoinstall
2. Minimal desktop environment is installed with auto-login enabled
3. OEM setup scripts install Python 3, create venv, and install Cua computer-server
4. systemd service created for Cua server (runs automatically on login)
5. X11 access configured for GUI automation
6. Golden image is saved to `/storage` directory
7. Container exits after setup completes

### 4. Subsequent Runs - Use Golden Image

After the golden image is created, subsequent runs boot much faster (30 sec - 2 min):

```bash
# Run without setup.iso - uses existing golden image
docker run -it --rm \
    --device=/dev/kvm \
    --name cua-qemu-linux \
    --cap-add NET_ADMIN \
    -v $(pwd)/storage:/storage \
    -p 8006:8006 \
    -p 5000:5000 \
    -e RAM_SIZE=8G \
    -e CPU_CORES=4 \
    trycua/cua-qemu-linux:dev
```

**Access points:**

- **Computer Server API**: `http://localhost:5000`
- **noVNC Browser**: `http://localhost:8006`

## Container Configuration

### Ports

- **5000**: Cua computer-server API endpoint
- **8006**: noVNC web interface for visual desktop access

### Environment Variables

- `RAM_SIZE`: RAM allocated to Ubuntu VM (default: "8G", recommended: "8G" for WSL2)
- `CPU_CORES`: CPU cores allocated to VM (default: "8")
- `DISK_SIZE`: VM disk size (default: "64G", minimum: "32G")

### Volumes

- `/storage`: Persistent VM storage (golden image, disk)
- `/custom.iso`: Mount point for setup.iso (only needed for first run)
- `/oem`: Optional mount point for custom OEM scripts (built-in scripts included in image)

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Docker Container (Linux host)                          │
│                                                         │
│  • Port forwarding: localhost:5000 → EMULATOR_IP:5000   │
│  • Exposes: 5000 (API), 8006 (noVNC)                    │
│                                                         │
│  ┌────────────────────────────────────────────────────┐ │
│  │  QEMU VM (Ubuntu 22.04)                            │ │
│  │                                                    │ │
│  │  • Cua computer-server listens on 5000             │ │
│  │                                                    │ │
│  └────────────────────────────────────────────────────┘ │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

**Communication Flow:**

1. External client → `localhost:5000` (host)
2. Docker port mapping → Container's `localhost:5000`
3. Container detects VM IP and waits for server to be ready
4. Cua computer-server in Ubuntu VM processes request

## Development

### Modifying Setup Scripts

Setup scripts are in `src/vm/setup/`:

- `install.sh`: Entry point called after cloud-init installation (runs OEM setup)
- `setup.sh`: Main setup orchestration (copies scripts to /opt/oem)
- `setup-cua-server.sh`: Cua server installation with isolated venv and systemd service

After modifying, rebuild the image:

```bash
docker build -t trycua/cua-qemu-linux:dev .
```
