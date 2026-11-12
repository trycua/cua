"""Choose one supplied candidate through a warm, loopback S1 decision service.

Loading Cua-S1 costs 10 to 35 seconds, which does not fit inside Driver's
60-second capture lifetime when every step starts a new process. The native
runners therefore reach an already loaded model through a small HTTP service
on the loopback interface. ``CUA_S1_DECISION_URL`` names its decide endpoint,
for example ``http://127.0.0.1:8791/decide``.

The service receives the same validated ``cua.jev_choice_request_v1`` or
``_v2`` request that every other provider sees and returns one
``cua.decision_choice_v1`` response. This module checks that response strictly
before the runner resolves the selected ID to its own immutable candidate. The
request never contains element tokens, values, secrets, or pixels.
"""

from __future__ import annotations

import ipaddress
import json
import math
import os
import urllib.error
import urllib.request
from typing import Any, Mapping
from urllib.parse import urlsplit

from choose_action import validate_request

DECISION_SCHEMA = "cua.decision_choice_v1"
DECISION_KINDS = frozenset({"selected", "reobserve", "abstain", "error"})
URL_ENV = "CUA_S1_DECISION_URL"
DEFAULT_TIMEOUT_S = 30.0
MAX_RESPONSE_BYTES = 256 * 1024


class S1ServiceError(RuntimeError):
    """The service was unreachable or returned an unusable decision."""


def s1_service_url(value: str | None = None) -> str:
    """Return the configured decide URL; it must be plain HTTP on loopback."""
    url = (value if value is not None else os.environ.get(URL_ENV, "")).strip()
    if not url:
        raise S1ServiceError(f"{URL_ENV} is not set")
    parts = urlsplit(url)
    host = parts.hostname or ""
    try:
        loopback = host == "localhost" or ipaddress.ip_address(host).is_loopback
    except ValueError:
        loopback = False
    if parts.scheme != "http" or not loopback or parts.username or parts.password:
        raise S1ServiceError(f"{URL_ENV} must be an http:// URL on the loopback interface")
    return url


def _finite_unit(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and 0 <= value <= 1
    )


def validate_decision(decision: Any, request: Mapping[str, Any]) -> dict[str, Any]:
    """Check a ``cua.decision_choice_v1`` response against the request it answers."""
    if not isinstance(decision, dict) or decision.get("schema") != DECISION_SCHEMA:
        raise S1ServiceError("response is not a cua.decision_choice_v1 decision")
    kind = decision.get("kind")
    if kind not in DECISION_KINDS:
        raise S1ServiceError("response kind is not supported")
    if decision.get("capture_id") != request.get("capture_id"):
        raise S1ServiceError("response capture_id does not match the request")
    if kind == "error":
        raise S1ServiceError(f"S1 decision failed: {decision.get('reason') or 'unknown'}")
    ids = [candidate["id"] for candidate in request["candidates"]]
    selected = decision.get("selected_id")
    if selected not in ids:
        raise S1ServiceError("S1 selected an ID that was not supplied")
    expected_kind = selected if selected in {"reobserve", "abstain"} else "selected"
    if kind != expected_kind:
        raise S1ServiceError("response kind does not match the selected ID")
    probabilities = decision.get("probabilities")
    if (
        not isinstance(probabilities, dict)
        or set(probabilities) != set(ids)
        or not all(_finite_unit(value) for value in probabilities.values())
    ):
        raise S1ServiceError("response probabilities do not match the candidate set")
    confidence = decision.get("confidence")
    if not _finite_unit(confidence):
        raise S1ServiceError("response confidence must be a finite number in [0, 1]")
    return decision


def choose_s1_service(
    request: Mapping[str, Any], url: str | None = None, timeout: float = DEFAULT_TIMEOUT_S
) -> tuple[str | None, float, dict[str, float]]:
    """POST one validated request and return ``(selected_id, confidence, probabilities)``."""
    validated = validate_request(request)
    body = json.dumps(request, separators=(",", ":")).encode()
    http_request = urllib.request.Request(
        s1_service_url(url), data=body, method="POST", headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(http_request, timeout=timeout) as response:  # noqa: S310
            payload = response.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as error:
        raise S1ServiceError(f"S1 service returned HTTP {error.code}") from None
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        raise S1ServiceError(f"S1 service is unreachable: {type(error).__name__}") from None
    if len(payload) > MAX_RESPONSE_BYTES:
        raise S1ServiceError("S1 response is too large")
    try:
        decision = json.loads(payload)
    except json.JSONDecodeError:
        raise S1ServiceError("S1 response is not JSON") from None
    decision = validate_decision(decision, validated)
    return decision["selected_id"], float(decision["confidence"]), dict(decision["probabilities"])
