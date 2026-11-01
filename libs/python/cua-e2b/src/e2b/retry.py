"""The retry policy of e2b's control-plane client, as pure functions.

cua-e2b has no REST control plane to retry against (see `e2b.api`), so the HTTP
transports that apply this policy (`RetryableTransport`, `AsyncRetryableTransport`)
are not mirrored. The parts user code can reach are: validating `retries=`
(`resolve_max_retries`, used by `ConnectionConfig`), and the `Retry-After` and
replay rules, which behave exactly as e2b's.
"""

from __future__ import annotations

import re
from typing import Any, List, Optional, Tuple

from e2b.exceptions import InvalidArgumentException

MAX_RETRY_AFTER_SECONDS = 2_147_483_647
MAX_RETRY_WAIT_WITHOUT_TIMEOUT_SECONDS = 60.0
BACKOFF_BASE_SECONDS = 0.1
BACKOFF_MAX_SECONDS = 10.0
BACKOFF_JITTER_MIN = 0.5
RETRYABLE_STATUSES = frozenset({429, 502, 503})

# GET/PUT/PATCH/DELETE are idempotent by HTTP semantics. A POST is replayed only
# when it is listed in REPLAYABLE_OPERATIONS: a second send is a no-op or fails
# with a 404/409 that callers already tolerate.
REPLAYABLE_METHODS = frozenset({"GET", "PUT", "PATCH", "DELETE"})
REPLAYABLE_OPERATIONS: List[Tuple[str, "re.Pattern[str]"]] = [
    ("POST", re.compile(r"^/sandboxes/[^/]+/(pause|resume|timeout|refreshes)$")),
    ("POST", re.compile(r"^/(v2/)?sandboxes/[^/]+/connect$")),
    ("POST", re.compile(r"^/v3/templates$")),
    ("POST", re.compile(r"^/v2/templates/[^/]+/builds/[^/]+$")),
    ("POST", re.compile(r"^/templates/tags$")),
    ("POST", re.compile(r"^/nodes/[^/]+$")),
    ("POST", re.compile(r"^/admin/teams/[^/]+/(sandboxes/kill|builds/cancel)$")),
]


def resolve_max_retries(retries: int) -> int:
    if isinstance(retries, bool) or not isinstance(retries, int) or retries < 0:
        raise InvalidArgumentException(
            f"Invalid retries={retries!r}: expected a non-negative integer."
        )
    return retries


def parse_retry_after(value: Optional[str]) -> Optional[int]:
    """Delta-seconds of a `Retry-After` header; `None` for an HTTP-date, a
    malformed value or one beyond 2^31-1."""
    if not value:
        return None

    value = value.strip()
    if not value.isascii() or not value.isdecimal() or len(value) > 10:
        return None
    delay = int(value)
    return delay if delay <= MAX_RETRY_AFTER_SECONDS else None


def is_replayable(request: Any) -> bool:
    """Whether `request` (anything with `.method` and `.url.path`, such as an
    `httpx.Request`) may be sent again after a 502 or a network error that may have
    occurred once it was written."""
    if request.method in REPLAYABLE_METHODS:
        return True
    path = request.url.path
    return any(
        request.method == method and pattern.match(path)
        for method, pattern in REPLAYABLE_OPERATIONS
    )
