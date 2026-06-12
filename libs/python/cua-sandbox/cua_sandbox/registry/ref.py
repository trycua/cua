"""Parse OCI image references into (registry, org, name, tag)."""

from __future__ import annotations


def parse_ref(ref: str) -> tuple[str, str, str, str]:
    """Parse a full or short image reference.

    Handles:
      ghcr.io/trycua/macos-sequoia-cua:latest → (ghcr.io, trycua, macos-sequoia-cua, latest)
      trycua/cua-xfce:latest                  → (ghcr.io, trycua, cua-xfce, latest)
      cua-xfce:latest                         → (ghcr.io, trycua, cua-xfce, latest)
      cua-xfce                                → (ghcr.io, trycua, cua-xfce, latest)
    """
    if ":" in ref:
        ref_no_tag, tag = ref.rsplit(":", 1)
    else:
        ref_no_tag, tag = ref, "latest"

    parts = ref_no_tag.split("/")

    if len(parts) >= 3:
        registry = parts[0]
        org = parts[1]
        name = "/".join(parts[2:])
    elif len(parts) == 2:
        registry = "ghcr.io"
        org = parts[0]
        name = parts[1]
    else:
        registry = "ghcr.io"
        org = "trycua"
        name = parts[0]

    # Docker Hub: docker.io → registry-1.docker.io (the actual API host)
    if registry == "docker.io":
        registry = "registry-1.docker.io"

    return registry, org, name, tag
