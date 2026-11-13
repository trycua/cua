"""OpenJev System One adapter for the jev-use DecisionModel seam.

This adapter is intentionally narrow: it consumes a validated DecisionRequest,
sends that bounded caller-owned request as System One state, and returns scores
for the same supplied candidate IDs. It does not construct actions or retry
mutations.
"""

from __future__ import annotations

import json
import math
import os
import socket
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable, Literal, Mapping
from urllib.parse import urlsplit

from choose_action import REQUEST_SCHEMA_V2
from decision_models import DecisionRequest, ModelScores

DEFAULT_MODEL = "openjev"
DEFAULT_TIMEOUT_MS = 2_500
MIN_TIMEOUT_MS = 100
MAX_TIMEOUT_MS = 60_000
MAX_RESPONSE_BYTES = 256 * 1024

OpenJevErrorCode = Literal[
    "missing_base_url",
    "invalid_url",
    "insecure_credentials",
    "timeout",
    "http_error",
    "response_too_large",
    "invalid_response",
]


class OpenJevError(RuntimeError):
    """A bounded OpenJev transport or response failure."""

    def __init__(self, code: OpenJevErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


_HTTP_OPENER = urllib.request.build_opener(_NoRedirect)


@dataclass(frozen=True)
class OpenJevConfig:
    base_url: str
    api_key: str = ""
    model: str = DEFAULT_MODEL
    timeout_ms: int = DEFAULT_TIMEOUT_MS


def read_openjev_config(env: Mapping[str, str] | None = None) -> OpenJevConfig:
    source = env if env is not None else os.environ
    base_url = source.get("OPENJEV_BASE_URL", "").strip()
    api_key = source.get("OPENJEV_API_KEY", "").strip()
    model = source.get("OPENJEV_MODEL", "").strip() or DEFAULT_MODEL
    raw_timeout = source.get("OPENJEV_TIMEOUT_MS", "").strip()
    try:
        timeout_ms = int(raw_timeout) if raw_timeout else DEFAULT_TIMEOUT_MS
    except ValueError:
        timeout_ms = DEFAULT_TIMEOUT_MS
    timeout_ms = min(MAX_TIMEOUT_MS, max(MIN_TIMEOUT_MS, timeout_ms))
    return OpenJevConfig(base_url, api_key, model, timeout_ms)


def validate_openjev_base_url(base_url: str, *, has_api_key: bool) -> str:
    value = base_url.strip().rstrip("/")
    if not value:
        raise OpenJevError("missing_base_url", "OPENJEV_BASE_URL is not set")
    try:
        parsed = urlsplit(value)
    except ValueError as error:
        raise OpenJevError("invalid_url", "OPENJEV_BASE_URL is not a valid URL") from error
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise OpenJevError(
            "invalid_url",
            "OPENJEV_BASE_URL must use http:// or https:// and include a host",
        )
    if parsed.username or parsed.password:
        raise OpenJevError(
            "invalid_url",
            "OPENJEV_BASE_URL must not embed credentials",
        )
    if parsed.query or parsed.fragment:
        raise OpenJevError(
            "invalid_url",
            "OPENJEV_BASE_URL must not contain a query or fragment",
        )
    if has_api_key and parsed.scheme != "https":
        raise OpenJevError(
            "insecure_credentials",
            "OPENJEV_API_KEY may only be sent to an https:// endpoint",
        )
    return value


def systemone_url(base_url: str) -> str:
    root = base_url.rstrip("/")
    if root.endswith("/v1/systemone"):
        return root
    return root + "/systemone" if root.endswith("/v1") else root + "/v1/systemone"


def decision_request_wire(request: DecisionRequest) -> dict[str, Any]:
    """Reconstruct the validated Cua request without adding executable authority."""
    wire: dict[str, Any] = {
        "schema": request.schema,
        "goal": request.goal,
        "capture_id": request.capture_id,
        "regions": [dict(item) for item in request.regions],
        "history": [dict(item) for item in request.history],
        "candidates": [dict(item) for item in request.candidates],
    }
    if request.schema == REQUEST_SCHEMA_V2:
        wire["snapshot_id"] = request.snapshot_id
        wire["elements"] = [dict(item) for item in request.elements]
        wire["progress"] = [dict(item) for item in request.progress]
    return wire


OpenJevTransport = Callable[
    [str, bytes, Mapping[str, str], float],
    Mapping[str, Any],
]


def _default_transport(
    url: str,
    body: bytes,
    headers: Mapping[str, str],
    timeout_s: float,
) -> Mapping[str, Any]:
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers=dict(headers),
    )
    try:
        with _HTTP_OPENER.open(request, timeout=timeout_s) as response:
            payload = response.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as error:
        if 300 <= error.code < 400:
            raise OpenJevError(
                "http_error",
                "OpenJev endpoint redirects are refused",
            ) from None
        raise OpenJevError(
            "http_error",
            "OpenJev endpoint returned HTTP " + str(error.code),
        ) from None
    except socket.timeout:
        raise OpenJevError("timeout", "OpenJev request timed out") from None
    except TimeoutError:
        raise OpenJevError("timeout", "OpenJev request timed out") from None
    except (urllib.error.URLError, OSError) as error:
        reason = getattr(error, "reason", None)
        if isinstance(reason, (socket.timeout, TimeoutError)):
            raise OpenJevError("timeout", "OpenJev request timed out") from None
        raise OpenJevError(
            "http_error",
            "OpenJev endpoint is unreachable",
        ) from None
    if len(payload) > MAX_RESPONSE_BYTES:
        raise OpenJevError("response_too_large", "OpenJev response is too large")
    try:
        value = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise OpenJevError("invalid_response", "OpenJev response is not JSON") from None
    if not isinstance(value, Mapping):
        raise OpenJevError("invalid_response", "OpenJev response is not an object")
    return value


def _finite_unit(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(float(value))
        and 0 <= float(value) <= 1
    )


class OpenJevDecisionModel:
    """DecisionModel backed by a TypeSafe/OpenJev-compatible System One server."""

    name = "openjev"

    def __init__(
        self,
        config: OpenJevConfig | None = None,
        *,
        transport: OpenJevTransport | None = None,
    ) -> None:
        self.config = config or read_openjev_config()
        self.transport = transport or _default_transport

    def score(self, request: DecisionRequest) -> ModelScores:
        base_url = validate_openjev_base_url(
            self.config.base_url,
            has_api_key=bool(self.config.api_key),
        )
        payload = {
            "model": self.config.model,
            "state": decision_request_wire(request),
            "questions": {
                "candidate": {
                    "type": "choice",
                    "instructions": request.goal,
                    "criteria": request.criteria,
                }
            },
        }
        body = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        headers = {
            "Accept": "application/json",
            "Content-Type": "application/json",
        }
        if self.config.api_key:
            headers["Authorization"] = "Bearer " + self.config.api_key
        response = self.transport(
            systemone_url(base_url),
            body,
            headers,
            self.config.timeout_ms / 1000.0,
        )
        answers = response.get("answers")
        if not isinstance(answers, Mapping):
            raise OpenJevError("invalid_response", "OpenJev response has no answers object")
        answer = answers.get("candidate")
        if not isinstance(answer, Mapping) or answer.get("type") != "choice":
            raise OpenJevError(
                "invalid_response",
                "OpenJev candidate answer is not a choice",
            )
        selected = answer.get("choice")
        confidence = answer.get("confidence")
        probabilities = answer.get("probabilities")
        if not isinstance(selected, str):
            raise OpenJevError("invalid_response", "OpenJev choice is not a string")
        if not _finite_unit(confidence):
            raise OpenJevError("invalid_response", "OpenJev confidence is invalid")
        if not isinstance(probabilities, Mapping):
            raise OpenJevError("invalid_response", "OpenJev probabilities are missing")
        model = response.get("model")
        model_name = model if isinstance(model, str) and model.strip() else self.config.model
        return ModelScores(
            {str(key): value for key, value in probabilities.items()},
            model_name,
            selected_id=selected,
            confidence=float(confidence),
        )
