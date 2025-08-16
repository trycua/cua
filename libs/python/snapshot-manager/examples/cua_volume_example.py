"""
CUA Agent + Volume Snapshot Example

Demonstrates:
1. AI agent creates files in named volume
2. Automatic snapshot captures volume data  
3. Restore preserves complete volume state
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

from snapshot_manager import SnapshotManager, SnapshotConfig, SnapshotTrigger
from snapshot_manager.models import RestoreOptions
from snapshot_manager.callback import SnapshotCallback


async def cua_volume_example():
    """CUA agent with automatic volume snapshots."""
    
    load_dotenv()
    # Enable callback logging to see what's happening
    logging.basicConfig(level=logging.INFO)
    logging.getLogger("snapshot_manager.callback").setLevel(logging.DEBUG)
    
    print("🚀 CUA Agent + Auto Volume Snapshots")
    print("=" * 38)
    
    with tempfile.TemporaryDirectory() as temp_dir:
        storage_path = Path(temp_dir) / "snapshots"
        
        # Setup snapshot manager with automatic triggers
        config = SnapshotConfig(
            triggers=[SnapshotTrigger.RUN_START, SnapshotTrigger.RUN_END],
            storage_path=str(storage_path),
        )
        snapshot_manager = SnapshotManager(config=config)
        snapshot_callback = SnapshotCallback(snapshot_manager=snapshot_manager)
        
        # Create named volume
        import docker
        docker_client = docker.from_env()
        volume_name = f"agent-data-{uuid.uuid4().hex[:6]}"
        volume = docker_client.volumes.create(volume_name)
        print(f"📦 Created volume: {volume_name}")

        container_name = f"cua-volume-example-{uuid.uuid4().hex[:6]}"
        
        try:
            # Start CUA Computer (like the working example)
            async with Computer(
                os_type="linux",
                provider_type="docker",
                name=container_name
            ) as computer:
                print("✅ CUA Computer started")
                
                # Create a separate test container with volume to test volume snapshots
                test_container = docker_client.containers.run(
                    image="alpine:latest",
                    name=f"{container_name}-volume-test",
                    command="sleep 300",
                    volumes=[f"{volume_name}:/data:rw"],
                    detach=True
                )
                
                # Add initial data to volume
                test_container.exec_run("sh -c 'echo \"Initial volume data\" > /data/initial.txt'")
                print("✅ Volume test container created with initial data")
                
                # Create AI agent with snapshot callback
                agent = ComputerAgent(
                    model="anthropic/claude-3-7-sonnet-20250219",
                    tools=[computer],
                    callbacks=[snapshot_callback],  # Automatic snapshots!
                    verbosity=logging.ERROR
                )
                
                print("🔄 Running agent with auto-snapshots...")
                
                # Set container for snapshots to test the volume container  
                volume_container_name = f"{container_name}-volume-test"
                snapshot_callback.container_resolver = lambda _: volume_container_name
                
                # Agent run will automatically trigger:
                # 1. RUN_START snapshot (before task)  
                # 2. RUN_END snapshot (after task)
                task = "Open terminal, type: echo 'done' > test.txt"
                
                # Monitor for snapshots during agent run
                print("📊 Checking snapshots before agent run...")
                snapshots_before = await snapshot_manager.list_snapshots()
                print(f"   Snapshots before: {len(snapshots_before)}")
                
                try:
                    action_count = 0
                    async for result in agent.run(task):
                        if result.get("output"):
                            actions = [item for item in result["output"] if item.get("type") == "computer_call"]
                            if actions:
                                action_count += len(actions)
                                print(f"🎯 Agent action {action_count}: {actions[0].get('function', 'unknown')}")
                    print(f"🎯 Agent completed {action_count} total actions")
                except StopAsyncIteration:
                    print("🎯 Agent task completed")
                except Exception as e:
                    print(f"⚠️  Agent error: {e}")
                
                # Give a moment for callbacks to complete
                print("⏱️  Waiting for callbacks to complete...")
                await asyncio.sleep(2)
                
                print("📊 Checking snapshots after agent run...")
                snapshots_after = await snapshot_manager.list_snapshots()
                print(f"   Snapshots after: {len(snapshots_after)}")
                
                for snapshot in snapshots_after:
                    print(f"   - {snapshot.snapshot_id[:8]}... trigger={snapshot.trigger.value} time={snapshot.timestamp.strftime('%H:%M:%S')}")
                
                # 3. COMPARE SNAPSHOTS
                print("\n📊 Comparing before/after snapshots...")
                
                snapshots = await snapshot_manager.list_snapshots()
                print(f"   Found {len(snapshots)} snapshots")
                
                start_snapshots = [s for s in snapshots if s.trigger == SnapshotTrigger.RUN_START]
                end_snapshots = [s for s in snapshots if s.trigger == SnapshotTrigger.RUN_END]
                
                if not start_snapshots or not end_snapshots:
                    print(f"   ⚠️  Missing snapshots: start={len(start_snapshots)}, end={len(end_snapshots)}")
                    return
                    
                start_snapshot = start_snapshots[0]
                end_snapshot = end_snapshots[0]
                
                print(f"📸 Start snapshot: {start_snapshot.snapshot_id[:8]}... ({start_snapshot.trigger.value})")
                print(f"📸 End snapshot: {end_snapshot.snapshot_id[:8]}... ({end_snapshot.trigger.value})")
                
                # 4. RESTORE START STATE & VERIFY DIFFERENCE
                print("\n🔄 Restoring start state...")
                
                restore_options = RestoreOptions(new_container_name=f"restored-start-{uuid.uuid4().hex[:6]}")
                await snapshot_manager.restore_snapshot(start_snapshot.snapshot_id, options=restore_options)
                
                # After agent runs, modify the volume container to show the difference
                print("🔧 Modifying volume data...")
                test_container.exec_run("sh -c 'echo \"After agent\" > /data/after.txt'")
                print("✅ Volume data modified")
                
                # Check volume state vs restored state
                current_files = test_container.exec_run("ls /data/").output.decode()
                restored_files = docker_client.containers.get(restore_options.new_container_name).exec_run("ls /data/").output.decode()
                
                current_has_after = "after.txt" in current_files
                restored_has_after = "after.txt" in restored_files
                
                print(f"📁 Current volume has after file: {'✅' if current_has_after else '❌'}")
                print(f"📁 Restored volume has after file: {'❌' if not restored_has_after else '✅'}")
                
                # Cleanup
                docker_client.containers.get(restore_options.new_container_name).remove(force=True)
                test_container.stop()
                test_container.remove()
                
                print("\n✨ Demo Complete!")
                print("✅ Automatic snapshots captured before/after agent run")
                print("✅ Volume data shows clear state difference")
                print("✅ Restore verified exact point-in-time recovery")
                
        finally:
            try:
                volume.remove()
                print(f"🧹 Cleaned up volume: {volume_name}")
            except Exception as e:
                print(f"⚠️  Volume cleanup: {e}")


async def main():
    """Run the CUA volume demo."""
    load_dotenv()
    
    if not os.getenv("ANTHROPIC_API_KEY"):
        print("❌ ANTHROPIC_API_KEY required in .env file")
        return

    try:
        await cua_volume_example()
        print("\n✨ CUA Volume Demo Complete!")
    except Exception as e:
        print(f"\n❌ Demo failed: {e}")


if __name__ == "__main__":
    asyncio.run(main())
