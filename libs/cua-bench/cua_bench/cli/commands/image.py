"""``cb image``: the images tasks run on.

Since cua-bench 0.3 tasks run on registry images: the canonical
``ghcr.io/trycua/<os>`` images by default, or any ``--image <registry ref>``.
``list``/``info`` show the canonical images; ``create``/``delete``/``clone``/
``shell`` (the local golden-image store) are deprecated and do nothing. The
flags still parse, so existing scripts keep running.
"""

from __future__ import annotations
from ._help import examples

import json

from ._canonical import MIGRATION, canonical_images, find

RESET = "\033[0m"
BOLD = "\033[1m"
YELLOW = "\033[33m"
GREY = "\033[90m"
RED = "\033[91m"


def cmd_list(args) -> int:
    rows = canonical_images()
    platform = getattr(args, "platform", None)
    if platform:
        match = find(platform)
        rows = [match] if match else []
    if getattr(args, "format", "table") == "json":
        print(json.dumps(rows, indent=2))
        return 0
    print(f"{BOLD}{'OS':<9}  {'IMAGE':<34}  {'KINDS':<14}  {'ON':<12}  VARIANTS{RESET}")
    for row in rows:
        print(
            f"{row['name']:<9}  {row['image']:<34}  {'/'.join(row['kinds']):<14}  "
            f"{'/'.join(row['on']):<12}  {row['variants']}"
        )
    print(f"\n{GREY}Any registry image works too: cb run <task> --image <ref>{RESET}")
    return 0


def cmd_info(args) -> int:
    row = find(args.name)
    if row is None:
        print(f"{RED}No canonical image named {args.name!r} (linux, windows, macos).{RESET}")
        print(f"{GREY}{MIGRATION}{RESET}")
        return 1
    for key in ("name", "image", "kinds", "on", "variants"):
        value = row[key]
        print(f"{key + ':':<10} {'/'.join(value) if isinstance(value, list) else value}")
    return 0


def _deprecated(args) -> int:
    print(
        f"{YELLOW}cb image {args.image_command} is deprecated and does nothing. {MIGRATION}{RESET}"
    )
    return 0


cmd_create = cmd_delete = cmd_clone = cmd_shell = _deprecated


def register_parser(subparsers):
    """Register the image command with the main CLI parser."""
    image_parser = subparsers.add_parser(
        "image",
        help="Show the canonical images (the local image store is deprecated)",
        **examples(
            ("List the canonical images", "cb image"), ("Show one image", "cb image info linux")
        ),
    )
    image_subparsers = image_parser.add_subparsers(dest="image_command", help="Image command")

    # image list
    list_parser = image_subparsers.add_parser(
        "list",
        help="List all images",
        **examples(
            ("List images", "cb image list"),
            (
                "List one platform's images as JSON",
                "cb image list --platform linux-docker --format json",
            ),
        ),
    )
    list_parser.add_argument("--platform", help="Filter by platform")
    list_parser.add_argument(
        "--format", choices=["table", "json"], default="table", help="Output format"
    )

    # image info
    info_parser = image_subparsers.add_parser(
        "info",
        help="Show image details",
        **examples(("Show one image", "cb image info linux")),
    )
    info_parser.add_argument("name", help="Image name")

    # image create
    create_parser = image_subparsers.add_parser(
        "create",
        help="Deprecated: create an image from a platform",
        **examples(
            ("Create a Linux image (deprecated)", "cb image create linux-docker --name my-linux")
        ),
    )
    create_parser.add_argument("platform", help="Platform name (e.g., linux-docker, windows-qemu)")
    create_parser.add_argument("--name", help="Image name (default: same as platform)")
    create_parser.add_argument("--iso", help="Path to ISO file (for QEMU platforms)")
    create_parser.add_argument(
        "--download-iso",
        action="store_true",
        dest="download_iso",
        help="Download Windows 11 ISO (~6GB)",
    )
    create_parser.add_argument("--docker-image", dest="docker_image", help="Override Docker image")
    create_parser.add_argument(
        "--distro", default="ubuntu", choices=["ubuntu", "fedora"], help="Linux distribution"
    )
    create_parser.add_argument(
        "--version", default="14", help="OS version (e.g., 14 for Android, sonoma for macOS)"
    )
    create_parser.add_argument("--disk", default="64G", help="Disk size (default: 64G)")
    create_parser.add_argument("--memory", default="8G", help="Memory (default: 8G)")
    create_parser.add_argument("--cpus", default="8", help="CPU cores (default: 8)")
    create_parser.add_argument(
        "--winarena-apps",
        action="store_true",
        dest="winarena_apps",
        help="Install WinArena benchmark apps (Chrome, LibreOffice, VLC, etc.)",
    )
    create_parser.add_argument("--detach", "-d", action="store_true", help="Run in background")
    create_parser.add_argument("--force", action="store_true", help="Force recreation")
    create_parser.add_argument(
        "--skip-pull", action="store_true", dest="skip_pull", help="Don't pull Docker image"
    )
    create_parser.add_argument(
        "--no-kvm", action="store_true", dest="no_kvm", help="Disable KVM acceleration"
    )
    create_parser.add_argument(
        "--vnc-port", dest="vnc_port", help="VNC port (default: auto-allocate from 8006)"
    )
    create_parser.add_argument(
        "--api-port", dest="api_port", help="API port (default: auto-allocate from 5000)"
    )

    # image delete
    delete_parser = image_subparsers.add_parser(
        "delete",
        help="Deprecated: delete a stored image",
        **examples(("Delete a stored image (deprecated)", "cb image delete my-linux --force")),
    )
    delete_parser.add_argument("name", help="Image name")
    delete_parser.add_argument("--force", action="store_true", help="Skip confirmation")

    # image clone
    clone_parser = image_subparsers.add_parser(
        "clone",
        help="Deprecated: clone a stored image",
        **examples(("Clone a stored image (deprecated)", "cb image clone my-linux my-linux-2")),
    )
    clone_parser.add_argument("source", help="Source image name")
    clone_parser.add_argument("target", help="Target image name")
    clone_parser.add_argument("--force", action="store_true", help="Overwrite if target exists")

    # image shell
    shell_parser = image_subparsers.add_parser(
        "shell",
        help="Deprecated: interactive shell into a stored image",
        **examples(("Open a shell in a stored image (deprecated)", "cb image shell my-linux")),
    )
    shell_parser.add_argument("name", help="Image name")
    shell_parser.add_argument(
        "--writable", action="store_true", help="Modify golden image directly (dangerous!)"
    )
    shell_parser.add_argument("--detach", "-d", action="store_true", help="Run in background")
    shell_parser.add_argument(
        "--vnc-port", dest="vnc_port", help="VNC port (default: auto-allocate from 8006)"
    )
    shell_parser.add_argument(
        "--api-port", dest="api_port", help="API port (default: auto-allocate from 5000)"
    )
    shell_parser.add_argument("--memory", default="8G", help="Memory (default: 8G)")
    shell_parser.add_argument("--cpus", default="8", help="CPU cores (default: 8)")
    shell_parser.add_argument(
        "--no-kvm", action="store_true", dest="no_kvm", help="Disable KVM acceleration"
    )

    image_parser.set_defaults(image_command="list")


def execute(args) -> int:
    """Execute the image command."""
    cmd = getattr(args, "image_command", "list") or "list"
    if cmd == "info":
        return cmd_info(args)
    if cmd in ("create", "delete", "clone", "shell"):
        return _deprecated(args)
    return cmd_list(args)
