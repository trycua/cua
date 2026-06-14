"""Docker provider for running containers with computer-server."""

import subprocess

from .provider import DockerProvider

# Check if Docker is available
try:
    subprocess.run(["docker", "--version"], capture_output=True, check=True)
    HAS_DOCKER = True
except (subprocess.SubprocessError, FileNotFoundError):
    HAS_DOCKER = False

try:
    subprocess.run(["container", "system", "version"], capture_output=True, check=True)
    HAS_CONTAINER = True
except (subprocess.SubprocessError, FileNotFoundError):
    HAS_CONTAINER = False

__all__ = ["DockerProvider", "HAS_DOCKER", "HAS_CONTAINER"]
