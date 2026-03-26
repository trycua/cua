"""Run an AndroidWorld benchmark image with the Cua Sandbox SDK.

AndroidWorld is a benchmark of 116 tasks across 20 real Android apps, backed by
a live Android emulator. This example shows how to:

  1. Boot an AndroidWorld container/VM image
  2. Pick a task and retrieve its natural-language goal
  3. Run a minimal agent loop (screenshot → action → score)
  4. Score the result

--- Local container image (built from google-research/android_world Dockerfile) ---

    image = Image.from_file(
        "path/to/androidworld.qcow2",
        os_type="android",
        agent_type="androidworld",
    )
    async with Sandbox.ephemeral(image, local=True) as sb:
        ...

--- Registry image (agent_type resolved from manifest annotation) ---

    image = Image.from_registry("ghcr.io/trycua/androidworld:latest")
    async with Sandbox.ephemeral(image, local=True) as sb:
        ...

The AndroidWorldTransport exposes the full AndroidWorld task lifecycle:
  sb.androidworld.initialize_task(task_type, task_idx)
  sb.androidworld.get_task_goal(task_type, task_idx)
  sb.androidworld.execute_action({...})
  sb.androidworld.get_task_score(task_type, task_idx)
  sb.androidworld.tear_down_task(task_type, task_idx)
"""

from __future__ import annotations

import asyncio

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

# Replace with ghcr.io/trycua/androidworld:latest once the image is published.
# For local testing: build from /tmp/android_world/Dockerfile and push to GHCR,
# or point at a local qcow2 disk image.
ANDROIDWORLD_IMAGE_REF = "ghcr.io/trycua/androidworld:latest"

# Task to run in the demo. Any key from the android_world suite works.
DEMO_TASK = "ContactsAddContact"
DEMO_TASK_IDX = 0


def _has_androidworld_image() -> bool:
    """True if the registry image is reachable (basic connectivity check)."""
    try:
        from cua_sandbox.registry.manifest import get_manifest

        get_manifest(ANDROIDWORLD_IMAGE_REF)
        return True
    except Exception:
        return False


@pytest.mark.skipif(
    not _has_androidworld_image(),
    reason="AndroidWorld registry image not reachable",
)
async def test_android_local_androidworld_vm():
    image = Image.from_registry(ANDROIDWORLD_IMAGE_REF)

    async with Sandbox.ephemeral(image, local=True) as sb:
        # Verify the server is healthy
        transport = sb._transport  # AndroidWorldTransport
        assert await transport.health(), "AndroidWorld server not healthy"

        # List available tasks
        task_list = await transport.suite_task_list()
        assert DEMO_TASK in task_list, f"{DEMO_TASK} not found in suite: {task_list[:5]}"

        # Initialize the task (sets up device state and app snapshots)
        await transport.initialize_task(DEMO_TASK, DEMO_TASK_IDX)

        # Get the natural-language goal
        goal = await transport.get_task_goal(DEMO_TASK, DEMO_TASK_IDX)
        assert goal, "Expected a non-empty goal string"
        print(f"Task goal: {goal}")

        # Take a screenshot
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG", "Expected PNG screenshot"
        print(f"Screenshot: {len(screenshot)} bytes")

        # Simulate one agent action (tap the center of the screen)
        screen_size = await sb.get_dimensions()
        cx, cy = screen_size[0] // 2, screen_size[1] // 2
        await transport.execute_action({"action_type": "click", "x": cx, "y": cy})

        # Score the task (0.0 = fail, 1.0 = success)
        score = await transport.get_task_score(DEMO_TASK, DEMO_TASK_IDX)
        print(f"Score after 1 step: {score}")

        # Tear down (restores app snapshots)
        await transport.tear_down_task(DEMO_TASK, DEMO_TASK_IDX)


async def main():
    """Interactive demo — boots the image and runs a full task loop."""
    image = Image.from_registry(ANDROIDWORLD_IMAGE_REF)

    async with Sandbox.ephemeral(image, local=True, name="androidworld-demo") as sb:
        transport = sb._transport

        # List tasks
        task_list = await transport.suite_task_list()
        print(f"Suite has {len(task_list)} tasks. First 5: {task_list[:5]}")

        task_type = task_list[0]
        task_idx = 0

        # Initialize
        await transport.initialize_task(task_type, task_idx)
        goal = await transport.get_task_goal(task_type, task_idx)
        print(f"\nRunning task: {task_type}[{task_idx}]")
        print(f"Goal: {goal}")

        # Minimal agent loop — replace with your agent
        max_steps = 10
        for step in range(max_steps):
            screenshot = await sb.screenshot()
            with open(f"/tmp/androidworld_step_{step:02d}.png", "wb") as f:
                f.write(screenshot)
            print(f"Step {step + 1}: screenshot saved ({len(screenshot)} bytes)")

            # TODO: run your agent here to produce an action dict
            # action = await my_agent.step(goal, screenshot)
            # await transport.execute_action(action)

            score = await transport.get_task_score(task_type, task_idx)
            print(f"  score: {score}")
            if score >= 1.0:
                print("Task completed successfully!")
                break

        await transport.tear_down_task(task_type, task_idx)
        print("Done.")


if __name__ == "__main__":
    asyncio.run(main())
