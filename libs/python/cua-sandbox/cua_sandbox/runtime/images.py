"""Default image references, port mappings, and runtime constants."""

from typing import Optional

# Default images live in one place: the native resolver's canonical images
# (``cua_sandbox.image.canonical_image``: ghcr.io/trycua/{linux,windows,macos}).

# ── Guest ports ──────────────────────────────────────────────────────────────

#: cua-spacesd (gRPC + gRPC-Web, and the HTML5 viewer at ``/viewer/``)
#: inside every SDK image. The canonical images publish no VNC port.
SPACESD_PORT = 3211

LUME_PROVIDER_PORT = 7777
#: Lume guests run cua-spacesd on the same port as every other image.
LUME_API_PORT = SPACESD_PORT

# ── Default host-side ports (hints; the SDK allocates free ports) ─────────────

DEFAULT_API_PORT = SPACESD_PORT
DEFAULT_VNC_PORT = 6901


def internal_ports(docker_image: str) -> tuple[int, Optional[int]]:
    """Return (spacesd port, VNC port) inside the given container image.

    The canonical images serve their display through the cua-spacesd viewer
    on the spacesd port and run no VNC server, so the VNC port is ``None``.
    """
    return SPACESD_PORT, None
