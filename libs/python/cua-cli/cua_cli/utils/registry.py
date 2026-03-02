"""Image registry for CUA CLI.

Manages a JSON registry of locally stored images at ~/.local/state/cua/images.json.
"""

import json
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

from cua_cli.utils.paths import get_data_dir, get_state_dir


def get_image_registry_path() -> Path:
    """Get the image registry file path."""
    return get_state_dir() / "images.json"


def load_image_registry() -> Dict[str, Any]:
    """Load the image registry."""
    registry_path = get_image_registry_path()
    if registry_path.exists():
        try:
            data = json.loads(registry_path.read_text())
            return data.get("images", data)
        except Exception:
            return {}
    return {}


def save_image_registry(registry: Dict[str, Any]) -> None:
    """Save the image registry."""
    registry_path = get_image_registry_path()
    registry_path.parent.mkdir(parents=True, exist_ok=True)
    data = {"version": 1, "images": registry}
    registry_path.write_text(json.dumps(data, indent=2))


def register_image(
    name: str,
    platform: str,
    path: Path,
    description: str = "",
    docker_image: Optional[str] = None,
    config: Optional[Dict] = None,
    parent: Optional[str] = None,
    tags: Optional[List[str]] = None,
    apps_installed: Optional[List[str]] = None,
) -> None:
    """Register an image in the registry."""
    from cua_cli.commands.platform import PLATFORMS

    registry = load_image_registry()

    entry: Dict[str, Any] = {
        "platform": platform,
        "path": str(path),
        "description": description,
        "created_at": datetime.now().isoformat(),
    }

    if docker_image:
        entry["docker_image"] = docker_image
    if config:
        entry["config"] = config
    if parent:
        entry["parent"] = parent
    if tags:
        entry["tags"] = tags
    if apps_installed:
        entry["apps_installed"] = apps_installed

    platform_config = PLATFORMS.get(platform, {})
    if platform_config.get("image_marker"):
        entry["marker_file"] = platform_config["image_marker"]

    registry[name] = entry
    save_image_registry(registry)


def unregister_image(name: str) -> None:
    """Remove an image from the registry."""
    registry = load_image_registry()
    if name in registry:
        del registry[name]
        save_image_registry(registry)


def get_image_info(name: str) -> Optional[Dict]:
    """Get image info by name."""
    registry = load_image_registry()
    return registry.get(name)


def list_images() -> Dict[str, Any]:
    """List all registered images."""
    return load_image_registry()


def auto_discover_images() -> None:
    """Scan images directory and register any unregistered images.

    Handles images created with --detach mode that never got registered.
    """
    from cua_cli.commands.platform import PLATFORMS

    images_dir = get_data_dir() / "images"
    if not images_dir.exists():
        return

    registry = load_image_registry()
    modified = False

    for image_dir in images_dir.iterdir():
        if not image_dir.is_dir():
            continue

        name = image_dir.name

        if name in registry:
            continue

        for platform_name, config in PLATFORMS.items():
            marker = config.get("image_marker")
            if not marker:
                continue

            marker_path = image_dir / marker
            if marker_path.exists():
                print(f"Auto-registering discovered image: {name} ({platform_name})")

                entry = {
                    "platform": platform_name,
                    "path": str(image_dir),
                    "description": f"Auto-discovered {platform_name} image",
                    "created_at": datetime.now().isoformat(),
                    "docker_image": config.get("image"),
                    "config": {"memory": "8G", "cpus": "8"},
                    "tags": ["auto-discovered"],
                    "marker_file": marker,
                }

                registry[name] = entry
                modified = True
                break

    if modified:
        save_image_registry(registry)
