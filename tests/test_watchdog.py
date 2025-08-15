"""
Watchdog Recovery Tests
Tests for the watchdog functionality to ensure server recovery after hanging commands.
Required environment variables:
- CUA_API_KEY: API key for Cua cloud provider
- CUA_CONTAINER_NAME: Name of the container to use
"""

import os
import asyncio
import pytest
from pathlib import Path
import sys
import traceback
import time

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.insert(0, path)  # Insert at beginning to prioritize
        print(f"Added to sys.path: {path}")

from computer import Computer, VMProviderType

@pytest.fixture(scope="session")
async def computer():
    """Shared Computer instance for all test cases."""
    # Create a remote Linux computer with Cua
    computer = Computer(
        os_type="linux",
        api_key=os.getenv("CUA_API_KEY"),
        name=str(os.getenv("CUA_CONTAINER_NAME")),
        provider_type=VMProviderType.CLOUD,
    )
    
    try:
        await computer.run()
        yield computer
    finally:
        await computer.disconnect()


@pytest.mark.asyncio(loop_scope="session")
async def test_simple_server_ping(computer):
    """
    Simple test to verify server connectivity before running watchdog tests.
    """
    print("Testing basic server connectivity...")
    
    try:
        result = await computer.interface.run_command("echo 'Server ping test'")
        print(f"Ping successful: {result}")
        assert result is not None, "Server ping returned None"
        print("✅ Server connectivity test passed")
    except Exception as e:
        print(f"❌ Server ping failed: {e}")
        pytest.fail(f"Basic server connectivity test failed: {e}")


@pytest.mark.asyncio(loop_scope="session")
async def test_watchdog_recovery_after_hanging_command(computer):
    """
    Test that the watchdog can recover the server after a hanging command.
    
    This test runs two concurrent tasks:
    1. A long-running command that hangs the server (sleep 300 = 5 minutes)
    2. Periodic ping commands every 30 seconds to test server responsiveness
    
    The watchdog should detect the unresponsive server and restart it.
    """
    print("Starting watchdog recovery test...")
    
    async def hanging_command():
        """Execute a command that sleeps forever to hang the server."""
        try:
            print("Starting hanging command (sleep infinity)...")
            # Use a very long sleep that should never complete naturally
            result = await computer.interface.run_command("sleep 999999")
            print(f"Hanging command completed unexpectedly: {result}")
            return True  # Should never reach here if watchdog works
        except Exception as e:
            print(f"Hanging command interrupted (expected if watchdog restarts): {e}")
            return None  # Expected result when watchdog kills the process
    
    async def ping_server():
        """Ping the server every 30 seconds with echo commands."""
        ping_count = 0
        successful_pings = 0
        failed_pings = 0
        
        try:
            # Run pings for up to 4 minutes (8 pings at 30-second intervals)
            for i in range(8):
                try:
                    ping_count += 1
                    print(f"Ping #{ping_count}: Sending echo command...")
                    
                    start_time = time.time()
                    result = await asyncio.wait_for(
                        computer.interface.run_command(f"echo 'Ping {ping_count} at {int(start_time)}'"),
                        timeout=10.0  # 10 second timeout for each ping
                    )
                    end_time = time.time()
                    
                    print(f"Ping #{ping_count} successful in {end_time - start_time:.2f}s: {result}")
                    successful_pings += 1
                    
                except asyncio.TimeoutError:
                    print(f"Ping #{ping_count} timed out (server may be unresponsive)")
                    failed_pings += 1
                except Exception as e:
                    print(f"Ping #{ping_count} failed with exception: {e}")
                    failed_pings += 1
                
                # Wait 30 seconds before next ping
                if i < 7:  # Don't wait after the last ping
                    print(f"Waiting 30 seconds before next ping...")
                    await asyncio.sleep(30)
            
            print(f"Ping summary: {successful_pings} successful, {failed_pings} failed")
            return successful_pings, failed_pings
            
        except Exception as e:
            print(f"Ping server function failed with critical error: {e}")
            traceback.print_exc()
            return successful_pings, failed_pings
    
    # Run both tasks concurrently
    print("Starting concurrent tasks: hanging command and ping monitoring...")
    
    try:
        # Use asyncio.gather to run both tasks concurrently
        hanging_task = asyncio.create_task(hanging_command())
        ping_task = asyncio.create_task(ping_server())
        
        # Wait for both tasks to complete or timeout after 5 minutes
        done, pending = await asyncio.wait(
            [hanging_task, ping_task],
            timeout=300,  # 5 minute timeout
            return_when=asyncio.ALL_COMPLETED
        )
        
        # Cancel any pending tasks
        for task in pending:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
        
        # Get results from completed tasks
        ping_result = None
        hanging_result = None
        
        if ping_task in done:
            try:
                ping_result = await ping_task
                print(f"Ping task completed with result: {ping_result}")
            except Exception as e:
                print(f"Error getting ping task result: {e}")
                traceback.print_exc()
        
        if hanging_task in done:
            try:
                hanging_result = await hanging_task
                print(f"Hanging task completed with result: {hanging_result}")
            except Exception as e:
                print(f"Error getting hanging task result: {e}")
                traceback.print_exc()
        
        # Analyze results
        if ping_result:
            successful_pings, failed_pings = ping_result
            
            # Test passes if we had some successful pings, indicating recovery
            assert successful_pings > 0, f"No successful pings detected. Server may not have recovered."
            
            # Check if hanging command was killed (indicating watchdog restart)
            if hanging_result is None:
                print("✅ SUCCESS: Hanging command was killed - watchdog restart detected")
            elif hanging_result is True:
                print("⚠️  WARNING: Hanging command completed naturally - watchdog may not have restarted")
            
            # If we had failures followed by successes, that indicates watchdog recovery
            if failed_pings > 0 and successful_pings > 0:
                print("✅ SUCCESS: Watchdog recovery detected - server became unresponsive then recovered")
                # Additional check: hanging command should be None if watchdog worked
                assert hanging_result is None, "Expected hanging command to be killed by watchdog restart"
            elif successful_pings > 0 and failed_pings == 0:
                print("✅ SUCCESS: Server remained responsive throughout test")
            
            print(f"Test completed: {successful_pings} successful pings, {failed_pings} failed pings")
            print(f"Hanging command result: {hanging_result} (None = killed by watchdog, True = completed naturally)")
        else:
            pytest.fail("Ping task did not complete - unable to assess server recovery")
            
    except Exception as e:
        print(f"Test failed with exception: {e}")
        traceback.print_exc()
        pytest.fail(f"Watchdog recovery test failed: {e}")


if __name__ == "__main__":
    # Run tests directly
    pytest.main([__file__, "-v"])
