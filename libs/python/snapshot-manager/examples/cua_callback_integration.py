"""
CUA Callback Integration Test

Tests DockerSnapshotProvider wired to CUA callbacks with automatic snapshot creation.
Tests all snapshot triggers: RUN_START, RUN_END, BEFORE_ACTION, AFTER_ACTION.
"""

import asyncio
import logging
import os
import tempfile
import uuid
from pathlib import Path

from dotenv import load_dotenv
from agent import ComputerAgent
from computer import Computer

from snapshot_manager import (
    SnapshotManager,
    SnapshotCallback,
    SnapshotConfig,
    SnapshotTrigger,
)
from snapshot_manager.providers.docker_provider import DockerSnapshotProvider
from snapshot_manager.storage import FileSystemSnapshotStorage


async def test_all_callback_triggers():
    """Test all CUA callback triggers with minimal API usage."""
    load_dotenv()
    logging.basicConfig(level=logging.WARNING)

    print("CUA Snapshot Manager - Complete Callback Integration Test")
    print("=" * 60)

    # Setup storage path
    storage_path = Path(tempfile.gettempdir()) / "cua-callback-test"
    print(f"Storage path: {storage_path}")
    storage_path.mkdir(exist_ok=True)

    # Test ALL triggers
    config = SnapshotConfig(
        triggers=[
            SnapshotTrigger.RUN_START,
            SnapshotTrigger.RUN_END,
            SnapshotTrigger.BEFORE_ACTION,
            SnapshotTrigger.AFTER_ACTION,
        ],
        storage_path=str(storage_path),
        max_snapshots_per_container=20,
    )

    # Explicit DockerSnapshotProvider wiring
    docker_provider = DockerSnapshotProvider()
    storage = FileSystemSnapshotStorage(base_path=str(storage_path))
    snapshot_manager = SnapshotManager(config=config, provider=docker_provider, storage=storage)

    container_name = f"cua-test-{uuid.uuid4().hex[:8]}"

    # Container resolver - get actual container ID from Docker
    def get_container_id(kwargs):
        return container_name  # Use container name consistently

    # Setup callback
    snapshot_callback = SnapshotCallback(
        snapshot_manager=snapshot_manager,
        container_resolver=get_container_id,
    )

    print(f"Testing all triggers with container: {container_name}")

    try:
        async with Computer(
            os_type="linux", provider_type="docker", name=container_name
        ) as computer:
            print("CUA Computer connected")

            # Create agent with snapshot callback
            agent = ComputerAgent(
                model="anthropic/claude-3-7-sonnet-20250219",
                tools=[computer],
                callbacks=[snapshot_callback],
                verbosity=logging.ERROR,
            )

            # Verify container can be snapshotted
            is_valid = await docker_provider.validate_container(container_name)
            if not is_valid:
                print("ERROR: Container not valid for snapshotting")
                return False

            print("Running minimal task to trigger all callbacks...")

            # Simple task that will trigger BEFORE_ACTION and AFTER_ACTION
            task = "Take a screenshot"

            action_count = 0
            async for result in agent.run(task):
                if result.get("output"):
                    for item in result["output"]:
                        if item["type"] == "computer_call":
                            action_count += 1

            print(f"Task completed with {action_count} actions")

            # Get all snapshots and find ones for our container
            await asyncio.sleep(1)  # Brief pause for snapshot completion
            all_snapshots = await snapshot_manager.list_snapshots()

            # Find snapshots for our container by checking container name in description/tags
            our_snapshots = []
            for snap in all_snapshots:
                # Check if this snapshot is related to our container
                if (
                    container_name in snap.container_id
                    or container_name in snap.container_name
                    or container_name in str(snap.description)
                    or container_name in str(snap.image_tag)
                ):
                    our_snapshots.append(snap)

            print(f"\nSnapshots found for {container_name}: {len(our_snapshots)}")

            # Show what we found
            for snap in our_snapshots:
                print(f"  - {snap.trigger.value} at {snap.timestamp.strftime('%H:%M:%S')}")

            # Check if we got all expected triggers
            expected = {"run_start", "run_end", "before_action", "after_action"}
            found = {snap.trigger.value for snap in our_snapshots}

            print("\nTrigger Summary:")
            for trigger in expected:
                status = "✅" if trigger in found else "❌"
                print(f"  {status} {trigger.upper()}")

            # Success if all triggers found
            success = expected.issubset(found)
            if success:
                print("\nSUCCESS: All callback triggers working!")
                print(f"- {len(our_snapshots)} snapshots created")
                print("- DockerSnapshotProvider correctly wired to CUA callbacks")
            else:
                print(f"\nMissing triggers: {expected - found}")

            return success

    except Exception as e:
        print(f"ERROR: Test failed: {e}")
        return False


def main():
    """Run the complete callback integration test."""
    if not os.getenv("ANTHROPIC_API_KEY"):
        print("ERROR: ANTHROPIC_API_KEY environment variable required")
        return False

    success = asyncio.run(test_all_callback_triggers())

    if success:
        print("\nTEST PASSED: Callback integration working correctly")
    else:
        print("\nTEST FAILED: Callback integration needs fixing")

    return success


if __name__ == "__main__":
    main()
