# CUA Ubuntu Container

Containerized virtual desktop for Computer-Using Agents (CUA). Utilizes Kasm's MIT-licensed Ubuntu XFCE container as a base with computer-server pre-installed.

## Features

- Ubuntu 22.04 (Jammy) with XFCE desktop environment
- Pre-installed computer-server for remote computer control
- VNC access for visual desktop interaction
- Python 3.11 with necessary libraries
- Screen capture tools (gnome-screenshot, wmctrl, ffmpeg)
- Clipboard utilities (xclip, socat)

## Usage

### Build and Push (multi-arch)

Use Docker Buildx to build and push a multi-architecture image for both `linux/amd64` and `linux/arm64` in a single command. Replace `trycua` with your Docker Hub username or your registry namespace as needed.

```bash
# Login to your registry first (Docker Hub shown here)
docker login

# Build and push for amd64 and arm64 in one step
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t trycua/cua-ubuntu:latest \
  --push \
  .
```

### Running the Container Manually

```bash
docker run --rm -it --shm-size=512m -p 6901:6901 -p 8000:8000 -e VNCOPTIONS=-disableBasicAuth cua-ubuntu:latest
```

- **VNC Access**: Available at `http://localhost:6901`
- **Computer Server API**: Available at `http://localhost:8000`

### Using with CUA Docker Provider

This container is designed to work with the CUA Docker provider for automated container management:

```python
from computer.providers.factory import VMProviderFactory

# Create docker provider
provider = VMProviderFactory.create_provider(
    provider_type="docker",
    image="cua-ubuntu:latest",
    port=8000,  # computer-server API port
    noVNC_port=6901  # VNC port
)

# Run a container
async with provider:
    vm_info = await provider.run_vm(
        image="cua-ubuntu:latest",
        name="my-cua-container",
        run_opts={
            "memory": "4GB",
            "cpu": 2,
            "vnc_port": 6901,
            "api_port": 8000
        }
    )
```

## Container Configuration

### Ports

- **6901**: VNC web interface (noVNC)
- **8080**: Computer-server API endpoint

### Environment Variables

- `VNC_PW`: VNC password (default: "password")
- `DISPLAY`: X11 display (set to ":0")

### Volumes

- `/home/kasm-user/storage`: Persistent storage mount point
- `/home/kasm-user/shared`: Shared folder mount point

## Creating Filesystem Snapshots

You can create a filesystem snapshot of the container at any time:

```bash
docker commit <container_id> cua-ubuntu-snapshot:latest
```

Then run the snapshot:

```bash
docker run --rm -it --shm-size=512m -p 6901:6901 -p 8080:8080 -e VNCOPTIONS=-disableBasicAuth cua-ubuntu-snapshot:latest
```

Memory snapshots are available using the experimental `docker checkpoint` command. [Docker Checkpoint Documentation](https://docs.docker.com/reference/cli/docker/checkpoint/)

## Integration with CUA System

This container integrates seamlessly with the CUA computer provider system:

- **Automatic Management**: Use the Docker provider for lifecycle management
- **Resource Control**: Configure memory, CPU, and storage limits
- **Network Access**: Automatic port mapping and IP detection
- **Storage Persistence**: Mount host directories for persistent data
- **Monitoring**: Real-time container status and health checking
