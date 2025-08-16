"""
Basic usage examples for the CUA Snapshot Manager.

This script demonstrates the core functionality of the snapshot system
without requiring the full CUA Agent SDK.
"""

import asyncio
import logging

import docker

from snapshot_manager import SnapshotConfig, SnapshotManager, SnapshotTrigger

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def basic_snapshot_operations():
    """Demonstrate basic snapshot operations."""

    print("🚀 CUA Snapshot Manager - Basic Usage Example")
    print("=" * 50)

    # Initialize the snapshot manager
    config = SnapshotConfig(storage_path="./example_snapshots", max_snapshots_per_container=5)

    manager = SnapshotManager(config=config)

    # Check if Docker is available
    try:
        docker_client = docker.from_env()
        docker_client.ping()
        print("✅ Docker connection successful")
    except Exception as e:
        print(f"❌ Docker not available: {e}")
        print("Please ensure Docker Desktop is running and try again.")
        return

    # List running containers
    containers = docker_client.containers.list()
    if not containers:
        print("⚠️  No running containers found.")
        print("Starting a test container for demonstration...")

        # Start a simple test container
        try:
            container = docker_client.containers.run(
                "alpine:latest", command="sleep 300", name="cua_snapshot_test", detach=True
            )
            print(f"✅ Started test container: {container.name}")
            target_container = container.name
        except Exception as e:
            print(f"❌ Failed to start test container: {e}")
            return
    else:
        # Use the first running container
        target_container = containers[0].name
        print(f"📦 Using existing container: {target_container}")

    try:
        # Validate the container
        print(f"\n1. Validating container '{target_container}'...")
        is_valid = await manager.provider.validate_container(target_container)
        if is_valid:
            print("✅ Container is valid for snapshotting")
        else:
            print("❌ Container is not valid for snapshotting")
            return

        # Create a manual snapshot
        print("\n2. Creating a manual snapshot...")
        snapshot_metadata = await manager.create_snapshot(
            container_id=target_container,
            trigger=SnapshotTrigger.MANUAL,
            description="Example manual snapshot",
            action_context="basic_usage_demo",
        )
        print(f"✅ Created snapshot: {snapshot_metadata.snapshot_id}")
        print(f"   Image tag: {snapshot_metadata.image_tag}")
        print(f"   Size: {snapshot_metadata.size_bytes / (1024*1024):.1f} MB")

        # Create another snapshot with different trigger
        print("\n3. Creating a run_start snapshot...")
        snapshot2_metadata = await manager.create_snapshot(
            container_id=target_container,
            trigger=SnapshotTrigger.RUN_START,
            description="Example run start snapshot",
            run_id="demo_run_001",
        )
        print(f"✅ Created snapshot: {snapshot2_metadata.snapshot_id}")

        # List all snapshots
        print("\n4. Listing all snapshots...")
        snapshots = await manager.list_snapshots()
        print(f"Found {len(snapshots)} snapshot(s):")
        for snapshot in snapshots:
            size_mb = snapshot.size_bytes / (1024 * 1024) if snapshot.size_bytes else 0
            print(f"   📸 {snapshot.snapshot_id}")
            print(f"      Container: {snapshot.container_name}")
            print(f"      Trigger: {snapshot.trigger.value}")
            print(f"      Created: {snapshot.timestamp.strftime('%Y-%m-%d %H:%M:%S')}")
            print(f"      Size: {size_mb:.1f} MB")
            print()

        # Demonstrate restoration
        print("5. Restoring from snapshot...")
        restored_container_id = await manager.restore_snapshot(
            snapshot_metadata.snapshot_id,
        )
        print(f"✅ Restored container: {restored_container_id}")

        # Get storage statistics
        print("\n6. Storage statistics...")
        stats = await manager.get_storage_stats()
        print(f"   Total snapshots: {stats['total_snapshots']}")
        print(f"   Total size: {stats['total_size_gb']:.2f} GB")
        print(f"   Storage path: {stats['storage_path']}")

        # Cleanup demonstration
        print("\n7. Cleaning up snapshots...")
        deleted_count = await manager.cleanup_old_snapshots(max_age_days=0)  # Delete all
        print(f"✅ Cleaned up {deleted_count} snapshots")

    except Exception as e:
        print(f"❌ Error during demonstration: {e}")
        logger.exception("Full error details:")

    finally:
        # Clean up test container if we created it
        try:
            if "container" in locals():
                container.stop()
                container.remove()
                print("🧹 Cleaned up test container")
        except Exception:
            pass

    print("\n✨ Demonstration complete!")


async def agent_integration_example():
    """Demonstrate how to integrate with CUA Agent SDK."""

    print("\n🤖 CUA Agent SDK Integration Example")
    print("=" * 40)

    from snapshot_manager import SnapshotCallback

    # Create snapshot callback
    config = SnapshotConfig(
        triggers=[SnapshotTrigger.RUN_START, SnapshotTrigger.RUN_END, SnapshotTrigger.AFTER_ACTION]
    )

    snapshot_callback = SnapshotCallback(config=config)

    # Simulate agent run lifecycle
    print("Simulating agent run lifecycle...")

    # Simulate run start
    run_kwargs = {"container_id": "demo_container", "run_id": "simulation_run_001"}

    await snapshot_callback.on_run_start(run_kwargs, [])
    print("📸 Run start snapshot triggered")

    # Simulate computer action
    action_item = {"action": {"type": "screenshot"}, "call_id": "action_001"}

    await snapshot_callback.on_computer_call_start(action_item)
    print("📸 Before action snapshot triggered")

    await snapshot_callback.on_computer_call_end(action_item, [])
    print("📸 After action snapshot triggered")

    # Simulate run end
    await snapshot_callback.on_run_end(run_kwargs, [], [])
    print("📸 Run end snapshot triggered")

    # Show current context
    context = snapshot_callback.get_current_context()
    print(f"\nAgent context: {context}")

    print("✅ Agent integration simulation complete")


async def advanced_configuration_example():
    """Demonstrate advanced configuration options."""

    print("\n⚙️  Advanced Configuration Example")
    print("=" * 35)

    # Create advanced configuration
    advanced_config = SnapshotConfig(
        # Triggers
        triggers=[
            SnapshotTrigger.RUN_START,
            SnapshotTrigger.AFTER_ACTION,
            SnapshotTrigger.ON_ERROR,
        ],
        # Storage limits
        storage_path="./advanced_snapshots",
        max_snapshots_per_container=3,
        max_total_snapshots=20,
        max_storage_size_gb=2.0,
        # Cleanup policy
        auto_cleanup_days=1,
        cleanup_on_exit=True,
        # Performance
        compression_enabled=True,
        parallel_operations=True,
        # Container configuration
        include_volumes=True,
        exclude_paths=["/tmp", "/var/tmp", "/proc", "/sys", "/dev"],
        # Naming
        naming_pattern="{container_name}_{trigger}_{timestamp}",
    )

    print("Configuration created with:")
    print(f"  - Triggers: {[t.value for t in advanced_config.triggers]}")
    print(f"  - Max snapshots per container: {advanced_config.max_snapshots_per_container}")
    print(f"  - Max storage size: {advanced_config.max_storage_size_gb} GB")
    print(f"  - Auto cleanup after: {advanced_config.auto_cleanup_days} days")
    print(f"  - Naming pattern: {advanced_config.naming_pattern}")

        # Create manager with advanced config
    SnapshotManager(config=advanced_config)
    
    print("✅ Advanced configuration applied")


if __name__ == "__main__":

    async def main():
        await basic_snapshot_operations()
        await agent_integration_example()
        await advanced_configuration_example()

    asyncio.run(main())
