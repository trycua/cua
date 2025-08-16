"""
CUA Agent + Snapshot Manager Integration Test

This test demonstrates the complete snapshot workflow:
1. Create a CUA Computer container
2. Take initial snapshot (before agent actions)
3. Run agent to create a test file
4. Take final snapshot (after agent actions)
5. Verify file was created in the container
6. Restore initial snapshot to new container
7. Verify restored container doesn't have the file (proves snapshot integrity)
"""

import asyncio
import logging
import os
import uuid

from dotenv import load_dotenv
from agent import ComputerAgent
from computer import Computer
from snapshot_manager import SnapshotManager, SnapshotTrigger, SnapshotConfig
from snapshot_manager.models import RestoreOptions

load_dotenv()
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def cua_snapshot_test():
    """Test CUA Agent with snapshot comparison"""
    print("\n🧪 CUA Agent + Snapshot Test")
    print("=" * 35)
    
    container_name = f"cua-test-{uuid.uuid4().hex[:8]}"
    config = SnapshotConfig(storage_path="./test_snapshots")
    snapshot_manager = SnapshotManager(config=config)
    
    try:
        print(f"🐳 Creating CUA Computer: {container_name}")
        
        async with Computer(
            os_type="linux",
            provider_type="docker", 
            name=container_name
        ) as computer:
            print("✅ CUA Computer connected")
            
            # Take initial snapshot
            print("📸 Taking initial snapshot...")
            initial_snapshot = await snapshot_manager.create_snapshot(
                container_id=container_name,
                trigger=SnapshotTrigger.MANUAL,
                description="Before agent execution"
            )
            print(f"   Initial snapshot: {initial_snapshot.snapshot_id}")
            
            # Create agent and run task
            agent = ComputerAgent(
                model="anthropic/claude-3-7-sonnet-20250219",
                tools=[computer],
                verbosity=logging.WARNING
            )
            
            print("🚀 Running task: create test.txt file")
            prompt = "Open a terminal and create a file called 'test.txt' with content 'Hello CUA'"
            
            action_count = 0
            async for result in agent.run(prompt):
                if result.get("output"):
                    for item in result["output"]:
                        if item["type"] == "computer_call":
                            action_count += 1
                            print(f"🔧 Action {action_count}: {item['action']['type']}")
            
            print(f"✅ Task completed with {action_count} actions")
            
            # Take final snapshot  
            print("📸 Taking final snapshot...")
            final_snapshot = await snapshot_manager.create_snapshot(
                container_id=container_name,
                trigger=SnapshotTrigger.MANUAL,
                description="After agent execution"
            )
            print(f"   Final snapshot: {final_snapshot.snapshot_id}")
            
            # Verify file was created
            print("🔍 Verifying file creation...")
            result = await asyncio.create_subprocess_exec(
                "docker", "exec", container_name, "cat", "test.txt",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            stdout, stderr = await result.communicate()
            
            if stdout and b"Hello CUA" in stdout:
                print(f"✅ File verified: '{stdout.decode().strip()}'")
            else:
                print(f"❌ File not found: {stderr.decode().strip() if stderr else 'No output'}")
            
            # Test snapshot restoration
            print("📊 Testing snapshot restoration...")
            
            # Restore initial snapshot (should NOT have test.txt)
            print("🔄 Restoring initial snapshot...")
            restore_options = RestoreOptions(new_container_name=f"{container_name}-restored")
            await snapshot_manager.restore_snapshot(
                snapshot_id=initial_snapshot.snapshot_id,
                options=restore_options
            )
            
            # Check if test.txt exists in restored container (should be NO)
            check_cmd = await asyncio.create_subprocess_exec(
                "docker", "exec", f"{container_name}-restored", "sh", "-c", 
                "test -f test.txt && echo 'YES' || echo 'NO'",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            stdout, _ = await check_cmd.communicate()
            initial_has_file = stdout.decode().strip() == "YES"
            
            print(f"   Restored container has test.txt: {'❌ NO' if not initial_has_file else '✅ YES'}")
            
            # Clean up restored container
            await asyncio.create_subprocess_exec("docker", "rm", "-f", f"{container_name}-restored")
            
            if not initial_has_file:
                print("🎉 Perfect! Snapshot restoration works correctly!")
            else:
                print("⚠️  Unexpected: file exists in initial snapshot")
            
    except Exception as e:
        print(f"❌ Test failed: {e}")
        logger.exception("Detailed error:")


async def main():
    if not os.getenv("ANTHROPIC_API_KEY"):
        print("❌ Need ANTHROPIC_API_KEY in .env file")
        return
    
    try:
        await cua_snapshot_test()
        print("\n✨ Test completed successfully!")
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        logger.exception("Error details:")


if __name__ == "__main__":
    asyncio.run(main())
