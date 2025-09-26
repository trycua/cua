"""Example demonstrating snapshot restoration and running from a restored state."""

import asyncio
import logging
import traceback
import signal

from computer import Computer

# Import the unified agent class and types
from agent import ComputerAgent

from libs.python.agent.agent.callbacks.snapshot_manager import SnapshotManagerCallback

# Import utility functions
from utils import load_dotenv_files, handle_sigint

# Set up logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def list_and_display_snapshots(snapshot_callback):
    """List all available snapshots and display them."""
    print("\n=== Available Snapshots ===")

    snapshots = await snapshot_callback.list_snapshots()

    if not snapshots:
        print("No snapshots found!")
        return None

    print(f"Found {len(snapshots)} snapshots:")

    # Sort snapshots by creation time (newest first)
    sorted_snapshots = sorted(snapshots, key=lambda x: x.get('created', ''), reverse=True)

    for i, snap in enumerate(sorted_snapshots):
        metadata = snap.get('metadata', {})
        print(f"\n  {i+1}. Snapshot ID: {snap.get('id', 'unknown')}")
        print(f"     Tag: {snap.get('tag', 'unknown')}")
        print(f"     Created: {snap.get('created', 'unknown')}")
        print(f"     Trigger: {metadata.get('trigger', 'unknown')}")

        # Show run context if available
        run_context = metadata.get('run_context', {})
        if run_context:
            print(f"     Run ID: {run_context.get('run_id', 'N/A')}")
            print(f"     Action Count: {run_context.get('action_count', 0)}")

    return sorted_snapshots


async def restore_latest_snapshot(snapshot_callback, snapshots):
    """Restore the most recent snapshot."""
    if not snapshots:
        print("No snapshots available to restore")
        return None

    latest_snapshot = snapshots[0]  # Already sorted, first is newest
    snapshot_id = latest_snapshot.get('id')

    print(f"\n=== Restoring Latest Snapshot ===")
    print(f"Restoring to: {latest_snapshot.get('tag')}")
    print(f"Created: {latest_snapshot.get('created')}")

    result = await snapshot_callback.restore_snapshot(snapshot_id)

    if result.get('status') == 'error':
        print(f"❌ Failed to restore snapshot: {result.get('error')}")
        return None

    print(f"✅ Successfully restored snapshot: {snapshot_id}")
    return latest_snapshot


async def restore_specific_snapshot(snapshot_callback, snapshot_id, tag=None):
    """Restore a specific snapshot by ID."""
    print(f"\n=== Restoring Snapshot ===")
    if tag:
        print(f"Restoring to: {tag}")
    print(f"Snapshot ID: {snapshot_id}")

    result = await snapshot_callback.restore_snapshot(snapshot_id)

    if result.get('status') == 'error':
        print(f"❌ Failed to restore snapshot: {result.get('error')}")
        return None

    print(f"✅ Successfully restored snapshot: {snapshot_id}")
    return result


async def verify_restored_state(agent, history):
    """Verify the restored state by checking what files exist."""
    print("\n=== Verifying Restored State ===")

    verification_task = "List all files in the home directory to see what was restored"
    history.append({"role": "user", "content": verification_task})

    async for result in agent.run(history, stream=False):
        history += result.get("output", [])

        for item in result.get("output", []):
            if item.get("type") == "message":
                content = item.get("content", [])
                for content_part in content:
                    if content_part.get("text"):
                        print(f"Agent: {content_part.get('text')}")
            elif item.get("type") == "computer_call":
                action = item.get("action", {})
                action_type = action.get("type", "")
                print(f"Computer Action: {action_type}")
            elif item.get("type") == "computer_call_output":
                print("Computer Output: [Screenshot/Result]")

    return history


async def run_restore_example():
    """Run example demonstrating running tasks then restoring to initial snapshot."""
    print("\n=== Snapshot Run and Restore Example ===")
    print("This example will:")
    print("1. Create an initial clean snapshot")
    print("2. Run tasks that modify the system")
    print("3. Show the modified state")
    print("4. Restore back to the initial snapshot")
    print("5. Verify we're back to the clean state")

    try:
        # Create a Docker container connection
        computer = Computer(
            os_type="linux",
            provider_type="docker",
            image="trycua/cua-ubuntu:latest",
            name='snapshot-container',
            verbosity=logging.DEBUG,
        )

        # Create ComputerAgent with same configuration
        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-20250514",
            tools=[computer],
            only_n_most_recent_images=3,
            verbosity=logging.DEBUG,
            trajectory_dir="trajectories",
            use_prompt_caching=True,
            max_trajectory_budget=1.0,
        )

        # Create snapshot callback
        snapshot_callback = SnapshotManagerCallback(
            computer=computer,
            snapshot_interval="manual",  # We're only doing manual operations
            max_snapshots=10,
            retention_days=7,
            auto_cleanup=False  # Don't cleanup automatically in this example
        )

        # Add the callback to the agent
        agent.callbacks.append(snapshot_callback)

        # Step 1: Create initial snapshot of clean state
        print("\n=== Step 1: Creating Initial Snapshot ===")

        # First, ensure we have a clean state
        history = []
        clean_task = "List all files in the home directory"
        print(f"Checking initial state: {clean_task}")
        history.append({"role": "user", "content": clean_task})

        async for result in agent.run(history, stream=False):
            history += result.get("output", [])
            for item in result.get("output", []):
                if item.get("type") == "message":
                    content = item.get("content", [])
                    for content_part in content:
                        if content_part.get("text"):
                            print(f"Initial state: {content_part.get('text')[:100]}...")  # Show first 100 chars

        # Create initial snapshot
        initial_snapshot = await snapshot_callback.create_manual_snapshot("Clean initial state")
        initial_snapshot_id = initial_snapshot.get('id')
        print(f"✅ Created initial snapshot: {initial_snapshot.get('tag')}")
        print(f"   Snapshot ID: {initial_snapshot_id}")

        # Step 2: Run tasks that modify the system
        print("\n=== Step 2: Running Tasks that Modify the System ===")

        modification_tasks = [
            "Create a file called experiment1.txt with content 'This is experiment 1'",
            "Create a directory called test_data",
            "Create a file called test_data/results.txt with content 'Test results here'",
            "Create a file called experiment2.txt with content 'This is experiment 2'",
            "List all files and directories to show what we created"
        ]

        for i, task in enumerate(modification_tasks):
            print(f"\nExecuting task {i+1}/{len(modification_tasks)}: {task}")
            history.append({"role": "user", "content": task})

            async for result in agent.run(history, stream=False):
                history += result.get("output", [])

                for item in result.get("output", []):
                    if item.get("type") == "message":
                        content = item.get("content", [])
                        for content_part in content:
                            if content_part.get("text"):
                                print(f"Agent: {content_part.get('text')}")
                    elif item.get("type") == "computer_call":
                        action = item.get("action", {})
                        action_type = action.get("type", "")
                        print(f"Computer Action: {action_type}")

            print(f"✅ Task {i+1} completed")

        # Step 3: Show the modified state
        print("\n=== Step 3: Current Modified State ===")
        print("The system now has all the files we created during our tasks.")

        # Create a snapshot of the modified state for comparison
        modified_snapshot = await snapshot_callback.create_manual_snapshot("Modified state after tasks")
        print(f"Created snapshot of modified state: {modified_snapshot.get('tag')}")

        # Step 4: Restore to initial snapshot
        print("\n=== Step 4: Restoring to Initial Clean Snapshot ===")
        print(f"Restoring to snapshot ID: {initial_snapshot_id}")

        restore_result = await restore_specific_snapshot(
            snapshot_callback,
            initial_snapshot_id,
            tag="Clean initial state"
        )

        if not restore_result:
            print("❌ Failed to restore to initial snapshot")
            return

        # Step 5: Verify restoration worked
        print("\n=== Step 5: Verifying Restoration to Clean State ===")

        # Create new history after restoration
        history = []
        verification_task = "List all files and directories to verify we're back to the clean state"
        print(f"Running verification: {verification_task}")
        history.append({"role": "user", "content": verification_task})

        async for result in agent.run(history, stream=False):
            history += result.get("output", [])

            for item in result.get("output", []):
                if item.get("type") == "message":
                    content = item.get("content", [])
                    for content_part in content:
                        if content_part.get("text"):
                            print(f"Restored state: {content_part.get('text')}")

        print("\n✅ Successfully demonstrated running tasks and restoring to initial state!")
        print("   All the files created during tasks (experiment1.txt, experiment2.txt, test_data/) ")
        print("   should now be gone, and we're back to the clean initial state.")

        # Show final snapshot list
        print("\n=== Final Snapshot List ===")
        await list_and_display_snapshots(snapshot_callback)

        # Show statistics
        stats = snapshot_callback.get_statistics()
        print(f"\n=== Snapshot Statistics ===")
        print(f"Total snapshots: {stats.get('snapshots', 0)}")
        print(f"Retention policy: {stats.get('retention', {})}")

    except Exception as e:
        logger.error("Error in run_restore_example: %s", e)
        traceback.print_exc()
        raise


def main():
    """Run the snapshot restore example."""
    try:
        load_dotenv_files()

        # Register signal handler for graceful exit
        signal.signal(signal.SIGINT, handle_sigint)

        asyncio.run(run_restore_example())
    except Exception as e:
        print(f"Error running example: {e}")
        traceback.print_exc()


if __name__ == "__main__":
    main()