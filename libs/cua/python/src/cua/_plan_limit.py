"""Reads the relay's plan-limit refusal back out of an error's text.

The relay answers a new computer in use over the plan's cap with HTTP 429. The
SDK turns it into one plain sentence, such as "You have 5 of 5 computers in
use at once on Pro. Close one or upgrade to Teams.", and appends
"Upgrade: <url>". This module holds no native code, so it loads anywhere.
"""

from __future__ import annotations

import re
from typing import NamedTuple, Optional

# The sessions sentence has two wordings: the current one, and the old "live
# sessions" one that older relays, daemons and SDKs still send. Both are read.
_FORMS = (
    ("You have ", "sessions", " computers in use at once on "),
    ("You're using ", "sessions", " live sessions on "),
    ("You've used ", "hours", " remote hours this month on "),
)
_COUNTS = re.compile(r"^(\d+(?:\.\d+)?) of (\d+(?:\.\d+)?)$")
_UPGRADE = re.compile(r"\sUpgrade: (https?://\S+)\s*$")


class PlanLimitDetails(NamedTuple):
    kind: str
    plan: str
    used: float
    limit: float
    upgrade_url: Optional[str]
    message: str


def parse_plan_limit(text: str) -> Optional[PlanLimitDetails]:
    """The details when `text` is a plan-limit refusal, else None."""
    for marker, kind, tail in _FORMS:
        start = text.find(marker)
        if start < 0:
            continue
        rest = text[start + len(marker):]
        mid = rest.find(tail)
        if mid < 0:
            continue
        counts = _COUNTS.match(rest[:mid])
        after = rest[mid + len(tail):]
        dot = after.find(". ")
        if not counts or dot < 0:
            continue
        name = after[:dot].strip().lower()
        plan = "free" if name == "the free plan" else "" if name == "your plan" else name
        upgrade = _UPGRADE.search(text)
        body = (text[: upgrade.start()] if upgrade else text)[start:].strip()
        return PlanLimitDetails(
            kind=kind,
            plan=plan,
            used=float(counts.group(1)),
            limit=float(counts.group(2)),
            upgrade_url=upgrade.group(1) if upgrade else None,
            message=body,
        )
    return None
