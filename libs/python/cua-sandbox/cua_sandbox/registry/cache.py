"""Local image cache at ~/.cua/cua-sandbox/images/."""

from __future__ import annotations

import json
import shutil
from pathlib import Path
from typing import Optional

CACHE_ROOT = Path.home() / ".cua" / "cua-sandbox" / "images"


class ImageCache:
    """Manages locally cached VM/container images."""

    def __init__(self, root: Optional[Path] = None):
        self.root = root or CACHE_ROOT
        self.root.mkdir(parents=True, exist_ok=True)

    def image_dir(self, registry: str, org: str, name: str, tag: str) -> Path:
        return self.root / registry / org / name / tag

    def is_cached(self, registry: str, org: str, name: str, tag: str) -> bool:
        return (self.image_dir(registry, org, name, tag) / "manifest.json").exists()

    def save_manifest(self, registry: str, org: str, name: str, tag: str, manifest: dict) -> Path:
        d = self.image_dir(registry, org, name, tag)
        d.mkdir(parents=True, exist_ok=True)
        p = d / "manifest.json"
        p.write_text(json.dumps(manifest, indent=2))
        return d

    def get_manifest(self, registry: str, org: str, name: str, tag: str) -> Optional[dict]:
        p = self.image_dir(registry, org, name, tag) / "manifest.json"
        if p.exists():
            return json.loads(p.read_text())
        return None

    def list_images(self) -> list[dict]:
        images = []
        for p in self.root.rglob("manifest.json"):
            try:
                parts = p.relative_to(self.root).parts
                if len(parts) >= 4:
                    images.append(
                        {
                            "registry": parts[0],
                            "org": parts[1],
                            "name": parts[2],
                            "tag": parts[3],
                            "path": str(p.parent),
                        }
                    )
            except Exception:
                pass
        return images

    def remove(self, registry: str, org: str, name: str, tag: str) -> bool:
        d = self.image_dir(registry, org, name, tag)
        if d.exists():
            shutil.rmtree(d)
            return True
        return False
