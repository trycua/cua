"""
Opt-in callback that asks a loopback vision endpoint for CAPTCHA hints.

When the endpoint reports a possible challenge in a screenshot, the callback
can inject a bounded, explicitly unverified hint into the agent's message
stream. Unreadable, unsafe, or unbound output is discarded and a later
screenshot can retry. The endpoint's process and any forwarding or retention
it performs are outside this callback's trust boundary.

The injected instruction is an ordinary user message. Logging and trajectory
callbacks later in the chain may therefore record the bounded answer or action.
Analysis runs when an agent loop emits a screenshot callback. A hint is eligible
for injection only after an identity-bearing computer-call hook binds the image
digest and call ID in the callback's preprocessed message list. That does not
prove the live page is unchanged or that a loop's final model payload preserves
the image bytes. Model loops that capture screenshots entirely inside prediction
and do not emit the binding hook are not supported.

Usage:

    from cua_agent.callbacks import CaptchaHintCallback

    helper = CaptchaHintCallback(
        api_base="http://127.0.0.1:1234/v1",
        model="bionic-vision-light",
    )
    agent = ComputerAgent(
        model="claude-sonnet-4-5-20250929",
        callbacks=[helper],
    )

"""

from __future__ import annotations

import asyncio
import base64
import binascii
import contextvars
import hashlib
import ipaddress
import json
import logging
import math
import re
import time
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Union
from urllib.parse import urlparse

import httpx

from .base import AsyncCallbackHandler

logger = logging.getLogger(__name__)

_MAX_RESPONSE_BYTES = 1024 * 1024
_MAX_CONSECUTIVE_TRANSPORT_FAILURES = 2
_SCREENSHOT_NAMES = {"screenshot", "screenshot_before", "screenshot_after"}
_ANALYSIS_HINT = "hint"
_ANALYSIS_NOT_DETECTED = "not_detected"
_ANALYSIS_DETECTED_UNSOLVED = "detected_unsolved"
_ANALYSIS_UNKNOWN = "unknown"


@dataclass(frozen=True)
class _PendingMessage:
    result: Dict[str, Any]
    screenshot_digest: str
    screenshot_sequence: int
    call_id: Optional[str] = None


_JSON_PROMPT = (
    "Look at this screenshot. Is there a CAPTCHA or bot-verification widget "
    "embedded in the page? A CAPTCHA is a specific challenge widget like a "
    "distorted-text image, a reCAPTCHA/hCaptcha checkbox, or a Cloudflare "
    "Turnstile box.\n\n"
    "These are NOT CAPTCHAs — ignore them:\n"
    "- OS permission dialogs (Allow/Don't Allow)\n"
    "- Cookie consent banners\n"
    "- Location prompts\n"
    "- Login forms without a CAPTCHA widget\n"
    "- Pages that mention CAPTCHAs in their text but have no actual challenge\n\n"
    "If a real CAPTCHA widget is present, respond with ONLY:\n"
    '{"captcha": true, "answer": "<the text to enter>", "type": "<type>"}\n'
    "If it is a text CAPTCHA, the answer is the exact distorted characters. "
    "If it is a checkbox ('I am not a robot' / 'I am human' / "
    "'Verify you are human'), "
    "set answer to 'click_checkbox' and type to 'checkbox'.\n"
    "If there is no CAPTCHA widget, respond with:\n"
    '{"captcha": false}'
)

_DETECT_PROMPT = (
    "Is there a CAPTCHA widget embedded in this page? "
    "A CAPTCHA is a reCAPTCHA checkbox, hCaptcha checkbox, Cloudflare Turnstile box, "
    "or distorted-text image challenge.\n"
    "OS dialogs, cookie banners, location prompts, and login forms are NOT CAPTCHAs.\n"
    "Answer ONLY: YES or NO"
)

_SOLVE_PROMPT = (
    "Look at the CAPTCHA or verification challenge in this image. "
    "What type is it and what should be done to solve it?\n"
    "- If it is a text CAPTCHA, reply with the exact characters shown.\n"
    "- If it is a checkbox like 'I am not a robot', reply: CLICK_CHECKBOX\n"
    "- If it asks to select images, reply: IMAGE_SELECT\n"
    "Reply with ONLY the answer, nothing else."
)

_INJECT_TEMPLATE = (
    "UNVERIFIED VISION HINT — a helper found a possible CAPTCHA text "
    'answer: "{answer}". Confirm that the current screenshot shows a matching '
    "challenge and follow the task's normal site-policy or user-handoff rules "
    "before using it; otherwise ignore this hint."
)

_CHECKBOX_INJECT = (
    "UNVERIFIED VISION HINT — a helper found a possible verification checkbox. "
    "Confirm that it is present in the current screenshot and follow the task's "
    "normal site-policy or user-handoff rules before acting; otherwise ignore "
    "this hint."
)

_IMAGE_SELECT_INJECT = (
    "UNVERIFIED VISION HINT — a helper found a possible image-selection "
    "challenge. Inspect the current screenshot and follow the task's normal "
    "site-policy or user-handoff rules if that challenge is actually present."
)

_UNSOLVED_INJECT = (
    "UNVERIFIED VISION OBSERVATION — a helper found a possible "
    "verification challenge but no bounded answer. Confirm that the current "
    "screenshot shows that challenge. If it does, stop ordinary retries and "
    "follow the task's normal site-policy or user-handoff rules."
)


class CaptchaHintCallback(AsyncCallbackHandler):
    """
    Sends agent screenshots to a loopback vision endpoint and injects only
    bounded, unverified challenge hints into an exactly bound computer-call
    result. Screenshot-only model loops are not supported.

    Strategy:
    1. Try a single-shot JSON prompt — fast path for text CAPTCHAs.
    2. If JSON comes back with captcha=true but no usable answer, ask a
       targeted solve-only follow-up. Reject a follow-up that contradicts a
       specific challenge type reported by the first pass.
    3. If JSON is malformed, try a simple YES/NO detection prompt, then
       solve if positive. A valid negative is final so ordinary pages consume
       only one request from the per-run budget.
    4. For possible image-selection challenges, preserve the main agent's
       normal site-policy and user-handoff behavior. Discard unreadable or
       unsafe output instead of forwarding it.

    Args:
        model: Vision model ID served by the loopback endpoint.
        api_base: Loopback-only LM Studio / OpenAI-compatible API base URL.
        max_tokens: Max tokens for the vision model response.
        timeout: Wall-clock limit for one screenshot analysis attempt and for
            each underlying HTTP request. Two consecutive request failures
            disable the helper for the rest of the current run.
        max_analysis_seconds_per_run: Aggregate wall-clock budget for helper
            analysis during one run. This includes successful and failed
            attempts, so slow negative responses cannot stall every screen.
        cooldown: Minimum seconds between screenshot scans in one run. The
            default is zero; digest deduplication and the request cap bound
            repeated work without skipping a newly observed challenge.
        max_requests_per_run: Hard cap on vision HTTP requests in one run.
        api_key: Optional bearer token for the loopback vision endpoint.
    """

    def __init__(
        self,
        model: str,
        api_base: str = "http://127.0.0.1:1234/v1",
        max_tokens: int = 200,
        timeout: float = 30.0,
        max_analysis_seconds_per_run: float = 60.0,
        cooldown: float = 0.0,
        max_requests_per_run: int = 12,
        api_key: Optional[str] = None,
    ) -> None:
        self.api_base = self._validate_api_base(api_base)
        if not isinstance(model, str) or not model.strip():
            raise ValueError("model must be a non-empty string")
        self.model = model
        self.max_tokens = max_tokens
        self.timeout = timeout
        self.max_analysis_seconds_per_run = max_analysis_seconds_per_run
        self.cooldown = cooldown
        self.max_requests_per_run = max_requests_per_run
        self.api_key = api_key

        if isinstance(max_tokens, bool) or not isinstance(max_tokens, int) or max_tokens <= 0:
            raise ValueError("max_tokens must be a positive integer")
        if (
            isinstance(timeout, bool)
            or not isinstance(timeout, (int, float))
            or not math.isfinite(timeout)
            or timeout <= 0
        ):
            raise ValueError("timeout must be finite and positive")
        if (
            isinstance(max_analysis_seconds_per_run, bool)
            or not isinstance(max_analysis_seconds_per_run, (int, float))
            or not math.isfinite(max_analysis_seconds_per_run)
            or max_analysis_seconds_per_run <= 0
        ):
            raise ValueError("max_analysis_seconds_per_run must be finite and positive")
        if (
            isinstance(cooldown, bool)
            or not isinstance(cooldown, (int, float))
            or not math.isfinite(cooldown)
            or cooldown < 0
        ):
            raise ValueError("cooldown must be finite and non-negative")
        if (
            isinstance(max_requests_per_run, bool)
            or not isinstance(max_requests_per_run, int)
            or max_requests_per_run <= 0
        ):
            raise ValueError("max_requests_per_run must be a positive integer")

        suffix = id(self)
        self._pending_message = contextvars.ContextVar[Optional[_PendingMessage]](
            f"captcha_pending_message_{suffix}", default=None
        )
        self._last_attempt_time = contextvars.ContextVar[float](
            f"captcha_last_attempt_time_{suffix}", default=0.0
        )
        self._analysis_seconds = contextvars.ContextVar[float](
            f"captcha_analysis_seconds_{suffix}", default=0.0
        )
        self._request_count = contextvars.ContextVar[int](
            f"captcha_request_count_{suffix}", default=0
        )
        self._screenshot_sequence = contextvars.ContextVar[int](
            f"captcha_screenshot_sequence_{suffix}", default=0
        )
        self._latest_screenshot_digest = contextvars.ContextVar[Optional[str]](
            f"captcha_latest_screenshot_digest_{suffix}", default=None
        )
        self._seen_screenshot_digests = contextvars.ContextVar[frozenset[str]](
            f"captcha_seen_screenshot_digests_{suffix}", default=frozenset()
        )
        self._attempt_transport_failed = contextvars.ContextVar[bool](
            f"captcha_attempt_transport_failed_{suffix}", default=False
        )
        self._analysis_status = contextvars.ContextVar[str](
            f"captcha_analysis_status_{suffix}", default=_ANALYSIS_UNKNOWN
        )
        self._consecutive_transport_failures = contextvars.ContextVar[int](
            f"captcha_consecutive_transport_failures_{suffix}", default=0
        )
        self._run_usage = contextvars.ContextVar[Optional[Dict[str, Union[int, float]]]](
            f"captcha_run_usage_{suffix}", default=None
        )

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        # A result is meaningful only for the run that produced its screenshot.
        # Do not carry a pending message or cooldown into an unrelated run.
        self._pending_message.set(None)
        self._last_attempt_time.set(0.0)
        self._analysis_seconds.set(0.0)
        self._request_count.set(0)
        self._screenshot_sequence.set(0)
        self._latest_screenshot_digest.set(None)
        self._seen_screenshot_digests.set(frozenset())
        self._attempt_transport_failed.set(False)
        self._analysis_status.set(_ANALYSIS_UNKNOWN)
        self._consecutive_transport_failures.set(0)
        self._run_usage.set(self._empty_run_usage())

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        if self._pending_message.get() is not None:
            self._update_run_usage(hints_discarded=1)
        self._pending_message.set(None)
        self._last_attempt_time.set(0.0)
        self._analysis_seconds.set(0.0)
        self._request_count.set(0)
        self._screenshot_sequence.set(0)
        self._latest_screenshot_digest.set(None)
        self._seen_screenshot_digests.set(frozenset())
        self._attempt_transport_failed.set(False)
        self._analysis_status.set(_ANALYSIS_UNKNOWN)
        self._consecutive_transport_failures.set(0)

    def get_run_usage(self) -> Dict[str, Union[int, float]]:
        """Return helper-model usage observed in the current or just-finished run.

        Token and cost totals include only responses whose endpoint usage data
        was valid. The corresponding complete/partial/missing response counters
        let a caller distinguish a complete total from incomplete evidence.
        The result is context-local, so callers running one callback concurrently
        should read it from the task that ran the agent.
        """

        return dict(self._run_usage.get() or self._empty_run_usage())

    async def on_screenshot(self, screenshot: Union[str, bytes], name: str = "screenshot") -> None:
        # Derived/annotated screenshots may replay historical images and are
        # not guaranteed to be the image attached to the next model turn.
        if name not in _SCREENSHOT_NAMES:
            return

        if isinstance(screenshot, bytes):
            screenshot_b64 = base64.b64encode(screenshot).decode("ascii")
        else:
            screenshot_b64 = screenshot

        digest = self._image_digest(screenshot_b64)
        sequence = self._screenshot_sequence.get()
        if digest != self._latest_screenshot_digest.get():
            sequence += 1
            self._screenshot_sequence.set(sequence)
            self._latest_screenshot_digest.set(digest)
            # A newer distinct screenshot invalidates any answer from an older
            # page state, even when this image was scanned earlier in the run
            # or is throttled below.
            if self._pending_message.get() is not None:
                self._update_run_usage(hints_discarded=1)
            self._pending_message.set(None)

        seen = self._seen_screenshot_digests.get()
        if digest in seen:
            return
        pending = self._pending_message.get()
        if pending is not None and pending.screenshot_digest == digest:
            return

        now = time.monotonic()
        if now - self._last_attempt_time.get() < self.cooldown:
            return
        if self._request_count.get() >= self.max_requests_per_run:
            return
        if self._consecutive_transport_failures.get() >= _MAX_CONSECUTIVE_TRANSPORT_FAILURES:
            return
        remaining_analysis_seconds = (
            self.max_analysis_seconds_per_run - self._analysis_seconds.get()
        )
        if remaining_analysis_seconds <= 0:
            return

        self._attempt_transport_failed.set(False)
        self._analysis_status.set(_ANALYSIS_UNKNOWN)
        attempt_started = time.monotonic()
        attempt_timeout = min(self.timeout, remaining_analysis_seconds)
        try:
            async with asyncio.timeout(attempt_timeout):
                result = await self._detect_and_solve(screenshot_b64)
        except TimeoutError:
            self._attempt_transport_failed.set(True)
            logger.warning(
                "CAPTCHA helper: screenshot analysis exceeded %.3f seconds",
                attempt_timeout,
            )
            result = None
        finally:
            attempt_seconds = time.monotonic() - attempt_started
            self._analysis_seconds.set(self._analysis_seconds.get() + attempt_seconds)
            self._update_run_usage(analysis_seconds=attempt_seconds)
        self._last_attempt_time.set(time.monotonic())
        analysis_status = _ANALYSIS_HINT if result is not None else self._analysis_status.get()
        self._analysis_status.set(analysis_status)
        self._update_run_usage(
            **{
                {
                    _ANALYSIS_HINT: "analysis_hint_candidates",
                    _ANALYSIS_NOT_DETECTED: "analysis_negative_results",
                    _ANALYSIS_DETECTED_UNSOLVED: "analysis_positive_without_hint",
                    _ANALYSIS_UNKNOWN: "analysis_inconclusive_results",
                }[analysis_status]: 1
            }
        )
        if self._attempt_transport_failed.get():
            failures = self._consecutive_transport_failures.get() + 1
            self._consecutive_transport_failures.set(failures)
            if failures == _MAX_CONSECUTIVE_TRANSPORT_FAILURES:
                logger.warning(
                    "CAPTCHA helper disabled for the current run after %d consecutive request failures",
                    failures,
                )
        else:
            self._consecutive_transport_failures.set(0)
        # A valid negative-shaped model response is safe to deduplicate now. A hint is not marked
        # seen until it is bound to and injected with the matching screenshot;
        # otherwise a binding mismatch must leave the same pixels retryable.
        if analysis_status == _ANALYSIS_NOT_DETECTED:
            self._seen_screenshot_digests.set(seen | {digest})
        pending_result = result
        if pending_result is None and analysis_status == _ANALYSIS_DETECTED_UNSOLVED:
            pending_result = {"type": "detected_unsolved"}
        if (
            pending_result
            and self._screenshot_sequence.get() == sequence
            and self._latest_screenshot_digest.get() == digest
        ):
            self._pending_message.set(
                _PendingMessage(
                    result=pending_result,
                    screenshot_digest=digest,
                    screenshot_sequence=sequence,
                )
            )
            logger.info(
                "CAPTCHA helper produced an unverified hint: type=%s",
                pending_result.get("type", "unknown"),
            )
        elif analysis_status == _ANALYSIS_UNKNOWN:
            logger.debug(
                "CAPTCHA helper produced no conclusive observation; screenshot remains eligible for retry"
            )

    async def on_computer_call_end(
        self, item: Dict[str, Any], result: List[Dict[str, Any]]
    ) -> None:
        """Bind a pending message to the exact screenshot-producing call.

        A digest alone cannot distinguish identical pixels captured by two
        different browser actions or tabs. Computer call ids already pair an
        action with its output, so retain that association when the matching
        screenshot result becomes available.
        """

        pending = self._pending_message.get()
        if pending is None:
            return

        item_call_id = item.get("call_id")
        if pending.call_id is not None:
            if item_call_id != pending.call_id:
                self._update_run_usage(hints_discarded=1)
                self._pending_message.set(None)
            return
        if not isinstance(item_call_id, str) or not item_call_id:
            self._update_run_usage(hints_discarded=1)
            self._pending_message.set(None)
            return

        for message in reversed(result):
            if message.get("type") != "computer_call_output":
                continue
            output = message.get("output")
            image_url = output.get("image_url") if isinstance(output, dict) else None
            if not isinstance(image_url, str):
                continue
            if self._image_digest(image_url) != pending.screenshot_digest:
                continue
            if message.get("call_id") != item_call_id:
                self._update_run_usage(hints_discarded=1)
                self._pending_message.set(None)
                return
            self._pending_message.set(
                _PendingMessage(
                    result=pending.result,
                    screenshot_digest=pending.screenshot_digest,
                    screenshot_sequence=pending.screenshot_sequence,
                    call_id=item_call_id,
                )
            )
            return

        self._update_run_usage(hints_discarded=1)
        self._pending_message.set(None)

    async def on_computer_call_start(self, item: Dict[str, Any]) -> None:
        """Invalidate an older hint before any later computer action."""

        if self._pending_message.get() is not None:
            self._update_run_usage(hints_discarded=1)
            self._pending_message.set(None)

    async def on_function_call_start(self, item: Dict[str, Any]) -> None:
        """A function call can change page state without producing an image."""

        if self._pending_message.get() is not None:
            self._update_run_usage(hints_discarded=1)
            self._pending_message.set(None)

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        pending = self._pending_message.get()
        if pending is None:
            return messages

        self._pending_message.set(None)
        if pending.screenshot_sequence != self._screenshot_sequence.get():
            self._update_run_usage(hints_discarded=1)
            return messages
        latest_digest, latest_call_id = self._latest_message_image_identity(messages)
        if (
            pending.call_id is None
            or latest_digest != pending.screenshot_digest
            or latest_call_id != pending.call_id
        ):
            self._update_run_usage(hints_discarded=1)
            logger.debug("Discarded CAPTCHA hint because the visible screenshot changed")
            return messages

        answer_info = pending.result
        captcha_type = answer_info.get("type", "text")
        answer = answer_info.get("answer", "")

        if captcha_type == "checkbox":
            hint = _CHECKBOX_INJECT
        elif captcha_type == "image_select":
            hint = _IMAGE_SELECT_INJECT
        elif captcha_type == "detected_unsolved":
            hint = _UNSOLVED_INJECT
        elif answer:
            hint = _INJECT_TEMPLATE.format(answer=answer)
        else:
            self._update_run_usage(hints_discarded=1)
            return messages

        self._seen_screenshot_digests.set(
            self._seen_screenshot_digests.get() | {pending.screenshot_digest}
        )
        self._update_run_usage(messages_injected=1)
        logger.info("Injecting an unverified CAPTCHA helper message into agent messages")
        return messages + [{"role": "user", "content": hint}]

    # ── internal ─────────────────────────────────────────────────

    @staticmethod
    def _empty_run_usage() -> Dict[str, Union[int, float]]:
        return {
            "requests": 0,
            "responses": 0,
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
            "provider_reported_cost": 0.0,
            "token_usage_complete_responses": 0,
            "token_usage_partial_responses": 0,
            "token_usage_missing_responses": 0,
            "cost_reported_responses": 0,
            "cost_missing_responses": 0,
            "analysis_seconds": 0.0,
            "analysis_hint_candidates": 0,
            "analysis_negative_results": 0,
            "analysis_positive_without_hint": 0,
            "analysis_inconclusive_results": 0,
            "messages_injected": 0,
            "hints_discarded": 0,
        }

    def _update_run_usage(self, **updates: Union[int, float]) -> None:
        usage = dict(self._run_usage.get() or self._empty_run_usage())
        for key, value in updates.items():
            usage[key] = usage.get(key, 0) + value
        self._run_usage.set(usage)

    def _record_response_usage(self, data: Any) -> None:
        response_usage = data.get("usage") if isinstance(data, dict) else None
        numeric_usage: Dict[str, Union[int, float]] = {}
        if isinstance(response_usage, dict):
            for source, destination in [
                ("prompt_tokens", "prompt_tokens"),
                ("input_tokens", "prompt_tokens"),
                ("completion_tokens", "completion_tokens"),
                ("output_tokens", "completion_tokens"),
                ("total_tokens", "total_tokens"),
                ("response_cost", "provider_reported_cost"),
                ("cost", "provider_reported_cost"),
            ]:
                value = response_usage.get(source)
                if destination == "provider_reported_cost":
                    valid = (
                        isinstance(value, (int, float))
                        and not isinstance(value, bool)
                        and math.isfinite(value)
                        and value >= 0
                    )
                else:
                    valid = isinstance(value, int) and not isinstance(value, bool) and value >= 0
                if valid and destination not in numeric_usage:
                    numeric_usage[destination] = value
        token_fields = {"prompt_tokens", "completion_tokens", "total_tokens"}
        reported_token_fields = token_fields.intersection(numeric_usage)
        if reported_token_fields == token_fields:
            token_usage_status = "complete"
        elif reported_token_fields:
            token_usage_status = "partial"
        else:
            token_usage_status = "missing"
        cost_reported = "provider_reported_cost" in numeric_usage
        self._update_run_usage(
            **numeric_usage,
            **{
                f"token_usage_{token_usage_status}_responses": 1,
                "cost_reported_responses" if cost_reported else "cost_missing_responses": 1,
            },
        )

    def _finish_analysis(
        self,
        status: str,
        result: Optional[Dict[str, Any]] = None,
    ) -> Optional[Dict[str, Any]]:
        self._analysis_status.set(status)
        return result

    @staticmethod
    def _validate_api_base(api_base: str) -> str:
        normalized = api_base.rstrip("/")
        parsed = urlparse(normalized)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("api_base must be an absolute HTTP(S) URL")
        if parsed.username is not None or parsed.password is not None:
            raise ValueError("api_base must not contain embedded credentials")
        if "?" in normalized or "#" in normalized:
            raise ValueError("api_base must not contain a query or fragment")
        try:
            _ = parsed.port
        except ValueError as exc:
            raise ValueError("api_base must contain a valid port") from exc
        try:
            is_loopback = ipaddress.ip_address(parsed.hostname).is_loopback
        except ValueError:
            is_loopback = False
        if not is_loopback:
            raise ValueError(
                "api_base must use a numeric loopback address because it receives full screenshots"
            )
        return normalized

    @staticmethod
    def _image_digest(image: Union[str, bytes]) -> str:
        if isinstance(image, bytes):
            payload = image
        else:
            encoded = (
                image.split(",", 1)[1] if image.startswith("data:") and "," in image else image
            )
            try:
                payload = base64.b64decode(encoded, validate=True)
            except (binascii.Error, ValueError):
                payload = encoded.encode("utf-8")
        return hashlib.sha256(payload).hexdigest()

    @classmethod
    def _latest_message_image_identity(
        cls, messages: List[Dict[str, Any]]
    ) -> tuple[Optional[str], Optional[str]]:
        for message in reversed(messages):
            candidates: List[Any] = []
            output = message.get("output")
            if isinstance(output, dict):
                candidates.append(output.get("image_url"))
            content = message.get("content")
            if isinstance(content, list):
                for item in reversed(content):
                    if isinstance(item, dict):
                        candidates.append(item.get("image_url"))
            candidates.append(message.get("image_url"))
            for candidate in candidates:
                if isinstance(candidate, dict):
                    candidate = candidate.get("url")
                if isinstance(candidate, str) and candidate:
                    call_id = message.get("call_id")
                    return cls._image_digest(candidate), (
                        call_id if isinstance(call_id, str) else None
                    )
            if message.get("type") in {"computer_call_output", "function_call_output"}:
                return None, None
        return None, None

    async def _budgeted_call_vision(self, image_b64: str, prompt: str) -> Optional[str]:
        requests = self._request_count.get()
        if requests >= self.max_requests_per_run:
            logger.debug("CAPTCHA helper request budget exhausted")
            return None
        self._request_count.set(requests + 1)
        self._update_run_usage(requests=1)
        return await self._call_vision(image_b64, prompt)

    async def _detect_and_solve(self, image_b64: str) -> Optional[Dict[str, Any]]:
        """Try single-shot JSON, fall back to YES/NO + solve."""

        self._analysis_status.set(_ANALYSIS_UNKNOWN)
        json_reply = await self._budgeted_call_vision(image_b64, _JSON_PROMPT)
        if json_reply is None and self._attempt_transport_failed.get():
            # One unavailable endpoint should consume one timeout, not cascade
            # into the fallback detector and solver timeouts. A later
            # screenshot hook may retry within the per-run request budget.
            return self._finish_analysis(_ANALYSIS_UNKNOWN)
        if json_reply:
            parsed = self._parse_json_response(json_reply)
            if parsed:
                if not parsed.get("captcha"):
                    return self._finish_analysis(_ANALYSIS_NOT_DETECTED)
                # Preserve positive evidence if the bounded solve phase times
                # out. The screenshot remains retryable, but it is not counted
                # as an entirely unknown analysis.
                self._analysis_status.set(_ANALYSIS_DETECTED_UNSOLVED)
                answer = parsed.get("answer", "")
                ctype = parsed.get("type", "")
                if not isinstance(ctype, str):
                    ctype = ""
                normalized_type = self._normalize_type_hint(ctype) or ""
                if not isinstance(answer, str):
                    return await self._solve_phase(image_b64, normalized_type)
                if answer and answer != "<the text to enter>":
                    classified = self._classify_answer(answer, normalized_type)
                    if classified is not None and (
                        not normalized_type or classified["type"] == normalized_type
                    ):
                        return self._finish_analysis(_ANALYSIS_HINT, classified)
                    if classified is not None:
                        logger.debug("Discarded CAPTCHA answer that contradicted the detected type")
                        return self._finish_analysis(_ANALYSIS_DETECTED_UNSOLVED)
                return await self._solve_phase(image_b64, normalized_type)

        return await self._fallback_detect_solve(image_b64)

    async def _fallback_detect_solve(self, image_b64: str) -> Optional[Dict[str, Any]]:
        detect_reply = await self._budgeted_call_vision(image_b64, _DETECT_PROMPT)
        if not detect_reply:
            return self._finish_analysis(_ANALYSIS_UNKNOWN)
        decision = detect_reply.strip().upper()
        if decision == "NO":
            return self._finish_analysis(_ANALYSIS_NOT_DETECTED)
        if decision != "YES":
            return self._finish_analysis(_ANALYSIS_UNKNOWN)
        self._analysis_status.set(_ANALYSIS_DETECTED_UNSOLVED)
        logger.info("CAPTCHA helper found a possible challenge; requesting a bounded hint")
        return await self._solve_phase(image_b64)

    async def _solve_phase(
        self, image_b64: str, expected_type: str = ""
    ) -> Optional[Dict[str, Any]]:
        solve_reply = await self._budgeted_call_vision(image_b64, _SOLVE_PROMPT)
        if not solve_reply:
            return self._finish_analysis(_ANALYSIS_DETECTED_UNSOLVED)
        classified = self._classify_answer(solve_reply.strip())
        normalized_expected = self._normalize_type_hint(expected_type)
        if (
            classified is not None
            and normalized_expected is not None
            and classified["type"] != normalized_expected
        ):
            logger.debug("Discarded CAPTCHA answer that contradicted the detected type")
            return self._finish_analysis(_ANALYSIS_DETECTED_UNSOLVED)
        if classified is None:
            return self._finish_analysis(_ANALYSIS_DETECTED_UNSOLVED)
        return self._finish_analysis(_ANALYSIS_HINT, classified)

    @staticmethod
    def _normalize_type_hint(type_hint: str) -> Optional[str]:
        normalized = re.sub(r"[^a-z0-9]+", "_", type_hint.strip().lower()).strip("_")
        # Provider names such as reCAPTCHA are deliberately left ambiguous:
        # the same provider can show a checkbox, text, or image-grid step.
        if "checkbox" in normalized:
            return "checkbox"
        if any(marker in normalized for marker in ("image_select", "image_grid", "image")):
            return "image_select"
        if any(marker in normalized for marker in ("distorted", "text")):
            return "text"
        return None

    _FALSE_POSITIVE_WORDS = {
        "allow",
        "don't",
        "block",
        "dismiss",
        "cancel",
        "submit",
        "close",
        "sign",
        "log",
        "accept",
        "reject",
        "continue",
        "verify",
        "human",
        "you",
        "are",
        "your",
        "the",
        "is",
        "a",
        "an",
        "to",
        "for",
        "of",
        "it",
        "in",
        "on",
        "not",
        "this",
        "that",
        "with",
        "from",
        "or",
        "and",
        "ok",
        "got",
        "now",
        "use",
        "location",
        "precise",
        "spam",
        "enter",
        "text",
        "image",
        "valid",
        "example",
        "answer",
        "captcha",
        "please",
        "click",
        "here",
        "below",
        "above",
    }
    _NON_ANSWER_TOKENS = {
        "ERROR",
        "FALSE",
        "N/A",
        "NO",
        "NONE",
        "NULL",
        "TRUE",
        "UNKNOWN",
        "UNREADABLE",
        "YES",
    }

    @classmethod
    def _classify_answer(cls, raw: str, type_hint: str = "") -> Optional[Dict[str, Any]]:
        upper = raw.strip().upper()

        if upper in cls._NON_ANSWER_TOKENS:
            return None

        if upper in {
            "CLICK_CHECKBOX",
            "CLICK CHECKBOX",
            "I AM NOT A ROBOT",
            "I'M NOT A ROBOT",
            "I AM HUMAN",
            "VERIFY YOU ARE HUMAN",
        }:
            return {"type": "checkbox", "answer": "click_checkbox"}

        if type_hint == "image_select" or upper in {
            "IMAGE_SELECT",
            "IMAGE SELECT",
            "SELECT_IMAGES",
            "SELECT IMAGES",
        }:
            return {"type": "image_select", "answer": "select_images"}

        if type_hint == "checkbox":
            return None

        cleaned = re.sub(r"^[`\"']+|[`\"']+$", "", raw).strip()

        if not cleaned or cleaned.upper() in cls._NON_ANSWER_TOKENS:
            return None

        if cls._looks_like_page_text(cleaned):
            logger.debug("Filtered probable page text from CAPTCHA model")
            return None

        # Never promote arbitrary model/page prose into an instruction for the
        # primary agent. Text CAPTCHA answers are short ASCII strings; anything
        # else is discarded and may be retried from a later screenshot.
        if not re.fullmatch(r"[A-Za-z0-9]{1,16}", cleaned):
            logger.debug("Filtered unsafe CAPTCHA answer")
            return None

        return {"type": "text", "answer": cleaned}

    @classmethod
    def _looks_like_page_text(cls, text: str) -> bool:
        """Real CAPTCHA answers are short random alphanumeric strings.
        English phrases with common words are page text, not answers."""
        words = text.lower().split()
        if len(words) >= 3:
            common = sum(1 for w in words if w in cls._FALSE_POSITIVE_WORDS)
            if common >= 2:
                return True
        if len(words) == 1 and words[0] in cls._FALSE_POSITIVE_WORDS:
            return True
        if len(words) == 2 and all(w in cls._FALSE_POSITIVE_WORDS for w in words):
            return True
        return False

    @staticmethod
    def _parse_json_response(text: str) -> Optional[Dict[str, Any]]:
        try:
            obj = json.loads(text)
            if isinstance(obj, dict) and isinstance(obj.get("captcha"), bool):
                return obj
        except json.JSONDecodeError:
            pass

        for pattern in [
            r"```(?:json)?\s*(\{[^}]+\})\s*```",
            r"(\{[^}]*\"captcha\"[^}]*\})",
        ]:
            match = re.search(pattern, text, re.DOTALL)
            if match:
                try:
                    obj = json.loads(match.group(1))
                    if isinstance(obj, dict) and isinstance(obj.get("captcha"), bool):
                        return obj
                except json.JSONDecodeError:
                    continue
        return None

    async def _call_vision(self, image_b64: str, prompt: str) -> Optional[str]:
        payload = {
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": f"data:image/png;base64,{image_b64}",
                            },
                        },
                        {"type": "text", "text": prompt},
                    ],
                }
            ],
            "max_tokens": self.max_tokens,
            "temperature": 0.0,
        }

        url = f"{self.api_base}/chat/completions"
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        try:
            async with asyncio.timeout(self.timeout):
                timeout = httpx.Timeout(self.timeout)
                # Screenshot routing is controlled solely by api_base. Never let
                # ambient HTTP(S)_PROXY variables redirect full-screen content.
                async with httpx.AsyncClient(
                    timeout=timeout,
                    trust_env=False,
                    follow_redirects=False,
                ) as client:
                    async with client.stream(
                        "POST", url, json=payload, headers=headers
                    ) as response:
                        response.raise_for_status()
                        response_bytes = bytearray()
                        async for chunk in response.aiter_bytes():
                            response_bytes.extend(chunk)
                            if len(response_bytes) > _MAX_RESPONSE_BYTES:
                                logger.warning("CAPTCHA helper: response exceeded size limit")
                                return None
            data = json.loads(response_bytes.decode("utf-8"))
        except (
            TimeoutError,
            httpx.HTTPError,
            UnicodeDecodeError,
            ValueError,
            json.JSONDecodeError,
        ) as exc:
            self._attempt_transport_failed.set(True)
            logger.warning("CAPTCHA helper: request failed: %s", exc)
            return None

        self._update_run_usage(responses=1)
        self._record_response_usage(data)

        try:
            content = data["choices"][0]["message"]["content"]
        except (KeyError, IndexError, TypeError):
            logger.warning("CAPTCHA helper: response did not contain message content")
            return None

        if not isinstance(content, str):
            logger.warning("CAPTCHA helper: message content was not text")
            return None
        return content.strip()
