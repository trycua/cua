"""
CAPTCHA solver callback that routes screenshots to a local vision model.

When the local model detects a CAPTCHA in a screenshot, it reads the
challenge and injects a bounded instruction into the agent's message
stream so the main model can act on it. The callback never pauses for a
human: unreadable or unsafe model output is discarded and a later
screenshot can retry.

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

import asyncio
import base64
import http.client
import json
import logging
import re
import time
import urllib.error
import urllib.request
from typing import Any, Dict, List, Optional, Union

from .base import AsyncCallbackHandler

logger = logging.getLogger(__name__)

_MAX_RESPONSE_BYTES = 1024 * 1024

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
        cooldown: Seconds to wait after any detected result before
            checking again (avoids reprocessing the same CAPTCHA).
    """

    def __init__(
        self,
        api_base: str = "http://127.0.0.1:1234/v1",
        model: str = "bionic-vision-light",
        max_tokens: int = 200,
        timeout: float = 30.0,
        cooldown: float = 5.0,
    ) -> None:
        self.api_base = api_base.rstrip("/")
        self.model = model
        self.max_tokens = max_tokens
        self.timeout = timeout
        self.cooldown = cooldown

        self._pending_answer: Optional[Dict[str, Any]] = None
        self._last_solve_time = 0.0

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        # A result is meaningful only for the run that produced its screenshot.
        # Do not carry a pending answer or cooldown into an unrelated run.
        self._pending_answer = None
        self._last_solve_time = 0.0

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        self._pending_answer = None

    async def on_screenshot(self, screenshot: Union[str, bytes], name: str = "screenshot") -> None:
        if time.monotonic() - self._last_solve_time < self.cooldown:
            return

        if isinstance(screenshot, bytes):
            screenshot_b64 = base64.b64encode(screenshot).decode("ascii")
        else:
            screenshot_b64 = screenshot

        loop = asyncio.get_running_loop()
        result = await loop.run_in_executor(None, self._detect_and_solve, screenshot_b64)
        if result:
            self._pending_answer = result
            self._last_solve_time = time.monotonic()
            logger.info(
                "CAPTCHA result: type=%s",
                result.get("type", "unknown"),
            )

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        answer_info = self._pending_answer
        if not answer_info:
            return messages

        self._pending_answer = None

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

    def _detect_and_solve(self, image_b64: str) -> Optional[Dict[str, Any]]:
        """Try single-shot JSON, fall back to YES/NO + solve."""

        json_reply = self._call_vision(image_b64, _JSON_PROMPT)
        if json_reply:
            parsed = self._parse_json_response(json_reply)
            if parsed:
                if not parsed.get("captcha"):
                    return self._fallback_detect_solve(image_b64)
                answer = parsed.get("answer", "")
                ctype = parsed.get("type", "")
                if not isinstance(answer, str):
                    return self._solve_phase(image_b64)
                if not isinstance(ctype, str):
                    ctype = ""
                if answer and answer != "<the text to enter>":
                    classified = self._classify_answer(answer, ctype)
                    if classified is not None:
                        return classified
                    return None
                return self._solve_phase(image_b64)

        return self._fallback_detect_solve(image_b64)

    def _fallback_detect_solve(self, image_b64: str) -> Optional[Dict[str, Any]]:
        detect_reply = self._call_vision(image_b64, _DETECT_PROMPT)
        if not detect_reply or detect_reply.strip().upper() != "YES":
            return None
        logger.info("CAPTCHA detected (fallback), asking model to solve...")
        return self._solve_phase(image_b64)

    def _solve_phase(self, image_b64: str) -> Optional[Dict[str, Any]]:
        solve_reply = self._call_vision(image_b64, _SOLVE_PROMPT)
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

        if upper in {"IMAGE_SELECT", "IMAGE SELECT", "SELECT IMAGES"}:
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

    def _call_vision(self, image_b64: str, prompt: str) -> Optional[str]:
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
        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )

        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as response:
                response_bytes = response.read(_MAX_RESPONSE_BYTES + 1)
                if len(response_bytes) > _MAX_RESPONSE_BYTES:
                    logger.warning("CAPTCHA solver: response exceeded size limit")
                    return None
                data = json.loads(response_bytes.decode("utf-8"))
        except (
            urllib.error.URLError,
            http.client.HTTPException,
            OSError,
            TimeoutError,
            UnicodeDecodeError,
            ValueError,
            json.JSONDecodeError,
        ) as exc:
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
