"""Image management commands for CUA CLI.

Handles both cloud images (push/pull to CUA cloud) and local images
(create, clone, shell, info).
"""

import argparse
from datetime import datetime
from pathlib import Path
from typing import Any

import aiohttp
from cua_cli.api.client import CloudAPIClient, calculate_file_hash, format_bytes
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import (
    print_error,
    print_info,
    print_json,
    print_success,
    print_table,
)

# Default local image storage
LOCAL_IMAGES_DIR = Path.home() / ".local" / "share" / "cua" / "images"

# Default part size for multi-part upload: 100MB
DEFAULT_PART_SIZE = 100 * 1024 * 1024


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the image command and subcommands."""
    for cmd_name in ("image", "img"):
        img_parser = subparsers.add_parser(
            cmd_name,
            help="Image management commands",
            description="Manage VM images (cloud and local)",
        )

        img_subparsers = img_parser.add_subparsers(
            dest="image_command",
            help="Image command",
        )

        # list command
        list_parser = img_subparsers.add_parser(
            "list",
            aliases=["ls"],
            help="List images",
        )
        list_parser.add_argument(
            "--json",
            action="store_true",
            help="Output as JSON",
        )
        list_parser.add_argument(
            "--local",
            action="store_true",
            help="List local images only",
        )
        list_parser.add_argument(
            "--cloud",
            action="store_true",
            help="List cloud images only (default)",
        )
        list_parser.add_argument(
            "--platform",
            type=str,
            help="Filter local images by platform",
        )
        list_parser.add_argument(
            "--format",
            choices=["table", "json"],
            default="table",
            help="Output format for local images",
        )

        # push command
        push_parser = img_subparsers.add_parser(
            "push",
            help="Push image to cloud storage",
        )
        push_parser.add_argument(
            "name",
            help="Image name",
        )
        push_parser.add_argument(
            "--file",
            "-f",
            type=str,
            help="Path to image file (default: ~/.local/share/cua/images/<name>/data.img)",
        )
        push_parser.add_argument(
            "--tag",
            type=str,
            default="latest",
            help="Image tag (default: latest)",
        )
        push_parser.add_argument(
            "--type",
            type=str,
            default="qcow2",
            choices=["qcow2", "raw", "vmdk"],
            help="Image type (default: qcow2)",
        )

        # pull command
        pull_parser = img_subparsers.add_parser(
            "pull",
            help="Pull image from cloud storage",
        )
        pull_parser.add_argument(
            "name",
            help="Image name",
        )
        pull_parser.add_argument(
            "--tag",
            type=str,
            default="latest",
            help="Image tag (default: latest)",
        )
        pull_parser.add_argument(
            "--output",
            "-o",
            type=str,
            help="Output file path",
        )

        # delete command
        delete_parser = img_subparsers.add_parser(
            "delete",
            help="Delete an image",
        )
        delete_parser.add_argument(
            "name",
            help="Image name",
        )
        delete_parser.add_argument(
            "--tag",
            type=str,
            default="latest",
            help="Image tag (default: latest, for cloud images)",
        )
        delete_parser.add_argument(
            "--force",
            action="store_true",
            help="Skip confirmation",
        )
        delete_parser.add_argument(
            "--local",
            action="store_true",
            help="Delete a local image instead of cloud",
        )

        # create command (local)
        create_parser = img_subparsers.add_parser(
            "create",
            help="Create a local image from platform",
        )
        create_parser.add_argument(
            "platform",
            help="Platform name (e.g., linux-docker, windows-qemu)",
        )
        create_parser.add_argument("--name", help="Image name (default: same as platform)")
        create_parser.add_argument("--iso", help="Path to ISO file (for QEMU platforms)")
        create_parser.add_argument(
            "--download-iso",
            action="store_true",
            dest="download_iso",
            help="Download Windows 11 ISO (~6GB)",
        )
        create_parser.add_argument(
            "--docker-image", dest="docker_image", help="Override Docker image"
        )
        create_parser.add_argument(
            "--distro",
            default="ubuntu",
            choices=["ubuntu", "fedora"],
            help="Linux distribution",
        )
        create_parser.add_argument(
            "--version",
            default="14",
            help="OS version (e.g., 14 for Android, sonoma for macOS)",
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
            "--skip-pull",
            action="store_true",
            dest="skip_pull",
            help="Don't pull Docker image",
        )
        create_parser.add_argument(
            "--no-kvm",
            action="store_true",
            dest="no_kvm",
            help="Disable KVM acceleration",
        )
        create_parser.add_argument(
            "--vnc-port",
            dest="vnc_port",
            help="VNC port (default: auto-allocate from 8006)",
        )
        create_parser.add_argument(
            "--api-port",
            dest="api_port",
            help="API port (default: auto-allocate from 5000)",
        )

        # info command (local)
        info_parser = img_subparsers.add_parser(
            "info",
            help="Show local image details",
        )
        info_parser.add_argument("name", help="Image name")

        # clone command (local)
        clone_parser = img_subparsers.add_parser(
            "clone",
            help="Clone a local image",
        )
        clone_parser.add_argument("source", help="Source image name")
        clone_parser.add_argument("target", help="Target image name")
        clone_parser.add_argument("--force", action="store_true", help="Overwrite if target exists")

        # shell command (local)
        shell_parser = img_subparsers.add_parser(
            "shell",
            help="Interactive shell into image (uses overlay by default)",
        )
        shell_parser.add_argument("name", help="Image name")
        shell_parser.add_argument(
            "--writable",
            action="store_true",
            help="Modify golden image directly (dangerous!)",
        )
        shell_parser.add_argument("--detach", "-d", action="store_true", help="Run in background")
        shell_parser.add_argument(
            "--vnc-port",
            dest="vnc_port",
            help="VNC port (default: auto-allocate from 8006)",
        )
        shell_parser.add_argument(
            "--api-port",
            dest="api_port",
            help="API port (default: auto-allocate from 5000)",
        )
        shell_parser.add_argument("--memory", default="8G", help="Memory (default: 8G)")
        shell_parser.add_argument("--cpus", default="8", help="CPU cores (default: 8)")
        shell_parser.add_argument(
            "--no-kvm",
            action="store_true",
            dest="no_kvm",
            help="Disable KVM acceleration",
        )


def execute(args: argparse.Namespace) -> int:
    """Execute image command based on subcommand."""
    from cua_cli.commands import local_image

    cmd = getattr(args, "image_command", None)

    if cmd in ("list", "ls"):
        return cmd_list(args)
    elif cmd == "push":
        return cmd_push(args)
    elif cmd == "pull":
        return cmd_pull(args)
    elif cmd == "delete":
        return cmd_delete(args)
    elif cmd == "create":
        return local_image.cmd_create(args)
    elif cmd == "info":
        return local_image.cmd_info(args)
    elif cmd == "clone":
        return local_image.cmd_clone(args)
    elif cmd == "shell":
        return local_image.cmd_shell(args)
    else:
        print_error("Usage: cua image <command>")
        print_info("Commands: list, push, pull, delete, create, info, clone, shell")
        return 1


def cmd_list(args: argparse.Namespace) -> int:
    """List images."""
    show_local = getattr(args, "local", False)
    show_cloud = getattr(args, "cloud", False)

    # If --local is set, delegate to local_image list
    if show_local and not show_cloud:
        from cua_cli.commands import local_image

        return local_image.cmd_local_list(args)

    # Default to cloud if neither specified, or if --cloud is set
    if not show_local:
        show_cloud = True

    all_images = []

    if show_cloud:
        cloud_images = run_async(_list_cloud_images())
        all_images.extend(cloud_images)

    if show_local:
        local_images = _list_local_images()
        all_images.extend(local_images)

    if getattr(args, "json", False):
        print_json(all_images)
        return 0

    if not all_images:
        print_info("No images found.")
        return 0

    columns = [
        ("name", "NAME"),
        ("type", "TYPE"),
        ("tag", "TAG"),
        ("size", "SIZE"),
        ("status", "STATUS"),
        ("created", "CREATED"),
    ]

    print_table(all_images, columns)
    return 0


async def _list_cloud_images() -> list[dict[str, Any]]:
    """List cloud images."""
    try:
        client = CloudAPIClient()
        images = await client.list_images()

        result = []
        for img in images:
            versions = img.get("versions", [])
            if versions:
                for ver in versions:
                    result.append(
                        {
                            "name": img.get("name", ""),
                            "type": img.get("image_type", ""),
                            "tag": ver.get("tag", ""),
                            "size": format_bytes(ver.get("size_bytes", 0)),
                            "status": ver.get("status", ""),
                            "created": _format_date(ver.get("created_at", "")),
                            "source": "cloud",
                        }
                    )
            else:
                result.append(
                    {
                        "name": img.get("name", ""),
                        "type": img.get("image_type", ""),
                        "tag": "-",
                        "size": "-",
                        "status": "-",
                        "created": _format_date(img.get("created_at", "")),
                        "source": "cloud",
                    }
                )
        return result
    except Exception as e:
        print_error(f"Failed to list cloud images: {e}")
        return []


def _list_local_images() -> list[dict[str, Any]]:
    """List local images."""
    result = []

    if not LOCAL_IMAGES_DIR.exists():
        return result

    for image_dir in LOCAL_IMAGES_DIR.iterdir():
        if not image_dir.is_dir():
            continue

        name = image_dir.name
        data_file = image_dir / "data.img"

        if data_file.exists():
            size = format_bytes(data_file.stat().st_size)
            created = datetime.fromtimestamp(data_file.stat().st_mtime).strftime("%Y-%m-%d")
        else:
            size = "-"
            created = "-"

        result.append(
            {
                "name": name,
                "type": "local",
                "tag": "latest",
                "size": size,
                "status": "ready" if data_file.exists() else "incomplete",
                "created": created,
                "source": "local",
            }
        )

    return result


def _format_date(date_str: str) -> str:
    """Format a date string."""
    if not date_str:
        return "-"
    try:
        dt = datetime.fromisoformat(date_str.replace("Z", "+00:00"))
        return dt.strftime("%Y-%m-%d")
    except Exception:
        return date_str[:10] if len(date_str) >= 10 else date_str


def cmd_push(args: argparse.Namespace) -> int:
    """Push an image to cloud storage."""
    name = args.name
    tag = args.tag
    image_type = args.type

    # Determine file path
    if args.file:
        file_path = Path(args.file)
    else:
        # Look in local images directory
        file_path = LOCAL_IMAGES_DIR / name / "data.img"

    if not file_path.exists():
        if args.file:
            print_error(f"File not found: {file_path}")
        else:
            print_error(f"Image not found: {name}")
            print_info(f"Looked in: {file_path}")
            print_info("Use --file to specify a custom path")
        return 1

    size_bytes = file_path.stat().st_size
    print_info(f"Pushing {file_path} ({format_bytes(size_bytes)})")

    return run_async(_push_image(name, tag, image_type, file_path, size_bytes))


async def _push_image(
    name: str, tag: str, image_type: str, file_path: Path, size_bytes: int
) -> int:
    """Push an image using multi-part upload."""
    from rich.progress import BarColumn, Progress, SpinnerColumn, TextColumn

    # Calculate checksum
    print_info("Calculating checksum...")
    checksum = calculate_file_hash(file_path)
    print_info(f"Checksum: {checksum}")

    client = CloudAPIClient()

    # Initiate upload
    print_info("Initiating upload...")
    status, data = await client.initiate_upload(
        name=name,
        tag=tag,
        image_type=image_type,
        size_bytes=size_bytes,
        checksum_sha256=checksum,
    )

    if status == 401:
        print_error("Unauthorized. Run 'cua auth login' again.")
        return 1
    if status == 409:
        print_error(f"Image version already exists: {name}:{tag}")
        return 1
    if status not in (200, 201):
        print_error(f"Failed to initiate upload: {data}")
        return 1

    upload_id = data.get("upload_id")
    part_size = data.get("part_size", DEFAULT_PART_SIZE)
    total_parts = data.get("total_parts", 1)

    print_info(f"Upload session: {upload_id}")
    print_info(f"Parts: {total_parts} x {format_bytes(part_size)}")

    # Read file into memory (for simplicity - could stream for very large files)
    file_data = file_path.read_bytes()

    completed_parts = []

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TextColumn("[progress.percentage]{task.percentage:>3.0f}%"),
    ) as progress:
        task = progress.add_task("Uploading...", total=total_parts)

        for part_num in range(1, total_parts + 1):
            # Get signed URL
            status, url_data = await client.get_upload_part_url(name, upload_id, part_num)
            if status != 200:
                print_error(f"Failed to get upload URL for part {part_num}")
                await client.abort_upload(name, upload_id)
                return 1

            upload_url = url_data.get("upload_url")

            # Calculate part range
            start = (part_num - 1) * part_size
            end = min(start + part_size, size_bytes)
            part_data = file_data[start:end]

            # Upload part
            async with aiohttp.ClientSession() as session:
                async with session.put(
                    upload_url,
                    data=part_data,
                    headers={"Content-Type": "application/octet-stream"},
                ) as resp:
                    if resp.status not in (200, 201):
                        print_error(f"Failed to upload part {part_num}: {resp.status}")
                        await client.abort_upload(name, upload_id)
                        return 1

                    etag = resp.headers.get("ETag", "")
                    completed_parts.append({"part_number": part_num, "etag": etag})

            progress.update(task, advance=1)

    # Complete upload
    print_info("Completing upload...")
    status, result = await client.complete_upload(name, upload_id, completed_parts)

    if status not in (200, 201):
        print_error(f"Failed to complete upload: {result}")
        return 1

    print_success(f"Push complete: {name}:{tag}")
    if isinstance(result, dict):
        print_info(f"Version ID: {result.get('version_id', 'N/A')}")
        print_info(f"Status: {result.get('status', 'N/A')}")

    return 0


def cmd_pull(args: argparse.Namespace) -> int:
    """Pull an image from cloud storage."""
    name = args.name
    tag = args.tag
    output_path = args.output or f"{name}-{tag}.qcow2"

    print_info(f"Pulling {name}:{tag}...")
    return run_async(_pull_image(name, tag, Path(output_path)))


async def _pull_image(name: str, tag: str, output_path: Path) -> int:
    """Pull an image from cloud storage."""
    from rich.progress import (
        BarColumn,
        DownloadColumn,
        Progress,
        SpinnerColumn,
        TextColumn,
    )

    client = CloudAPIClient()

    # Get download URL
    status, data = await client.get_download_url(name, tag)

    if status == 401:
        print_error("Unauthorized. Run 'cua auth login' again.")
        return 1
    if status == 404:
        print_error(f"Image not found: {name}:{tag}")
        return 1
    if status != 200:
        print_error(f"Failed to get download URL: {data}")
        return 1

    download_url = data.get("download_url")
    size_bytes = data.get("size_bytes", 0)
    expected_checksum = data.get("checksum_sha256", "")

    print_info(f"Size: {format_bytes(size_bytes)}")
    print_info(f"Downloading to {output_path}...")

    # Download file
    async with aiohttp.ClientSession() as session:
        async with session.get(download_url) as resp:
            if resp.status != 200:
                print_error(f"Download failed: {resp.status}")
                return 1

            with Progress(
                SpinnerColumn(),
                TextColumn("[progress.description]{task.description}"),
                BarColumn(),
                DownloadColumn(),
            ) as progress:
                task = progress.add_task("Downloading...", total=size_bytes)

                with open(output_path, "wb") as f:
                    async for chunk in resp.content.iter_chunked(8192):
                        f.write(chunk)
                        progress.update(task, advance=len(chunk))

    # Verify checksum
    if expected_checksum:
        print_info("Verifying checksum...")
        downloaded_checksum = calculate_file_hash(output_path)

        if downloaded_checksum != expected_checksum:
            print_error("Checksum mismatch! Download may be corrupted.")
            print_error(f"Expected: {expected_checksum}")
            print_error(f"Got: {downloaded_checksum}")
            return 1

        print_success(f"Checksum verified: {downloaded_checksum}")

    print_success(f"Pull complete: {output_path}")
    return 0


def cmd_delete(args: argparse.Namespace) -> int:
    """Delete an image."""
    # If --local flag, delegate to local_image delete
    if getattr(args, "local", False):
        from cua_cli.commands import local_image

        return local_image.cmd_local_delete(args)

    # Cloud delete
    name = args.name
    tag = args.tag

    if not args.force:
        print_info(f"This will delete {name}:{tag}. Use --force to confirm.")
        return 1

    print_info(f"Deleting {name}:{tag}...")
    return run_async(_delete_image(name, tag))


async def _delete_image(name: str, tag: str) -> int:
    """Delete an image from cloud storage."""
    client = CloudAPIClient()

    status, data = await client.delete_image(name, tag)

    if status == 401:
        print_error("Unauthorized. Run 'cua auth login' again.")
        return 1
    if status == 404:
        print_error(f"Image not found: {name}:{tag}")
        return 1
    if status not in (200, 202, 204):
        print_error(f"Delete failed: {data}")
        return 1

    print_success(f"Deleted: {name}:{tag}")
    return 0
