"""Agent-based software discovery using Claude Agent SDK.

Each occupation group gets its own agent with WebSearch + a custom
submit_apps tool that ingests entries directly into the catalog JSONL.
All 22 groups run fully in parallel. Outer loop re-prompts until target hit.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import random
import time
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

DISCOVERY_SYSTEM_PROMPT = """\
You are a massive-scale software discovery agent. Your goal is to find as many
real software applications as possible that are used in professional work.

You have these tools:
- WebSearch / WebFetch: research software used in occupations
- submit_apps: submit discovered apps to the catalog (MUST use this)

WORKFLOW — repeat this loop until you run out of ideas:
1. WebSearch for lists of software for a specific sub-occupation or category
2. Compile up to 200 entries from the results
3. Call submit_apps with the list — it returns how many were new vs duplicates
4. Pick a DIFFERENT search angle and repeat

Rules:
- EVERY entry must be a REAL software product. WebSearch to verify if unsure.
- Include ALL types: paid, free, open-source, SaaS, mobile, desktop, CLI, web apps.
- Go DEEP into subcategories and niche tools.
- Cover ALL operating systems including Android and macOS-only apps.
- Do NOT stop after one batch. Keep searching new angles.
- Aim for 300-500 unique entries across multiple submit_apps calls.
"""


def _lock_path(output_path: Path) -> Path:
    return output_path.with_suffix(".lock")


def _acquire_lock(output_path: Path, timeout: float = 30.0) -> bool:
    lock = _lock_path(output_path)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            lock.open("x").close()
            return True
        except FileExistsError:
            try:
                if time.time() - lock.stat().st_mtime > 300:
                    lock.unlink(missing_ok=True)
                    continue
            except FileNotFoundError:
                continue
            time.sleep(0.3)
    return False


def _release_lock(output_path: Path) -> None:
    _lock_path(output_path).unlink(missing_ok=True)


def _count_catalog(catalog_path: Path) -> int:
    if not catalog_path.exists():
        return 0
    count = 0
    with open(catalog_path, encoding="utf-8") as f:
        for line in f:
            if line.strip():
                count += 1
    return count


def _existing_names(catalog_path: Path) -> set[str]:
    names = set()
    if not catalog_path.exists():
        return names
    with open(catalog_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                names.add(json.loads(line).get("name", "").lower().strip())
            except json.JSONDecodeError:
                continue
    return names


def _make_ingress_tool(catalog_path: Path):
    """Create the submit_apps MCP tool that ingests entries into the catalog."""
    from claude_agent_sdk import tool

    @tool(
        "submit_apps",
        "Submit discovered software applications to the catalog. "
        "Pass a JSON array of app objects. Each object needs: name, website, category, "
        "os_support (array), description. Returns count of new vs duplicate entries.",
        {
            "type": "object",
            "properties": {
                "apps": {
                    "type": "array",
                    "description": "Array of app objects to submit",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "website": {"type": "string"},
                            "category": {"type": "string"},
                            "os_support": {"type": "array", "items": {"type": "string"}},
                            "description": {"type": "string"},
                            "_source": {"type": "string"},
                            "_occupation_group": {"type": "string"},
                        },
                        "required": ["name", "website", "category"],
                    },
                }
            },
            "required": ["apps"],
        },
    )
    async def submit_apps(args: dict[str, Any]) -> dict[str, Any]:
        apps = args.get("apps", [])
        if not apps:
            return {"content": [{"type": "text", "text": "ERROR: No apps provided."}]}

        existing = _existing_names(catalog_path)
        new_entries = []
        dupes = 0
        invalid = 0

        for entry in apps:
            name = entry.get("name", "").strip()
            if not name:
                invalid += 1
                continue
            if name.lower() in existing:
                dupes += 1
                continue
            existing.add(name.lower())
            new_entries.append(entry)

        if new_entries:
            _acquire_lock(catalog_path)
            try:
                catalog_path.parent.mkdir(parents=True, exist_ok=True)
                with open(catalog_path, "a", encoding="utf-8") as f:
                    for e in new_entries:
                        f.write(json.dumps(e, default=str) + "\n")
            finally:
                _release_lock(catalog_path)

        total = _count_catalog(catalog_path)
        msg = (
            f"INGRESS RESULT: {len(new_entries)} new, {dupes} duplicates, "
            f"{invalid} invalid. TOTAL IN CATALOG: {total}"
        )
        logger.info(msg)
        return {"content": [{"type": "text", "text": msg}]}

    return submit_apps


def _get_categories_so_far(catalog_path: Path) -> dict[str, int]:
    cats: dict[str, int] = {}
    if not catalog_path.exists():
        return cats
    with open(catalog_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
                cat = entry.get("category", "unknown")
                cats[cat] = cats.get(cat, 0) + 1
            except json.JSONDecodeError:
                continue
    return cats


def _gap_analysis(catalog_path: Path, occupations: list[dict]) -> str:
    cats = _get_categories_so_far(catalog_path)
    total = _count_catalog(catalog_path)
    lines = [f"CATALOG: {total} entries across {len(cats)} categories.\n"]
    lines.append("TOP CATEGORIES:")
    for cat, count in sorted(cats.items(), key=lambda x: -x[1])[:15]:
        lines.append(f"  {cat}: {count}")
    lines.append("\nUNDERSERVED (< 5 entries):")
    for cat, count in sorted(cats.items(), key=lambda x: x[1]):
        if count < 5:
            lines.append(f"  {cat}: {count}")
    return "\n".join(lines)


SEARCH_ANGLES = [
    "mainstream commercial tools, SaaS platforms, and industry-standard desktop software",
    "free and open-source alternatives, self-hosted tools, and community-driven software",
    "mobile apps (Android/iOS), web-based tools, and browser extensions",
    "niche specialist tools, legacy software, and vertical-market applications",
    "enterprise and team collaboration platforms, workflow automation, and integrations",
    "AI-powered tools, analytics platforms, data visualization, and reporting software",
    "security, compliance, monitoring, and IT management tools for this sector",
    "hardware-specific software, device drivers, firmware tools, and embedded systems",
]


async def discover_software_for_group(
    occupation_group: str,
    occupation_title: str,
    catalog_path: Path,
    model: str = "haiku",
    search_angle: str = SEARCH_ANGLES[0],
    min_new_entries: int = 50,
) -> int:
    """Run one discovery agent session for a specific occupation group.

    If the agent submits fewer than min_new_entries new items, it is re-prompted
    up to 2 extra times to search from a different sub-angle.
    """
    from claude_agent_sdk import ClaudeAgentOptions, create_sdk_mcp_server, query

    submit_tool = _make_ingress_tool(catalog_path)
    ingress_server = create_sdk_mcp_server(
        name="ingress",
        version="1.0.0",
        tools=[submit_tool],
    )

    initial_prompt = (
        f'Discover software used by workers in: "{occupation_title}" (SOC: {occupation_group})\n\n'
        f"THIS ROUND FOCUS: {search_angle}\n\n"
        f"Search specifically for tools matching this focus area. Think about sub-occupations and niches.\n\n"
        f"Call submit_apps with batches of up to 200 entries. Each entry needs:\n"
        f'- name, website, category, os_support (["linux","windows","macos","android"]), description\n'
        f'- _source (your search query), _occupation_group ("{occupation_group}")\n\n'
        f"After each submit_apps call, read the INGRESS RESULT. If it shows fewer than {min_new_entries} "
        f"new entries so far, KEEP SEARCHING from a different angle until you reach {min_new_entries}+ new entries.\n"
        f"Aim for 200+ unique entries across 3+ submit_apps calls."
    )

    continue_prompt = (
        f"The previous search only found a few new apps. Continue discovering software for "
        f'"{occupation_title}" workers, focus: {search_angle}.\n\n'
        f"Try COMPLETELY DIFFERENT search queries — specific sub-specialties, regional vendors, "
        f"niche verticals, legacy tools, or non-English markets.\n\n"
        f"Call submit_apps with whatever you find. Every new entry helps."
    )

    count_before = _count_catalog(catalog_path)
    total_added = 0
    session_id: str | None = None

    for attempt in range(3):
        attempt_before = _count_catalog(catalog_path)
        try:
            options = ClaudeAgentOptions(
                model=model,
                mcp_servers={"ingress": ingress_server},
                allowed_tools=[
                    "WebSearch",
                    "WebFetch",
                    "mcp__ingress__submit_apps",
                ],
                permission_mode="dontAsk",
                system_prompt=DISCOVERY_SYSTEM_PROMPT,
            )
            # Resume the same session on retry so the agent remembers what it already searched
            if attempt > 0 and session_id:
                options.resume = session_id

            prompt = initial_prompt if attempt == 0 else continue_prompt

            async for msg in query(prompt=prompt, options=options):
                # Capture session_id for potential resume
                if hasattr(msg, "session_id") and msg.session_id:
                    session_id = msg.session_id

        except Exception as e:
            logger.error(
                "Discovery agent failed for %s (attempt %d): %s", occupation_group, attempt, e
            )

        added_this_attempt = _count_catalog(catalog_path) - attempt_before
        total_added += added_this_attempt
        logger.info(
            "%s attempt %d: +%d new entries (total this call: %d)",
            occupation_group,
            attempt,
            added_this_attempt,
            total_added,
        )

        if total_added >= min_new_entries:
            break
        if attempt < 2:
            logger.info(
                "%s: only %d new entries (< %d), resuming session %s...",
                occupation_group,
                total_added,
                min_new_entries,
                session_id,
            )

    return total_added


async def run_discovery(
    occupations: list[dict],
    output_path: Path,
    *,
    target: int = 50_000,
    model: str = "haiku",
    max_parallel: int = 22,
    start_angle: int = 0,
) -> None:
    """Outer loop: keep spawning discovery agents until we hit the target.

    All occupation groups run in parallel (up to max_parallel). If too many
    fail, automatically reduces parallelism. Individual agent failures never
    block others — each is fire-and-forget with error logging.
    """
    output_path.parent.mkdir(parents=True, exist_ok=True)

    current = _count_catalog(output_path)
    logger.info(
        "Starting discovery. Current catalog: %d, target: %d, model: %s", current, target, model
    )

    if current >= target:
        logger.info("Already at target (%d >= %d)", current, target)
        return

    concurrency = max_parallel
    round_num = 0
    angle_idx = start_angle
    consecutive_zero_rounds = 0

    # Shuffle occupation order so parallel angle runs don't all hammer the same
    # groups at the same time (reduces lock contention and API rate limit spikes)
    occupations = list(occupations)
    random.shuffle(occupations)

    while current < target:
        round_num += 1
        angle = SEARCH_ANGLES[angle_idx % len(SEARCH_ANGLES)]
        logger.info(
            "=== DISCOVERY ROUND %d (catalog: %d / %d, concurrency: %d, angle: %s) ===",
            round_num,
            current,
            target,
            concurrency,
            angle[:40],
        )

        tasks_to_run = [(occ["soc_code"], occ["occupation_title"]) for occ in occupations]

        sem = asyncio.Semaphore(concurrency)

        async def _run_one(soc_code: str, title: str, _angle: str = angle) -> tuple[int, bool]:
            """Returns (entries_added, success). Never raises."""
            async with sem:
                logger.info("[Round %d] Starting: %s", round_num, title)
                try:
                    n = await discover_software_for_group(
                        soc_code,
                        title,
                        output_path,
                        model=model,
                        search_angle=_angle,
                    )
                    logger.info("[Round %d] +%d entries for %s", round_num, n, title)
                    return (n, True)
                except Exception as e:
                    logger.error("[Round %d] FAILED %s: %s", round_num, title, e)
                    return (0, False)

        batch_tasks = [_run_one(code, title) for code, title in tasks_to_run]
        results = await asyncio.gather(*batch_tasks)

        added = sum(r[0] for r in results)
        successes = sum(1 for r in results if r[1])
        failures = sum(1 for r in results if not r[1])

        current = _count_catalog(output_path)
        logger.info(
            "Round %d done: +%d entries, %d/%d agents succeeded. TOTAL: %d / %d",
            round_num,
            added,
            successes,
            len(results),
            current,
            target,
        )

        # Advance to next search angle each round
        angle_idx += 1

        if added == 0:
            consecutive_zero_rounds += 1
            logger.warning(
                "No new entries in round %d (angle: %s). Zero streak: %d",
                round_num,
                angle[:40],
                consecutive_zero_rounds,
            )
            # Give up only after exhausting all angles twice
            if consecutive_zero_rounds >= len(SEARCH_ANGLES) * 2:
                logger.warning("Exhausted all search angles, stopping.")
                break
        else:
            consecutive_zero_rounds = 0

        # If >50% failed, reduce parallelism for next round (rate limits, OOM, etc.)
        if failures > successes and concurrency > 3:
            concurrency = max(3, concurrency // 2)
            logger.warning("High failure rate, reducing concurrency to %d", concurrency)
