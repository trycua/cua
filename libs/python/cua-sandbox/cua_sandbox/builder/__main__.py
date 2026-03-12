"""CLI for building QEMU VM images.

Usage:
  python -m cua_sandbox.builder build-base --os windows --version 11
  python -m cua_sandbox.builder build-base --os windows --version 11 --iso-path /path/to/win11.iso
  python -m cua_sandbox.builder build --spec '{"os_type":"windows","version":"11","layers":[{"type":"winget_install","packages":["Git.Git"]}]}'
  python -m cua_sandbox.builder info
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
from pathlib import Path

from cua_sandbox.builder.overlay import IMAGES_DIR, base_image_path


def main():
    logging.basicConfig(level=logging.INFO, format="%(message)s")

    parser = argparse.ArgumentParser(description="CUA Sandbox image builder")
    sub = parser.add_subparsers(dest="command")

    # build-base
    bb = sub.add_parser("build-base", help="Build a base image (OS + computer-server)")
    bb.add_argument("--os", default="windows", help="OS type (windows, linux)")
    bb.add_argument("--version", default="11", help="OS version")
    bb.add_argument("--iso-path", help="Path to OS ISO (skip download)")
    bb.add_argument("--product-key", help="Windows product key")
    bb.add_argument("--force", action="store_true", help="Rebuild even if cached")

    # build (from Image spec)
    b = sub.add_parser("build", help="Build a user image from an Image spec JSON")
    b.add_argument("--spec", required=True, help="Image spec as JSON string or @file.json")
    b.add_argument("--force", action="store_true")

    # info
    sub.add_parser("info", help="Show cached images")

    args = parser.parse_args()

    if args.command == "build-base":
        from cua_sandbox.builder.build import ensure_base_image
        disk = asyncio.run(ensure_base_image(
            args.os, args.version,
            windows_iso=args.iso_path,
            product_key=args.product_key,
            force=args.force,
        ))
        print(f"Base image: {disk}")

    elif args.command == "build":
        spec_str = args.spec
        if spec_str.startswith("@"):
            spec_str = Path(spec_str[1:]).read_text()
        spec = json.loads(spec_str)

        from cua_sandbox.image import Image
        from cua_sandbox.builder.build import build_user_image, ensure_base_image

        image = Image.from_dict(spec)
        base = asyncio.run(ensure_base_image(image.os_type, image.version))
        user_disk = asyncio.run(build_user_image(image, base, force=args.force))
        print(f"User image: {user_disk}")

    elif args.command == "info":
        if not IMAGES_DIR.exists():
            print(f"No images directory: {IMAGES_DIR}")
            return
        print(f"Images directory: {IMAGES_DIR}\n")
        for d in sorted(IMAGES_DIR.iterdir()):
            if d.is_dir():
                disk = d / "disk.qcow2"
                meta = d / "disk.json"
                size = f"{disk.stat().st_size / 1024 / 1024:.0f} MB" if disk.exists() else "no disk"
                layers = ""
                if meta.exists():
                    m = json.loads(meta.read_text())
                    layers = f" ({len(m.get('layers', []))} layers)"
                print(f"  {d.name}: {size}{layers}")

    else:
        parser.print_help()


if __name__ == "__main__":
    main()
