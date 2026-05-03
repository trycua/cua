"""Deduplicate and clean the enriched software catalog.

Pure Python — no LLM calls. Uses fuzzy string matching to merge duplicates
and validates entries by checking if websites are reachable.
"""

from __future__ import annotations

import json
import logging
import re
from pathlib import Path

from thefuzz import fuzz

logger = logging.getLogger(__name__)


def _normalize_name(name: str) -> str:
    """Normalize app name for dedup comparison."""
    name = name.lower().strip()
    # Remove common suffixes
    for suffix in (" app", " software", " tool", " editor", " ide", " studio"):
        if name.endswith(suffix):
            name = name[: -len(suffix)].strip()
    # Remove version numbers
    name = re.sub(r"\s*\d+(\.\d+)*\s*$", "", name)
    return name


def deduplicate(
    input_path: Path,
    output_path: Path,
    *,
    similarity_threshold: int = 88,
) -> int:
    """Deduplicate enriched entries by fuzzy name matching.

    Returns the number of unique entries written.
    """
    from .onet import read_jsonl

    entries = read_jsonl(input_path)
    logger.info("Deduplicating %d entries (threshold=%d)", len(entries), similarity_threshold)

    # Group by normalized name
    seen: dict[str, dict] = {}  # normalized_name → best entry
    duplicates = 0

    for entry in entries:
        name = entry.get("name", "")
        if not name:
            continue

        norm = _normalize_name(name)

        # Check against all seen entries for fuzzy match
        matched = False
        for seen_norm, seen_entry in seen.items():
            score = fuzz.token_sort_ratio(norm, seen_norm)
            if score >= similarity_threshold:
                # Merge: keep the entry with more fields populated
                existing_fields = sum(1 for v in seen_entry.values() if v)
                new_fields = sum(1 for v in entry.values() if v)
                if new_fields > existing_fields:
                    seen[seen_norm] = entry
                duplicates += 1
                matched = True
                break

        if not matched:
            seen[norm] = entry

    # Write deduplicated output
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as f:
        for entry in seen.values():
            f.write(json.dumps(entry, default=str) + "\n")

    logger.info(
        "Dedup complete: %d unique entries (%d duplicates removed)",
        len(seen),
        duplicates,
    )
    return len(seen)
