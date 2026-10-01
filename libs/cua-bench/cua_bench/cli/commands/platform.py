"""``cb platform`` (deprecated): the canonical images, by OS.

The old platform table (linux-docker, windows-qemu, ...) is replaced by the
canonical ``ghcr.io/trycua/<os>`` images and ``--kind container|vm``.
Old platform names still resolve to their OS.
"""

from __future__ import annotations
from ._help import examples

import json

from ._canonical import LEGACY_PLATFORMS, MIGRATION, canonical_images, find

RESET = "\033[0m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def cmd_list(args) -> int:
    print(f"{YELLOW}cb platform is deprecated: use cb image list.{RESET}")
    rows = canonical_images()
    if getattr(args, "format", "table") == "json":
        print(json.dumps(rows, indent=2))
        return 0
    for row in rows:
        print(f"  {row['name']:<9} {row['image']:<34} kinds: {'/'.join(row['kinds'])}")
    return 0


def cmd_info(args) -> int:
    print(f"{YELLOW}cb platform is deprecated: use cb image info.{RESET}")
    row = find(args.platform)
    if row is None:
        print(f"{RED}Unknown platform {args.platform!r}.{RESET}")
        print(f"{GREY}Known: {', '.join(sorted(LEGACY_PLATFORMS))}. {MIGRATION}{RESET}")
        return 1
    for key in ("name", "image", "kinds", "on", "variants"):
        value = row[key]
        print(f"{key + ':':<10} {'/'.join(value) if isinstance(value, list) else value}")
    return 0


def register_parser(subparsers):
    """Register the platform command with the main CLI parser."""
    platform_parser = subparsers.add_parser(
        "platform",
        help="Show available platform configurations",
        **examples(
            ("List platforms", "cb platform"),
            ("Show one platform", "cb platform info linux-docker"),
        ),
    )
    platform_subparsers = platform_parser.add_subparsers(
        dest="platform_command", help="Platform command"
    )

    # platform list
    list_parser = platform_subparsers.add_parser(
        "list",
        help="List all available platforms",
        **examples(
            ("List platforms", "cb platform list"), ("As JSON", "cb platform list --format json")
        ),
    )
    list_parser.add_argument(
        "--format", choices=["table", "json"], default="table", help="Output format"
    )

    # platform info
    info_parser = platform_subparsers.add_parser(
        "info",
        help="Show platform details",
        **examples(("Show one platform", "cb platform info windows-qemu")),
    )
    info_parser.add_argument("platform", help="Platform name (e.g., linux-docker, windows-qemu)")

    platform_parser.set_defaults(platform_command="list")


def execute(args) -> int:
    """Execute the platform command."""
    if getattr(args, "platform_command", "list") == "info":
        return cmd_info(args)
    return cmd_list(args)
