"""Generalized Jev backends for the jev-use example.

Ports the fail-open Jev adapter semantics from Kevin's Hermes / oh-my-pi work
(agentweb ``jevAdapter.ts``, Hermes ``tools/computer_use/system_one.py`` and the
``computer.decide()`` decision lane) into the cua jev-use example, and adds a
first-class backend for a locally running Jev.

Backends
--------
``mock``
    Deterministic credential-free chooser. Default; used by CI and dry runs.
``typesafe``
    TypeSafe cloud System One. Requires ``JEV_API_KEY`` (or ``TYPESAFE_API_KEY``).
``openjev``
    Any OpenJev-compatible System One HTTP endpoint. Requires ``JEV_BASE_URL``
    (or ``OPENJEV_BASE_URL``); the API key is optional.
``local``
    A locally running Jev on loopback (default ``http://127.0.0.1:8787``). The
    URL must stay on loopback; no API key is required.

All HTTP backends speak the same wire format -- ``POST {base}/v1/systemone``
with ``{state, model, questions}`` returning ``{model, answers, usage}`` --
over the standard library only (no SDK dependency). Transport problems never
raise into the caller: they come back as a skipped :class:`JevOutcome` with a
machine-readable reason, mirroring the Hermes/agentweb fail-open contract.
Malformed model output is fail-closed instead: it is reported as
``invalid_response`` rather than trusted.
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

JevBackendName = Literal["mock", "typesafe", "openjev", "local"]

BACKENDS: tuple[str, ...] = ("mock", "typesafe", "openjev", "local")

#: Skip reasons for the fail-open envelope. ``invalid_response`` is a live
#: variant: malformed model output is reported, never trusted.
JevSkipReason = Literal[
    "disabled",
    "missing_credentials",
    "missing_base_url",
    "timeout",
    "http_error",
    "invalid_response",
    "validation_error",
]

DEFAULT_MODEL = "jev-latest"
DEFAULT_TIMEOUT_MS = 2500
DEFAULT_TYPESAFE_BASE_URL = "https://api.typesafe.ai"
DEFAULT_LOCAL_JEV_URL = "http://127.0.0.1:8787"

_RESERVED_IDS = ("reobserve", "abstain")
_LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1"}


class JevTransportError(Exception):
    """The System One HTTP request failed (network, timeout, HTTP status)."""


class JevProtocolError(Exception):
    """The System One HTTP response was not usable (bad JSON / wire shape)."""


@dataclass(frozen=True)
class JevConfig:
    backend: str
    base_url: str
    api_key: str
    model: str
    timeout_ms: int


def _env(env: Mapping[str, str], *names: str) -> str:
    for name in names:
        value = env.get(name)
        if value is not None and value.strip():
            return value.strip()
    return ""


def read_jev_config(env: Mapping[str, str] | None = None) -> JevConfig:
    """Read the Jev backend configuration from the environment.

    ``JEV_BACKEND`` selects ``mock`` (default), ``typesafe``, ``openjev`` or
    ``local``; ``live`` is accepted as an alias for ``typesafe``. An
    explicit unknown backend is rejected rather than silently becoming ``mock``.
    ``TYPESAFE_*`` variables are honored as
    aliases for the shared settings.
    """
    source: Mapping[str, str] = env if env is not None else os.environ
    backend = _env(source, "JEV_BACKEND").lower() or "mock"
    if backend == "live":
        backend = "typesafe"
    if backend not in BACKENDS:
        raise ValueError(
            f"JEV_BACKEND must be one of {', '.join(BACKENDS)} (or deprecated live), got {backend!r}"
        )

    base_url = _env(source, "JEV_BASE_URL", "TYPESAFE_BASE_URL", "OPENJEV_BASE_URL")
    if not base_url:
        if backend == "typesafe":
            base_url = DEFAULT_TYPESAFE_BASE_URL
        elif backend == "local":
            base_url = DEFAULT_LOCAL_JEV_URL

    timeout_raw = _env(source, "JEV_TIMEOUT_MS", "TYPESAFE_TIMEOUT_MS")
    try:
        timeout_ms = int(timeout_raw)
    except (TypeError, ValueError):
        timeout_ms = DEFAULT_TIMEOUT_MS
    if timeout_ms <= 0:
        timeout_ms = DEFAULT_TIMEOUT_MS

    return JevConfig(
        backend=backend,
        base_url=base_url.rstrip("/"),
        api_key=_env(source, "JEV_API_KEY", "TYPESAFE_API_KEY"),
        model=_env(source, "JEV_MODEL", "TYPESAFE_MODEL") or DEFAULT_MODEL,
        timeout_ms=timeout_ms,
    )


def systemone_url(base_url: str) -> str:
    """Return the System One endpoint for a base URL."""
    root = base_url.rstrip("/")
    return root + "/systemone" if root.endswith("/v1") else root + "/v1/systemone"


def validate_loopback_url(url: str) -> str:
    """Ensure a ``local`` backend URL stays on loopback. Returns normalized URL."""
    from urllib.parse import urlsplit

    parsed = urlsplit(url)
    if parsed.scheme != "http" or parsed.hostname not in _LOOPBACK_HOSTS:
        raise ValueError("local Jev backend must be an http:// loopback URL")
    if parsed.username or parsed.password:
        raise ValueError("local Jev backend URL must not embed credentials")
    return url.rstrip("/")

def validate_remote_url(url: str, *, has_api_key: bool) -> str:
    """Validate a non-local backend URL before task state or credentials leave the host."""
    from urllib.parse import urlsplit

    parsed = urlsplit(url)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ValueError("remote Jev backend must use an http:// or https:// URL")
    if parsed.username or parsed.password:
        raise ValueError("remote Jev backend URL must not embed credentials")
    if has_api_key and parsed.scheme != "https":
        raise ValueError("Jev API keys may only be sent to an https:// backend")
    return url.rstrip("/")


@dataclass(frozen=True)
class ChoiceAnswer:
    question_id: str
    choice: str
    confidence: float
    probabilities: dict[str, float]


def _is_finite_unit(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(float(value))
        and 0.0 <= float(value) <= 1.0
    )


def validate_choice_answer(
    question_id: str, answer: Mapping[str, Any], allowed_ids: set[str]
) -> ChoiceAnswer:
    """Fail-closed validation of one System One choice answer.

    Ports oh-my-pi's ``validateChoiceAnswer``: the probability mass must be
    ~1 (tolerance 0.02), every value finite in [0,1], the key set must equal
    the allowed candidate ids exactly, and the choice must be the argmax.
    Anything else raises :class:`JevProtocolError`.
    """
    if not isinstance(answer, Mapping) or answer.get("type") != "choice":
        raise JevProtocolError(f"question {question_id!r}: expected a choice answer")
    choice = answer.get("choice")
    if not isinstance(choice, str) or choice not in allowed_ids:
        raise JevProtocolError(f"question {question_id!r}: unknown choice {choice!r}")
    confidence = answer.get("confidence")
    if not _is_finite_unit(confidence):
        raise JevProtocolError(f"question {question_id!r}: invalid confidence")
    raw_probs = answer.get("probabilities")
    if not isinstance(raw_probs, Mapping):
        raise JevProtocolError(f"question {question_id!r}: missing probabilities")
    probabilities: dict[str, float] = {}
    for candidate_id, value in raw_probs.items():
        if candidate_id not in allowed_ids:
            raise JevProtocolError(
                f"question {question_id!r}: probability for unknown id {candidate_id!r}"
            )
        if not _is_finite_unit(value):
            raise JevProtocolError(
                f"question {question_id!r}: invalid probability for {candidate_id!r}"
            )
        probabilities[candidate_id] = float(value)
    if set(probabilities) != allowed_ids:
        raise JevProtocolError(
            f"question {question_id!r}: probability keys do not match candidate ids"
        )
    mass = sum(probabilities.values())
    if abs(mass - 1.0) > 0.02:
        raise JevProtocolError(
            f"question {question_id!r}: probability mass {mass:.3f} != 1"
        )
    argmax = max(probabilities, key=lambda key: probabilities[key])
    if argmax != choice:
        raise JevProtocolError(
            f"question {question_id!r}: choice {choice!r} is not the argmax {argmax!r}"
        )
    return ChoiceAnswer(
        question_id=question_id,
        choice=choice,
        confidence=float(confidence),  # type: ignore[arg-type]
        probabilities=probabilities,
    )


def validate_noul_answer(question_id: str, answer: Mapping[str, Any]) -> float:
    """Validate one System One noul answer, returning the probability."""
    if not isinstance(answer, Mapping) or answer.get("type") != "noul":
        raise JevProtocolError(f"question {question_id!r}: expected a noul answer")
    value = answer.get("noul")
    if not _is_finite_unit(value):
        raise JevProtocolError(f"question {question_id!r}: invalid noul value")
    return float(value)  # type: ignore[arg-type]


class SystemOneHttpClient:
    """Minimal System One HTTP client over the standard library.

    ``transport`` is an injectable ``(url, payload, headers, timeout) -> dict``
    used by tests; the default performs a real ``urllib`` POST.
    """

    def __init__(
        self,
        config: JevConfig,
        transport: Callable[[str, dict[str, Any], dict[str, str], float], dict[str, Any]]
        | None = None,
    ) -> None:
        self.config = config
        self.transport = transport or self._default_transport

    @staticmethod
    def _default_transport(
        url: str, payload: dict[str, Any], headers: dict[str, str], timeout: float
    ) -> dict[str, Any]:
        body = json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            url, data=body, headers=headers, method="POST"
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                status = getattr(response, "status", 200)
                raw = response.read()
        except socket.timeout as error:
            raise JevTransportError(f"request timed out: {error}") from error
        except urllib.error.HTTPError as error:
            detail = ""
            try:
                detail = error.read().decode("utf-8", "replace")[:200]
            except Exception:
                pass
            raise JevTransportError(
                f"HTTP {error.code}{': ' + detail if detail else ''}"
            ) from error
        except (urllib.error.URLError, OSError) as error:
            raise JevTransportError(f"transport error: {error}") from error
        if status < 200 or status >= 300:
            raise JevTransportError(f"HTTP {status}")
        try:
            parsed = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise JevProtocolError(f"response is not JSON: {error}") from error
        if not isinstance(parsed, dict):
            raise JevProtocolError("response was not a JSON object")
        return parsed

    def ask(
        self, *, state: Mapping[str, Any], questions: Mapping[str, Any]
    ) -> dict[str, Any]:
        """POST one System One judgment and return the parsed response."""
        if self.config.backend == "local":
            validate_loopback_url(self.config.base_url or DEFAULT_LOCAL_JEV_URL)
        elif self.config.backend in {"typesafe", "openjev"}:
            validate_remote_url(self.config.base_url, has_api_key=bool(self.config.api_key))
        base_url = self.config.base_url
        if not base_url:
            raise JevTransportError("no base URL configured")
        headers = {"Accept": "application/json", "Content-Type": "application/json"}
        if self.config.api_key:
            headers["Authorization"] = f"Bearer {self.config.api_key}"
        payload = {
            "state": dict(state),
            "model": self.config.model,
            "questions": dict(questions),
        }
        parsed = self.transport(
            systemone_url(base_url),
            payload,
            headers,
            self.config.timeout_ms / 1000.0,
        )
        if not isinstance(parsed, dict):
            raise JevProtocolError("transport returned a non-object")
        answers = parsed.get("answers")
        if not isinstance(answers, dict):
            raise JevProtocolError("response has no answers object")
        return {
            "model": parsed["model"]
            if isinstance(parsed.get("model"), str) and parsed["model"].strip()
            else self.config.model,
            "answers": answers,
            "usage": parsed.get("usage")
            if isinstance(parsed.get("usage"), dict)
            else None,
        }


@dataclass(frozen=True)
class JevDecision:
    selected_id: str
    confidence: float
    probabilities: dict[str, float]
    model: str | None
    backend: str


@dataclass(frozen=True)
class JevOutcome:
    """Fail-open envelope: ``ok`` carries a decision, otherwise ``reason``."""

    ok: bool
    decision: JevDecision | None = None
    reason: str | None = None
    message: str | None = None
    backend: str = "mock"


def _skipped(backend: str, reason: str, message: str) -> JevOutcome:
    return JevOutcome(ok=False, reason=reason, message=message, backend=backend)


def validate_criteria(criteria: Mapping[str, str]) -> dict[str, str]:
    if not isinstance(criteria, Mapping) or not criteria:
        raise ValueError("criteria must be a non-empty mapping")
    cleaned: dict[str, str] = {}
    for candidate_id, description in criteria.items():
        if not isinstance(candidate_id, str) or not candidate_id.strip():
            raise ValueError("candidate ids must be non-empty strings")
        if not isinstance(description, str) or not description.strip():
            raise ValueError(f"candidate {candidate_id!r} needs a description")
        if candidate_id in cleaned:
            raise ValueError("candidate set contains duplicate IDs")
        cleaned[candidate_id] = description
    return cleaned


def choose_with_backend(
    config: JevConfig,
    *,
    goal: str,
    observation: Mapping[str, Any],
    criteria: Mapping[str, str],
    transport: Callable[[str, dict[str, Any], dict[str, str], float], dict[str, Any]]
    | None = None,
) -> JevOutcome:
    """Choose one candidate id through the configured Jev backend.

    Never raises for backend problems: transport failures, timeouts and
    malformed model output come back as a skipped :class:`JevOutcome` with a
    reason. Only caller-side misuse (bad criteria) raises :class:`ValueError`.
    """
    try:
        cleaned = validate_criteria(criteria)
    except ValueError as error:
        return _skipped(config.backend, "validation_error", str(error))
    if not isinstance(goal, str) or not goal.strip():
        return _skipped(config.backend, "validation_error", "goal must be non-empty")

    if config.backend == "mock":
        selected = next(
            (cid for cid in cleaned if cid not in _RESERVED_IDS),
            "reobserve" if "reobserve" in cleaned else next(iter(cleaned)),
        )
        probabilities = {cid: float(cid == selected) for cid in cleaned}
        return JevOutcome(
            ok=True,
            decision=JevDecision(
                selected_id=selected,
                confidence=1.0,
                probabilities=probabilities,
                model="mock",
                backend="mock",
            ),
            backend="mock",
        )

    if config.backend == "typesafe" and not config.api_key:
        return _skipped(
            config.backend,
            "missing_credentials",
            "JEV_BACKEND=typesafe but no API key is set "
            "(JEV_API_KEY or TYPESAFE_API_KEY); skipped fail-open.",
        )
    if config.backend == "openjev" and not config.base_url:
        return _skipped(
            config.backend,
            "missing_base_url",
            "JEV_BACKEND=openjev but no base URL is set "
            "(JEV_BASE_URL or OPENJEV_BASE_URL); skipped fail-open.",
        )
    if config.backend == "local":
        try:
            validate_loopback_url(config.base_url or DEFAULT_LOCAL_JEV_URL)
        except ValueError as error:
            return _skipped(config.backend, "validation_error", str(error))
    elif config.backend in {"typesafe", "openjev"}:
        try:
            validate_remote_url(config.base_url, has_api_key=bool(config.api_key))
        except ValueError as error:
            return _skipped(config.backend, "validation_error", str(error))

    client = SystemOneHttpClient(config, transport=transport)
    questions = {
        "candidate": {
            "type": "choice",
            "instructions": "Select exactly one supplied candidate ID.",
            "criteria": cleaned,
        }
    }
    try:
        response = client.ask(
            state={"goal": goal, "observation": dict(observation)},
            questions=questions,
        )
        answer = validate_choice_answer(
            "candidate", response["answers"].get("candidate"), set(cleaned)
        )
    except JevTransportError as error:
        message = str(error)
        reason = "timeout" if "timed out" in message else "http_error"
        return _skipped(config.backend, reason, message)
    except JevProtocolError as error:
        return _skipped(config.backend, "invalid_response", str(error))
    return JevOutcome(
        ok=True,
        decision=JevDecision(
            selected_id=answer.choice,
            confidence=answer.confidence,
            probabilities=answer.probabilities,
            model=response["model"],
            backend=config.backend,
        ),
        backend=config.backend,
    )


def describe_backend(config: JevConfig) -> dict[str, Any]:
    """Redacted backend description for logs and evidence (no secrets)."""
    return {
        "backend": config.backend,
        "base_url": config.base_url,
        "model": config.model,
        "timeout_ms": config.timeout_ms,
        "has_api_key": bool(config.api_key),
    }
