"""Enrich raw software entries with detailed metadata using Claude Agent SDK.

Each entry gets its own agent with WebSearch + a submit_enriched MCP tool
that writes the result directly — no fragile JSON parsing from stdout.
"""

from __future__ import annotations

import asyncio
import json
import logging
import random
import re
import time
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

ENRICH_SYSTEM_PROMPT = """\
You are a software metadata enrichment agent. Research the given app and call
submit_enriched exactly once with a complete metadata object.

Key fields to research:
- requires_payment: True only if a CUA agent needs a credit card / paid subscription
  to use the core UI. Free tiers, trials, OSS installs = False.
- foss: True if the software has an OSI-approved open-source license with a public repo.
- gh_repo: public GitHub/GitLab/Codeberg URL, or null.
- package_managers: exact IDs for apt/snap/flatpak/brew/choco/winget — null if unknown.
- os_support: verified list from ["linux","windows","macos","android"].
- self_hostable: True if it runs fully locally with no cloud dependency.
- app_type: one of:
  "standalone" — installable desktop GUI or mobile app
  "cli" — command-line or TUI software (no GUI window)
  "library" — software dependency/SDK for development (pip, npm, cargo packages etc.)
  "webapp" — browser-only, no installable client
  "both" — has both an installable client AND a web interface
- hallucinated: True if you cannot find ANY credible evidence this software exists
  (website 404s, no search results, name looks like a generic description, etc.).
  False if you find any credible source (official site, repo, app store, review).
- hallucination_reason: Brief reason string if hallucinated=True, else null.

Be precise. Never guess package manager IDs.
"""


def _lock_path(p: Path) -> Path:
    return p.with_suffix(".lock")


def _acquire_lock(p: Path, timeout: float = 30.0) -> bool:
    lock = _lock_path(p)
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
            time.sleep(0.5)
    return False


def _release_lock(p: Path) -> None:
    _lock_path(p).unlink(missing_ok=True)


def _already_enriched(output_path: Path) -> set[str]:
    seen = set()
    if not output_path.exists():
        return seen
    with open(output_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                name = json.loads(line).get("name", "").lower().strip()
                if name:
                    seen.add(name)
            except json.JSONDecodeError:
                continue
    return seen


def _make_submit_tool(output_path: Path, result_holder: list):
    """MCP tool that writes the enriched entry to disk."""
    from claude_agent_sdk import tool

    @tool(
        "submit_enriched",
        "Submit the enriched metadata for this app. Call exactly once when done researching.",
        {
            "type": "object",
            "properties": {
                "entry": {
                    "type": "object",
                    "description": "Complete enriched app metadata object",
                    "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "description": {"type": "string"},
                        "website": {"type": "string"},
                        "icon_url": {"type": "string"},
                        "categories": {"type": "array", "items": {"type": "string"}},
                        "tags": {"type": "array", "items": {"type": "string"}},
                        "os_support": {"type": "array", "items": {"type": "string"}},
                        "app_type": {
                            "type": "string",
                            "enum": ["standalone", "cli", "library", "webapp", "both"],
                        },
                        "requires_payment": {"type": "boolean"},
                        "foss": {"type": "boolean"},
                        "gh_repo": {"type": "string"},
                        "self_hostable": {"type": "boolean"},
                        "requires_hardware": {"type": "boolean"},
                        "package_managers": {"type": "object"},
                        "download_url": {"type": "string"},
                        "hallucinated": {"type": "boolean"},
                        "hallucination_reason": {"type": "string"},
                    },
                    "required": ["name"],
                }
            },
            "required": ["entry"],
        },
    )
    async def submit_enriched(args: dict[str, Any]) -> dict[str, Any]:
        entry = args.get("entry", {})
        if not entry.get("name"):
            return {"content": [{"type": "text", "text": "ERROR: entry.name is required"}]}

        _acquire_lock(output_path)
        try:
            output_path.parent.mkdir(parents=True, exist_ok=True)
            with open(output_path, "a", encoding="utf-8") as f:
                f.write(json.dumps(entry, default=str) + "\n")
        finally:
            _release_lock(output_path)

        result_holder.append(entry)
        return {
            "content": [{"type": "text", "text": f"OK: enriched entry saved for {entry['name']}"}]
        }

    return submit_enriched


async def enrich_app(raw_entry: dict, output_path: Path, model: str = "haiku") -> dict | None:
    """Enrich a single app entry via a Claude agent with submit_enriched tool."""
    from claude_agent_sdk import ClaudeAgentOptions, create_sdk_mcp_server, query

    name = raw_entry.get("name", "unknown")
    website = raw_entry.get("website", "")

    result_holder: list = []
    submit_tool = _make_submit_tool(output_path, result_holder)
    server = create_sdk_mcp_server(name="enrich", version="1.0.0", tools=[submit_tool])

    prompt = (
        f"Enrich this app: {name} ({website})\n"
        f"Category hint: {raw_entry.get('category', '')}\n"
        f"Known OS: {raw_entry.get('os_support', [])}\n\n"
        f"Research with WebSearch, then call submit_enriched once with the complete metadata.\n"
        f"Required fields: id (slug), name, description, website, icon_url, categories, tags, "
        f"os_support, requires_payment, foss, gh_repo, self_hostable, requires_hardware, "
        f"package_managers (apt/snap/flatpak/brew/choco/winget), download_url."
    )

    try:
        async for _ in query(
            prompt=prompt,
            options=ClaudeAgentOptions(
                model=model,
                allowed_tools=["WebSearch", "WebFetch", "mcp__enrich__submit_enriched"],
                permission_mode="dontAsk",
                system_prompt=ENRICH_SYSTEM_PROMPT,
                mcp_servers={"enrich": server},
            ),
        ):
            pass
    except Exception as e:
        logger.error("Enrichment failed for %s: %s", name, e)

    return result_holder[0] if result_holder else None


async def run_enrichment(
    input_path: Path,
    output_path: Path,
    *,
    concurrency: int = 5,
    model: str = "haiku",
) -> None:
    """Enrich all raw entries, resumable via already-enriched tracking."""
    from .onet import read_jsonl

    raw_entries = read_jsonl(input_path)
    already_done = _already_enriched(output_path)

    remaining = [e for e in raw_entries if e.get("name", "").lower().strip() not in already_done]
    random.shuffle(remaining)

    if not remaining:
        logger.info("All %d entries already enriched", len(raw_entries))
        return

    logger.info(
        "Enriching %d entries (%d already done, concurrency=%d)",
        len(remaining),
        len(already_done),
        concurrency,
    )

    sem = asyncio.Semaphore(concurrency)

    async def _run_one(entry: dict) -> bool:
        async with sem:
            result = await enrich_app(entry, output_path, model=model)
            if result:
                logger.info("Enriched: %s", entry.get("name"))
            else:
                logger.warning("Failed to enrich: %s", entry.get("name"))
            return result is not None

    results = await asyncio.gather(*[_run_one(e) for e in remaining], return_exceptions=True)
    success = sum(1 for r in results if r is True)
    logger.info("Enrichment complete: %d/%d succeeded", success, len(remaining))
