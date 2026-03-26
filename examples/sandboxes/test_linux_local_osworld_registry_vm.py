"""Run the OSWorld benchmark image pulled from the OCI registry.

The image is a pre-built Ubuntu qcow2 pushed to GHCR with agent_type=osworld
so Image.from_registry() auto-detects it — no explicit agent_type= needed.

    image = Image.from_registry("ghcr.io/trycua/osworld:latest")
    async with Sandbox.ephemeral(image, local=True) as sb:
        transport = sb._transport  # OSWorldTransport
        ...

The OSWorld Flask server runs inside the VM (port 5000) and handles screen
capture, shell commands, and task setup steps. Evaluation is host-side via
the osworld Python package (desktop_env.controllers, desktop_env.evaluators).

Prerequisites
-------------
  GITHUB_TOKEN with read:packages scope — for pulling the OCI image.

  osworld is NOT required in the cua-sandbox venv.  Set one of:
    export OSWORLD_SOURCE_DIR=/tmp/osworld   # source checkout
    # OR: pip install osworld in a separate venv and set:
    # export OSWORLD_PYTHON=/path/to/venv/bin/python  (future)

  The image itself ships the Flask server — nothing extra to install.

  To push the image first (one-time, maintainers only):
    bash examples/sandboxes/images/build-osworld.sh
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any, Dict, Optional

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

OSWORLD_IMAGE_REF = "ghcr.io/trycua/osworld:latest"

# Small test suite — pick one task per category so the test runs quickly.
# IDs from evaluation_examples/test_small.json.
DEMO_TASK_ID = "bb5e4c0d-f964-439c-97b6-bdb9747de3f4"  # chrome: set Bing as default search


# ── Helpers ───────────────────────────────────────────────────────────────────


def _has_osworld_image() -> bool:
    """True if the registry image is reachable."""
    try:
        from cua_sandbox.registry.manifest import get_manifest

        get_manifest(OSWORLD_IMAGE_REF)
        return True
    except Exception:
        return False


def _has_qemu() -> bool:
    try:
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        qemu_bin("x86_64")
        return True
    except Exception:
        return False


def _load_task_config(task_id: str, osworld_dir: Optional[str] = None) -> Optional[Dict[str, Any]]:
    """Load a task config JSON from the OSWorld evaluation_examples directory.

    Searches:
      1. OSWORLD_SOURCE_DIR env var
      2. osworld_dir argument
      3. /tmp/osworld (default clone location)
    """
    search_dirs = [
        os.environ.get("OSWORLD_SOURCE_DIR", ""),
        osworld_dir or "",
        "/tmp/osworld",
    ]
    for base in search_dirs:
        if not base:
            continue
        examples_root = Path(base) / "evaluation_examples" / "examples"
        if not examples_root.exists():
            continue
        for json_path in examples_root.rglob(f"{task_id}.json"):
            with open(json_path) as f:
                return json.load(f)
    return None


# ── Tests ─────────────────────────────────────────────────────────────────────


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(not _has_osworld_image(), reason="OSWorld registry image not reachable")
async def test_linux_local_osworld_registry_vm():
    """Boot the OSWorld registry image, take a screenshot, run a shell command."""
    image = Image.from_registry(OSWORLD_IMAGE_REF)

    async with Sandbox.ephemeral(image, local=True) as sb:
        transport = sb._transport  # OSWorldTransport

        # Basic sanity: screenshot
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG", "Expected PNG screenshot"
        print(f"Screenshot: {len(screenshot)} bytes")

        # Screen dimensions — x86_64 image is 1920×1080; arm64 virtio-gpu defaults to 1280×800
        size = await transport.get_screen_size()
        assert size["width"] >= 800 and size["height"] >= 600, f"Unexpected size: {size}"
        print(f"Screen size: {size['width']}x{size['height']}")

        # Shell execution via /execute
        result = await transport.send("execute", command=["uname", "-a"], shell=False)
        assert result.get("returncode") == 0
        print(f"uname: {result.get('output', '').strip()}")


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(not _has_osworld_image(), reason="OSWorld registry image not reachable")
async def test_linux_local_osworld_task_setup():
    """Boot the OSWorld image, load a task config, run setup steps, take screenshot."""
    osworld_dir = os.environ.get("OSWORLD_SOURCE_DIR", "/tmp/osworld")
    task_config = _load_task_config(DEMO_TASK_ID, osworld_dir)

    if task_config is None:
        pytest.skip(
            f"OSWorld task {DEMO_TASK_ID} not found. "
            "Clone OSWorld to /tmp/osworld or set OSWORLD_SOURCE_DIR."
        )

    image = Image.from_registry(OSWORLD_IMAGE_REF)

    async with Sandbox.ephemeral(image, local=True) as sb:
        transport = sb._transport  # OSWorldTransport

        print(f"Task: {task_config['instruction']}")

        # Save a clean-state snapshot before the first task
        # (skip if no QMP port — e.g. running without bare-metal runtime)
        if transport._qmp_port:
            await transport.save_snapshot()

        # Run task setup steps on the VM (launches Chrome, etc.)
        await transport.setup_task(task_config)

        # Take a screenshot after setup — should show the app launched
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        print(f"Post-setup screenshot: {len(screenshot)} bytes")

        # Save screenshot for inspection
        out = Path("/tmp/osworld_task_setup.png")
        out.write_bytes(screenshot)
        print(f"Saved to {out}")


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(not _has_osworld_image(), reason="OSWorld registry image not reachable")
async def test_linux_local_osworld_full_loop():
    """Full OSWorld task loop: setup → agent step → evaluate.

    Requires osworld importable on the host (OSWORLD_SOURCE_DIR or installed).
    The score will be 0.0 since no real agent runs — this validates the pipeline.
    """
    osworld_dir = os.environ.get("OSWORLD_SOURCE_DIR", "/tmp/osworld")
    task_config = _load_task_config(DEMO_TASK_ID, osworld_dir)

    if task_config is None:
        pytest.skip("OSWorld task JSON not found — set OSWORLD_SOURCE_DIR")

    try:
        import sys

        if osworld_dir and osworld_dir not in sys.path:
            sys.path.insert(0, osworld_dir)
        import desktop_env  # noqa: F401
    except ImportError:
        pytest.skip(
            "osworld (desktop_env) not importable. "
            "Clone OSWorld to /tmp/osworld or set OSWORLD_SOURCE_DIR."
        )

    image = Image.from_registry(OSWORLD_IMAGE_REF)

    async with Sandbox.ephemeral(image, local=True) as sb:
        transport = sb._transport  # OSWorldTransport

        print(f"Task: {task_config['instruction']}")

        # Setup
        await transport.setup_task(task_config)

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")

        # (Agent would act here)

        # Evaluate — score = 0.0 since we took no action
        score = await transport.evaluate_task(task_config, osworld_source_dir=osworld_dir)
        print(f"Score: {score}")
        assert 0.0 <= score <= 1.0


# ── Interactive main ──────────────────────────────────────────────────────────


async def main():
    """Interactive demo — boots the image, runs a task, prints score."""
    osworld_dir = os.environ.get("OSWORLD_SOURCE_DIR", "/tmp/osworld")
    task_config = _load_task_config(DEMO_TASK_ID, osworld_dir)

    if task_config is None:
        print(
            "OSWorld task JSON not found.\n"
            "Clone OSWorld: git clone --depth=1 https://github.com/xlang-ai/OSWorld /tmp/osworld\n"
            "Or set: export OSWORLD_SOURCE_DIR=/path/to/osworld"
        )
        return

    image = Image.from_registry(OSWORLD_IMAGE_REF)

    async with Sandbox.ephemeral(image, local=True, name="osworld-demo") as sb:
        transport = sb._transport

        print(f"Task: {task_config['instruction']}")
        await transport.setup_task(task_config)

        screenshot = await sb.screenshot()
        Path("/tmp/osworld_demo.png").write_bytes(screenshot)
        print(f"Screenshot saved ({len(screenshot)} bytes) → /tmp/osworld_demo.png")

        # TODO: run your agent here
        # action = await my_agent.step(task_config["instruction"], screenshot)
        # await transport.send("execute", command=action)

        score = await transport.evaluate_task(task_config, osworld_source_dir=osworld_dir)
        print(f"Final score: {score}")


if __name__ == "__main__":
    asyncio.run(main())
