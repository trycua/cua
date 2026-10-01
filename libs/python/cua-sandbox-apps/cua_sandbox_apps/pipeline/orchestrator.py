"""Orchestrator: distributes app installer creation across parallel agents.

Detects which OS sandboxes are HW-acceleratable on this host, filters apps
to matching OS types, and spawns concurrent creator agents.
"""

from __future__ import annotations

import asyncio
import json
import logging
from pathlib import Path

from .shared_memory import append_learning, init_memory, read_memory

logger = logging.getLogger(__name__)

# OS types we support
ALL_OS = ["linux", "windows", "macos", "android"]


def detect_supported_os() -> list[str]:
    """Detect which OS types can run with HW acceleration on this host."""
    supported = []
    try:
        from cua_sandbox import Image
        from cua_sandbox.runtime.compat import check_local_support

        checks = {
            "linux": Image.linux(),
            "windows": Image.windows(),
            "macos": Image.macos(),
            "android": Image.android(),
        }
        for os_type, img in checks.items():
            try:
                result = check_local_support(img)
                if result.supported:
                    hw = " (HW accel)" if result.hw_accel else " (no HW accel)"
                    logger.info("OS %s: supported via %s%s", os_type, result.runtime_name, hw)
                    supported.append(os_type)
                else:
                    logger.info("OS %s: not supported (%s)", os_type, result.reason)
            except Exception as e:
                logger.warning("OS %s: check failed: %s", os_type, e)
    except ImportError:
        logger.warning("cua_sandbox not installed, defaulting to linux-only")
        supported = ["linux"]

    if not supported:
        logger.warning("No OS types detected as supported, defaulting to linux")
        supported = ["linux"]

    return supported


def load_catalog(catalog_path: Path) -> list[dict]:
    """Load the discovery catalog JSONL."""
    entries = []
    if not catalog_path.exists():
        return entries
    with open(catalog_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entries.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return entries


def _already_created(apps_dir: Path) -> set[str]:
    """Get set of (app_id/os) pairs that already have install scripts."""
    done = set()
    if not apps_dir.exists():
        return done
    for app_dir in apps_dir.iterdir():
        if not app_dir.is_dir():
            continue
        for os_dir in app_dir.iterdir():
            if not os_dir.is_dir():
                continue
            ext = "ps1" if os_dir.name == "windows" else "sh"
            has_install = (os_dir / f"install.{ext}").exists()
            has_result = (
                (os_dir / "result.json").exists()
                or (os_dir / "logs" / "install-result.json").exists()
                or (os_dir / "logs" / "result.json").exists()
            )  # legacy name
            if has_install or has_result:
                done.add(f"{app_dir.name}/{os_dir.name}")
    return done


async def run_orchestrator(
    catalog_path: Path,
    apps_dir: Path,
    *,
    os_filter: list[str] | None = None,
    concurrency: int = 4,
    limit: int | None = None,
    strict_install_verify: bool = True,
) -> None:
    """Main orchestrator: create installers for all apps in the catalog.

    Args:
        catalog_path: Path to the discovery catalog JSONL.
        apps_dir: Root output directory for app installers.
        os_filter: Only create for these OS types. None = auto-detect.
        concurrency: Max concurrent creator agents.
        limit: Max number of (app, os) pairs to process. None = all.
    """
    from .creator_agent import create_installer_from_entry

    # Detect or filter OS types
    if os_filter:
        supported_os = os_filter
    else:
        supported_os = detect_supported_os()

    logger.info("Supported OS types: %s", supported_os)

    # Initialize shared memory
    base_dir = apps_dir.parent
    mem_dir = init_memory(base_dir)

    # Load catalog
    entries = load_catalog(catalog_path)
    if not entries:
        logger.error("No entries in catalog: %s", catalog_path)
        return

    logger.info("Catalog: %d entries", len(entries))

    # Determine what's already done
    already_done = _already_created(apps_dir)
    logger.info("Already created: %d app/os pairs", len(already_done))

    # Build work queue: (entry, os) pairs not yet done
    skipped_webapp = 0
    skipped_os = 0
    work = []
    for entry in entries:
        app_id = entry.get("id", entry.get("name", "").lower().replace(" ", "_"))
        if not app_id:
            continue
        # Skip webapps and libraries — nothing to install/run as end-user software
        if entry.get("app_type") in ("webapp", "library"):
            skipped_webapp += 1
            continue
        entry_os = entry.get("os_support", [])
        for os_type in supported_os:
            if os_type not in entry_os:
                skipped_os += 1
                continue
            if f"{app_id}/{os_type}" not in already_done:
                work.append((entry, os_type))

    logger.info("Skipped: %d webapps, %d OS mismatches", skipped_webapp, skipped_os)

    if limit:
        work = work[:limit]

    if not work:
        logger.info("Nothing to do — all app/os pairs already created")
        return

    logger.info("Work queue: %d app/os pairs (concurrency=%d)", len(work), concurrency)

    # Run with semaphore
    sem = asyncio.Semaphore(concurrency)
    success = 0
    fail = 0

    async def _run_one(entry: dict, os_type: str) -> bool:
        async with sem:
            app_name = entry.get("name", "unknown")
            logger.info("Creating: %s / %s", app_name, os_type)
            ok = await create_installer_from_entry(
                entry,
                os_type,
                apps_dir,
                strict_install_verify=strict_install_verify,
            )
            if ok:
                logger.info("SUCCESS: %s / %s", app_name, os_type)
            else:
                logger.warning("FAILED: %s / %s", app_name, os_type)
            return ok

    tasks = [_run_one(entry, os_type) for entry, os_type in work]
    results = await asyncio.gather(*tasks, return_exceptions=True)

    for r in results:
        if r is True:
            success += 1
        elif isinstance(r, Exception):
            logger.error("Agent exception: %s", r)
            fail += 1
        else:
            fail += 1

    logger.info(
        "Orchestrator complete: %d success, %d failed out of %d",
        success,
        fail,
        len(work),
    )
