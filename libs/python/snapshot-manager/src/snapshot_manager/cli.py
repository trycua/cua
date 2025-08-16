"""
Command-line interface for the snapshot manager.

Provides a convenient CLI for testing and managing snapshots outside
of the agent SDK integration.
"""

import asyncio
import json
import logging
import sys

import click

from .manager import SnapshotManager
from .models import RestoreOptions, SnapshotConfig, SnapshotTrigger

# Configure logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)


@click.group()
@click.option("--config", type=click.Path(exists=True), help="Configuration file path")
@click.option("--storage-path", default=".snapshots", help="Base path for snapshot storage")
@click.option("--verbose", "-v", is_flag=True, help="Enable verbose logging")
@click.pass_context
def main(ctx, config, storage_path, verbose):
    """CUA Snapshot Manager CLI - Manage container snapshots for the Cua Agent SDK."""

    if verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    # Load configuration
    if config:
        try:
            with open(config, "r") as f:
                config_data = json.load(f)
                # CLI storage_path takes precedence over config file
                if storage_path != ".snapshots":  # Only override if explicitly set
                    config_data["storage_path"] = storage_path
                snapshot_config = SnapshotConfig(**config_data)
        except Exception as e:
            click.echo(f"Error loading config: {e}", err=True)
            sys.exit(1)
    else:
        snapshot_config = SnapshotConfig(storage_path=storage_path)

    # Initialize snapshot manager
    from .providers import DockerSnapshotProvider
    from .storage import FileSystemSnapshotStorage

    storage = FileSystemSnapshotStorage(base_path=snapshot_config.storage_path)
    provider = DockerSnapshotProvider()
    manager = SnapshotManager(provider=provider, storage=storage, config=snapshot_config)

    # Store in context for subcommands
    ctx.ensure_object(dict)
    ctx.obj["manager"] = manager
    ctx.obj["config"] = snapshot_config


@main.command()
@click.argument("container_id")
@click.option(
    "--trigger",
    type=click.Choice([t.value for t in SnapshotTrigger]),
    default="manual",
    help="Snapshot trigger type",
)
@click.option("--description", help="Description for the snapshot")
@click.option("--context", help="Action context")
@click.option("--run-id", help="Agent run ID")
@click.pass_context
def create(ctx, container_id, trigger, description, context, run_id):
    """Create a snapshot of a container."""

    async def _create():
        manager = ctx.obj["manager"]

        try:
            metadata = await manager.create_snapshot(
                container_id=container_id,
                trigger=SnapshotTrigger(trigger),
                description=description,
                action_context=context,
                run_id=run_id,
            )

            click.echo(f"✅ Created snapshot: {metadata.snapshot_id}")
            click.echo(f"   Container: {metadata.container_name}")
            click.echo(f"   Trigger: {metadata.trigger.value}")
            click.echo(
                f"   Size: {metadata.size_bytes / (1024*1024):.1f} MB"
                if metadata.size_bytes
                else "   Size: Unknown"
            )
            click.echo(f"   Image: {metadata.image_tag}")

        except Exception as e:
            click.echo(f"❌ Error creating snapshot: {e}", err=True)
            sys.exit(1)

    asyncio.run(_create())


@main.command()
@click.option("--container", help="Filter by container ID")
@click.option("--limit", type=int, help="Limit number of results")
@click.option("--json-output", is_flag=True, help="Output as JSON")
@click.pass_context
def list(ctx, container, limit, json_output):
    """List snapshots."""

    async def _list():
        manager = ctx.obj["manager"]

        try:
            snapshots = await manager.list_snapshots(container_id=container, limit=limit)

            if json_output:
                # Convert to JSON-serializable format
                data = []
                for snapshot in snapshots:
                    snapshot_dict = snapshot.model_dump()
                    snapshot_dict["timestamp"] = snapshot.timestamp.isoformat()
                    data.append(snapshot_dict)
                click.echo(json.dumps(data, indent=2))
            else:
                if not snapshots:
                    click.echo("No snapshots found.")
                    return

                click.echo(f"Found {len(snapshots)} snapshot(s):")
                click.echo()

                for snapshot in snapshots:
                    size_mb = snapshot.size_bytes / (1024 * 1024) if snapshot.size_bytes else 0
                    click.echo(f"📸 {snapshot.snapshot_id}")
                    click.echo(
                        f"   Container: {snapshot.container_name} ({snapshot.container_id[:12]})"
                    )
                    click.echo(f"   Created: {snapshot.timestamp.strftime('%Y-%m-%d %H:%M:%S')}")
                    click.echo(f"   Trigger: {snapshot.trigger.value}")
                    click.echo(f"   Size: {size_mb:.1f} MB")
                    click.echo(f"   Status: {snapshot.status.value}")
                    if snapshot.description:
                        click.echo(f"   Description: {snapshot.description}")
                    click.echo()

        except Exception as e:
            click.echo(f"❌ Error listing snapshots: {e}", err=True)
            sys.exit(1)

    asyncio.run(_list())


@main.command()
@click.argument("snapshot_id")
@click.option("--container-name", help="Name for the restored container")
@click.option(
    "--preserve-networks/--no-preserve-networks", default=True, help="Preserve network connections"
)
@click.option(
    "--preserve-volumes/--no-preserve-volumes", default=True, help="Preserve volume mounts"
)
@click.pass_context
def restore(ctx, snapshot_id, container_name, preserve_networks, preserve_volumes):
    """Restore a container from a snapshot."""

    async def _restore():
        manager = ctx.obj["manager"]

        try:
            options = RestoreOptions(
                new_container_name=container_name,
                preserve_networks=preserve_networks,
                preserve_volumes=preserve_volumes,
            )

            container_id = await manager.restore_snapshot(snapshot_id, options)

            click.echo(f"✅ Restored container: {container_id}")

        except Exception as e:
            click.echo(f"❌ Error restoring snapshot: {e}", err=True)
            sys.exit(1)

    asyncio.run(_restore())


@main.command()
@click.argument("snapshot_id")
@click.option("--force", is_flag=True, help="Force deletion without confirmation")
@click.pass_context
def delete(ctx, snapshot_id, force):
    """Delete a snapshot."""

    async def _delete():
        manager = ctx.obj["manager"]

        try:
            if not force:
                # Get snapshot info for confirmation
                metadata = await manager.get_snapshot(snapshot_id)
                if metadata:
                    click.echo("About to delete snapshot:")
                    click.echo(f"  ID: {metadata.snapshot_id}")
                    click.echo(f"  Container: {metadata.container_name}")
                    click.echo(f"  Created: {metadata.timestamp}")
                    click.echo(
                        f"  Size: {metadata.size_bytes / (1024*1024):.1f} MB"
                        if metadata.size_bytes
                        else "  Size: Unknown"
                    )

                    if not click.confirm("Are you sure you want to delete this snapshot?"):
                        click.echo("Deletion cancelled.")
                        return

            await manager.delete_snapshot(snapshot_id)
            click.echo(f"✅ Deleted snapshot: {snapshot_id}")

        except Exception as e:
            click.echo(f"❌ Error deleting snapshot: {e}", err=True)
            sys.exit(1)

    asyncio.run(_delete())


@main.command()
@click.option("--max-age-days", type=int, help="Maximum age in days for snapshots to keep")
@click.option(
    "--dry-run", is_flag=True, help="Show what would be deleted without actually deleting"
)
@click.pass_context
def cleanup(ctx, max_age_days, dry_run):
    """Clean up old snapshots."""

    async def _cleanup():
        manager = ctx.obj["manager"]

        try:
            if dry_run:
                click.echo("🔍 Dry run mode not implemented yet")
                return
            else:
                count = await manager.cleanup_old_snapshots(max_age_days)
                click.echo(f"✅ Cleaned up {count} old snapshots")

        except Exception as e:
            click.echo(f"❌ Error during cleanup: {e}", err=True)
            sys.exit(1)

    asyncio.run(_cleanup())


@main.command()
@click.pass_context
def stats(ctx):
    """Show storage statistics."""

    async def _stats():
        manager = ctx.obj["manager"]

        try:
            stats = await manager.get_storage_stats()

            click.echo("📊 Snapshot Storage Statistics:")
            click.echo(f"   Total snapshots: {stats['total_snapshots']}")
            click.echo(f"   Total containers: {stats['total_containers']}")
            click.echo(f"   Total size: {stats['total_size_gb']:.2f} GB")
            click.echo(f"   Metadata size: {stats['metadata_size_bytes'] / 1024:.1f} KB")
            click.echo(f"   Storage path: {stats['storage_path']}")
            if stats.get("last_updated"):
                click.echo(f"   Last updated: {stats['last_updated']}")

        except Exception as e:
            click.echo(f"❌ Error getting stats: {e}", err=True)
            sys.exit(1)

    asyncio.run(_stats())


@main.command()
@click.argument("container_id")
@click.pass_context
def validate(ctx, container_id):
    """Validate that a container can be snapshotted."""

    async def _validate():
        manager = ctx.obj["manager"]

        try:
            is_valid = await manager.provider.validate_container(container_id)

            if is_valid:
                click.echo(f"✅ Container {container_id} is valid for snapshotting")
            else:
                click.echo(f"❌ Container {container_id} is not valid for snapshotting")
                sys.exit(1)

        except Exception as e:
            click.echo(f"❌ Error validating container: {e}", err=True)
            sys.exit(1)

    asyncio.run(_validate())


if __name__ == "__main__":
    main()
