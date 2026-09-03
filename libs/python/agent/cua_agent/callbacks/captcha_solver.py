"""
CAPTCHA solver callback that routes screenshots to a local vision model.

When the local model detects a CAPTCHA in a screenshot, it reads the
challenge and injects a bounded instruction into the agent's message
stream so the main model can act on it. The callback never pauses for a
human: unreadable or unsafe model output is discarded and a later
screenshot can retry.

The injected instruction is an ordinary user message. Logging and trajectory
callbacks later in the chain may therefore record the bounded answer or action.
Detection runs when an agent loop emits a screenshot callback and can affect
the next model call that carries those exact image bytes; it does not interrupt
an already-running model call.

Usage:

    from cua_agent.callbacks import CaptchaSolverCallback

    solver = CaptchaSolverCallback(
        api_base="http://127.0.0.1:1234/v1",
        model="bionic-vision-light",
    )
    agent = ComputerAgent(
        model="claude-sonnet-4-5-20250929",
        callbacks=[solver],
    )

"""

from __future__ import annotations

import base64
import binascii
import contextvars
import hashlib
import ipaddress
import json
import logging
import re
import time
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Union
from urllib.parse import urlparse

import httpx

from .base import AsyncCallbackHandler

logger = logging.getLogger(__name__)

_MAX_RESPONSE_BYTES = 1024 * 1024
_SCREENSHOT_NAMES = {"screenshot", "screenshot_before", "screenshot_after"}


@dataclass(frozen=True)
class _PendingAnswer:
    result: Dict[str, Any]
    screenshot_digest: str
    screenshot_sequence: int


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
    "CAPTCHA DETECTED — the local vision model read the challenge and "
    'determined the answer is: "{answer}". '
    "Type this answer into the CAPTCHA input field to proceed."
)

_CHECKBOX_INJECT = (
    "CAPTCHA DETECTED — the local vision model sees a reCAPTCHA "
    "'I am not a robot' checkbox. Click the checkbox to proceed."
)

_IMAGE_SELECT_INJECT = (
    "CAPTCHA DETECTED — the local vision model found an image-selection "
    "challenge. Inspect the visible challenge, select the requested images "
    "with the computer controls, and continue. Do not ask the user to solve it."
)


class CaptchaSolverCallback(AsyncCallbackHandler):
    """
    Intercepts agent screenshots, sends them to a local vision model
    to detect and solve CAPTCHAs, and injects the answer into the
    message stream for the main model to act on.

    Strategy (optimized for small 2B vision models):
    1. Try a single-shot JSON prompt — fast path for text CAPTCHAs.
    2. If JSON comes back with captcha=true but no answer, ask a
       targeted solve-only follow-up.
    3. If JSON fails or says no captcha, try a simple YES/NO detection
       prompt that catches reCAPTCHA checkboxes on busy pages, then
       solve if positive.
    4. For image-selection challenges, return control to the main model with
       an automated computer-use instruction. Discard unreadable or unsafe
       output instead of forwarding it or requiring a user.

    Args:
        api_base: LM Studio / OpenAI-compatible API base URL.
        model: Vision model ID served by the local API.
        max_tokens: Max tokens for the vision model response.
        timeout: HTTP request timeout in seconds.
        cooldown: Minimum seconds between screenshot scans in one run. The
            default is zero; digest deduplication and the request cap bound
            repeated work without skipping a newly observed challenge.
        max_requests_per_run: Hard cap on vision HTTP requests in one run.
        api_key: Optional bearer token for the vision endpoint.
        allow_remote: Permit a non-loopback HTTPS endpoint. Remote endpoints
            also require api_key because they receive the full screenshot.
    """

    def __init__(
        self,
        api_base: str = "http://127.0.0.1:1234/v1",
        model: str = "bionic-vision-light",
        max_tokens: int = 200,
        timeout: float = 30.0,
        cooldown: float = 0.0,
        max_requests_per_run: int = 12,
        api_key: Optional[str] = None,
        allow_remote: bool = False,
    ) -> None:
        self.api_base = self._validate_api_base(api_base, allow_remote, api_key)
        self.model = model
        self.max_tokens = max_tokens
        self.timeout = timeout
        self.cooldown = cooldown
        self.max_requests_per_run = max_requests_per_run
        self.api_key = api_key

        if max_tokens <= 0:
            raise ValueError("max_tokens must be positive")
        if timeout <= 0:
            raise ValueError("timeout must be positive")
        if cooldown < 0:
            raise ValueError("cooldown cannot be negative")
        if max_requests_per_run <= 0:
            raise ValueError("max_requests_per_run must be positive")

        suffix = id(self)
        self._pending_answer = contextvars.ContextVar[Optional[_PendingAnswer]](
            f"captcha_pending_answer_{suffix}", default=None
        )
        self._last_attempt_time = contextvars.ContextVar[float](
            f"captcha_last_attempt_time_{suffix}", default=0.0
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

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        # A result is meaningful only for the run that produced its screenshot.
        # Do not carry a pending answer or cooldown into an unrelated run.
        self._pending_answer.set(None)
        self._last_attempt_time.set(0.0)
        self._request_count.set(0)
        self._screenshot_sequence.set(0)
        self._latest_screenshot_digest.set(None)
        self._seen_screenshot_digests.set(frozenset())
        self._attempt_transport_failed.set(False)

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        self._pending_answer.set(None)
        self._last_attempt_time.set(0.0)
        self._request_count.set(0)
        self._screenshot_sequence.set(0)
        self._latest_screenshot_digest.set(None)
        self._seen_screenshot_digests.set(frozenset())
        self._attempt_transport_failed.set(False)

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
            self._pending_answer.set(None)

        seen = self._seen_screenshot_digests.get()
        if digest in seen:
            return

        now = time.monotonic()
        if now - self._last_attempt_time.get() < self.cooldown:
            return
        if self._request_count.get() >= self.max_requests_per_run:
            return

        self._last_attempt_time.set(now)
        self._attempt_transport_failed.set(False)
        result = await self._detect_and_solve(screenshot_b64)
        # A completed negative is safe to deduplicate. A transport failure is
        # not a model decision, so allow the same still-visible challenge to
        # retry on a later hook while the per-run request cap stays decisive.
        if result is not None or not self._attempt_transport_failed.get():
            self._seen_screenshot_digests.set(seen | {digest})
        if (
            result
            and self._screenshot_sequence.get() == sequence
            and self._latest_screenshot_digest.get() == digest
        ):
            self._pending_answer.set(
                _PendingAnswer(
                    result=result,
                    screenshot_digest=digest,
                    screenshot_sequence=sequence,
                )
            )
            logger.info(
                "CAPTCHA result: type=%s",
                result.get("type", "unknown"),
            )

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        pending = self._pending_answer.get()
        if pending is None:
            return messages

        self._pending_answer.set(None)
        if pending.screenshot_sequence != self._screenshot_sequence.get():
            return messages
        if self._latest_message_image_digest(messages) != pending.screenshot_digest:
            logger.debug("Discarded CAPTCHA result because the visible screenshot changed")
            return messages

        answer_info = pending.result
        captcha_type = answer_info.get("type", "text")
        answer = answer_info.get("answer", "")

        if captcha_type == "checkbox":
            hint = _CHECKBOX_INJECT
        elif captcha_type == "image_select":
            hint = _IMAGE_SELECT_INJECT
        elif answer:
            hint = _INJECT_TEMPLATE.format(answer=answer)
        else:
            return messages

        logger.info("Injecting CAPTCHA hint into agent messages")
        return messages + [{"role": "user", "content": hint}]

    # ── internal ─────────────────────────────────────────────────

    @staticmethod
    def _validate_api_base(api_base: str, allow_remote: bool, api_key: Optional[str]) -> str:
        normalized = api_base.rstrip("/")
        parsed = urlparse(normalized)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("api_base must be an absolute HTTP(S) URL")
        try:
            is_loopback = ipaddress.ip_address(parsed.hostname).is_loopback
        except ValueError:
            is_loopback = parsed.hostname.lower() == "localhost"
        if not is_loopback:
            if not allow_remote:
                raise ValueError(
                    "remote CAPTCHA endpoints require allow_remote=True because they receive the full screenshot"
                )
            if parsed.scheme != "https":
                raise ValueError("remote CAPTCHA endpoints must use HTTPS")
            if not api_key:
                raise ValueError("remote CAPTCHA endpoints require api_key")
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
    def _latest_message_image_digest(cls, messages: List[Dict[str, Any]]) -> Optional[str]:
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
                    return cls._image_digest(candidate)
        return None

    async def _budgeted_call_vision(self, image_b64: str, prompt: str) -> Optional[str]:
        requests = self._request_count.get()
        if requests >= self.max_requests_per_run:
            logger.debug("CAPTCHA solver request budget exhausted")
            return None
        self._request_count.set(requests + 1)
        return await self._call_vision(image_b64, prompt)

    async def _detect_and_solve(self, image_b64: str) -> Optional[Dict[str, Any]]:
        """Try single-shot JSON, fall back to YES/NO + solve."""

        json_reply = await self._budgeted_call_vision(image_b64, _JSON_PROMPT)
        if json_reply is None and self._attempt_transport_failed.get():
            # One unavailable endpoint should consume one timeout, not cascade
            # into the fallback detector and solver timeouts. A later
            # screenshot hook may retry within the per-run request budget.
            return None
        if json_reply:
            parsed = self._parse_json_response(json_reply)
            if parsed:
                if not parsed.get("captcha"):
                    return await self._fallback_detect_solve(image_b64)
                answer = parsed.get("answer", "")
                ctype = parsed.get("type", "")
                if not isinstance(answer, str):
                    return await self._solve_phase(image_b64)
                if not isinstance(ctype, str):
                    ctype = ""
                if answer and answer != "<the text to enter>":
                    classified = self._classify_answer(answer, ctype)
                    if classified is not None:
                        return classified
                    return None
                return await self._solve_phase(image_b64)

        return await self._fallback_detect_solve(image_b64)

    async def _fallback_detect_solve(self, image_b64: str) -> Optional[Dict[str, Any]]:
        detect_reply = await self._budgeted_call_vision(image_b64, _DETECT_PROMPT)
        if not detect_reply or detect_reply.strip().upper() != "YES":
            return None
        logger.info("CAPTCHA detected (fallback), asking model to solve...")
        return await self._solve_phase(image_b64)

    async def _solve_phase(self, image_b64: str) -> Optional[Dict[str, Any]]:
        solve_reply = await self._budgeted_call_vision(image_b64, _SOLVE_PROMPT)
        if not solve_reply:
            return None
        return self._classify_answer(solve_reply.strip()) or None

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

    @classmethod
    def _classify_answer(cls, raw: str, type_hint: str = "") -> Optional[Dict[str, Any]]:
        upper = raw.strip().upper()

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

        if not cleaned:
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
            timeout = httpx.Timeout(self.timeout)
            # Screenshot routing is controlled solely by api_base. Never let
            # ambient HTTP(S)_PROXY variables redirect full-screen content.
            async with httpx.AsyncClient(timeout=timeout, trust_env=False) as client:
                async with client.stream("POST", url, json=payload, headers=headers) as response:
                    response.raise_for_status()
                    response_bytes = bytearray()
                    async for chunk in response.aiter_bytes():
                        response_bytes.extend(chunk)
                        if len(response_bytes) > _MAX_RESPONSE_BYTES:
                            logger.warning("CAPTCHA solver: response exceeded size limit")
                            return None
            data = json.loads(response_bytes.decode("utf-8"))
        except (
            httpx.HTTPError,
            UnicodeDecodeError,
            ValueError,
            json.JSONDecodeError,
        ) as exc:
            self._attempt_transport_failed.set(True)
            logger.warning("CAPTCHA solver: request failed: %s", exc)
            return None

        try:
            content = data["choices"][0]["message"]["content"]
        except (KeyError, IndexError, TypeError):
            logger.warning("CAPTCHA solver: response did not contain message content")
            return None

        if not isinstance(content, str):
            logger.warning("CAPTCHA solver: message content was not text")
            return None
        return content.strip()
