# Cua Docker XFCE Container

Vanilla XFCE desktop container for Computer-Using Agents (Cua) with noVNC and computer-server. This is a lightweight alternative to the Kasm-based container with minimal dependencies.

## Features

- Ubuntu 22.04 (Jammy) with vanilla XFCE desktop environment
- TigerVNC server for remote desktop access
- noVNC for web-based VNC access (no client required)
- Pre-installed computer-server for remote computer control
- Python 3.11 with necessary libraries
- Screen capture tools (gnome-screenshot, wmctrl, ffmpeg)
- Clipboard utilities (xclip, socat)
- Firefox browser with telemetry disabled

## Architecture

```
┌─────────────────────────────────────────┐
│   Docker Container (Ubuntu 22.04)       │
├─────────────────────────────────────────┤
│  XFCE Desktop Environment               │
│  ├── Firefox                            │
│  ├── XFCE Terminal                      │
│  └── Desktop utilities                  │
├─────────────────────────────────────────┤
│  TigerVNC Server (Port 5901)            │
│  └── X11 Display :1                     │
├─────────────────────────────────────────┤
│  noVNC Web Interface (Port 6901)        │
│  └── WebSocket proxy to VNC             │
├─────────────────────────────────────────┤
│  Cua Computer Server (Port 8000)        │
│  └── WebSocket API for automation       │
└─────────────────────────────────────────┘
```

## Building the Container

```bash
docker build -t cua-xfce:latest .
```

### Build and Push (multi-arch)

Use Docker Buildx to build and push a multi-architecture image for both `linux/amd64` and `linux/arm64` in a single command. Replace `trycua` with your Docker Hub username or your registry namespace as needed.

```bash
# Login to your registry first (Docker Hub shown here)
docker login

# Build and push for amd64 and arm64 in one step
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t trycua/cua-xfce:latest \
  --push \
  .
```

## Running the Container Manually

### Basic Usage

```bash
docker run --rm -it \
  --shm-size=512m \
  -p 5901:5901 \
  -p 6901:6901 \
  -p 8000:8000 \
  cua-xfce:latest
```

### With Custom Resolution

```bash
docker run --rm -it \
  --shm-size=512m \
  -p 5901:5901 \
  -p 6901:6901 \
  -p 8000:8000 \
  -e VNC_RESOLUTION=1280x720 \
  cua-xfce:latest
```

### With Persistent Storage

```bash
docker run --rm -it \
  --shm-size=512m \
  -p 5901:5901 \
  -p 6901:6901 \
  -p 8000:8000 \
  -v $(pwd)/storage:/home/cua/storage \
  cua-xfce:latest
```

## Accessing the Container

- **noVNC Web Interface**: Open `http://localhost:6901` in your browser (no password required)
- **VNC Client**: Connect to `localhost:5901` (no password required)
- **Computer Server API**: Available at `http://localhost:8000`

## Using with Cua Docker Provider

This container is designed to work with the Cua Docker provider. Simply specify the xfce image:

```python
from computer import Computer

# Create computer with xfce container
computer = Computer(
    os_type="linux",
    provider_type="docker",
    image="trycua/cua-xfce:latest",  # Use xfce instead of Kasm
    display="1024x768",
    memory="4GB",
    cpu="2"
)

# Use the computer
async with computer:
    # Take a screenshot
    screenshot = await computer.interface.screenshot()

    # Click and type
    await computer.interface.left_click(100, 100)
    await computer.interface.type_text("Hello from Cua!")

    # Run commands
    result = await computer.interface.run_command("ls -la")
    print(result.stdout)
```

### Switching between Kasm and xfce

The Docker provider automatically detects which image you're using:

```python
# Use Kasm-based container (default for Linux)
computer_kasm = Computer(
    os_type="linux",
    provider_type="docker",
    image="trycua/cua-ubuntu:latest",  # Kasm image
)

# Use xfce container (vanilla XFCE)
computer_xfce = Computer(
    os_type="linux",
    provider_type="docker",
    image="trycua/cua-xfce:latest",  # xfce image
)
```

Both provide the same API and functionality - the provider automatically configures the correct paths and settings based on the image.

## Environment Variables

| Variable         | Default    | Description              |
| ---------------- | ---------- | ------------------------ |
| `VNC_RESOLUTION` | `1024x768` | Screen resolution        |
| `VNC_COL_DEPTH`  | `24`       | Color depth              |
| `VNC_PORT`       | `5901`     | VNC server port          |
| `NOVNC_PORT`     | `6901`     | noVNC web interface port |
| `API_PORT`       | `8000`     | Computer-server API port |
| `DISPLAY`        | `:1`       | X11 display number       |

## Exposed Ports

- **5901**: TigerVNC server
- **6901**: noVNC web interface
- **8000**: Computer-server WebSocket API

## Volume Mount Points

- `/home/cua/storage`: Persistent storage mount point
- `/home/cua/shared`: Shared folder mount point

## User Credentials

- **Username**: `cua`
- **Password**: `password` (for shell login only)
- **Sudo access**: Enabled without password
- **VNC access**: No password required

## Creating Snapshots

### Filesystem Snapshot

```bash
docker commit <container_id> cua-xfce-snapshot:latest
```

### Running from Snapshot

```bash
docker run --rm -it \
  --shm-size=512m \
  -p 6901:6901 \
  -p 8000:8000 \
  cua-xfce-snapshot:latest
```

## Comparison with Kasm Container

| Feature       | Kasm Container  | Docker XFCE Container |
| ------------- | --------------- | --------------------- |
| Base Image    | KasmWeb Ubuntu  | Vanilla Ubuntu        |
| VNC Server    | KasmVNC         | TigerVNC              |
| Dependencies  | Higher          | Lower                 |
| Configuration | Pre-configured  | Minimal               |
| Size          | Larger          | Smaller               |
| Maintenance   | Depends on Kasm | Independent           |

## Process Management

The container uses `supervisord` to manage three main processes:

1. **VNC Server** (Priority 10): TigerVNC with XFCE desktop
2. **noVNC** (Priority 20): WebSocket proxy for browser access
3. **Computer Server** (Priority 30): Cua automation API

All processes are automatically restarted on failure.

## Troubleshooting

### VNC server won't start

Check if X11 lock files exist:

```bash
docker exec <container_id> rm -rf /tmp/.X1-lock /tmp/.X11-unix/X1
```

### noVNC shows black screen

Ensure VNC server is running:

```bash
docker exec <container_id> supervisorctl status vncserver
```

### Computer-server not responding

Check if X server is accessible:

```bash
docker exec <container_id> env DISPLAY=:1 xdpyinfo
```

### View logs

```bash
docker exec <container_id> tail -f /var/log/supervisor/supervisord.log
docker exec <container_id> supervisorctl status
```

## Integration with Cua System

This container provides the same functionality as the Kasm container but with:

- **Reduced dependencies**: No reliance on KasmWeb infrastructure
- **Smaller image size**: Minimal base configuration
- **Full control**: Direct access to all components
- **Easy customization**: Simple to modify and extend

The container integrates seamlessly with:

- Cua Computer library (via WebSocket API)
- Docker provider for lifecycle management
- Standard VNC clients for debugging
- Web browsers for visual monitoring

## License

MIT License - See LICENSE file for details
