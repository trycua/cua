"""Browser benchmarks on the ``bench-web`` image.

``bench-web`` runs Chromium in the desktop session with ``bench-web-ctl``
on :7000, a small HTTP API over the browser's DevTools (``POST /reset
{url}``, ``POST /navigate``, ``POST /eval {expression}``, ``GET /state``,
``GET /healthz``). Adapters talk to it through ``ep.request("server", ...)``,
plain HTTP that works the same locally and through the Fleet gateway. The
agent acts on the visible browser through cua-driver like on any desktop.

Answers: an agent that produces a final text answer sets
``session.final_answer = "..."`` (and may keep ``session.action_history``,
a list of strings); live-web judges read both.
"""

from __future__ import annotations

import json
from typing import Any, Optional

from cua_bench import images

from .base import BenchAdapter, ServerSpec

CTL_PORT = 7000
CDP_PORT = 9222
MINIWOB_PORT = 7560


class WebCtlError(RuntimeError):
    pass


async def ctl(ep: Any, method: str, path: str, body: Optional[dict] = None,
              timeout: float = 90.0) -> dict:
    """One bench-web-ctl call; the JSON answer, or :class:`WebCtlError`."""
    kwargs: dict = {"timeout": timeout}
    if body is not None:
        kwargs["json"] = body
    r = await ep.request("server", method, path, **kwargs)
    status = getattr(r, "status_code", 200)
    try:
        data = r.json()
    except Exception:  # noqa: BLE001 - non-JSON error page
        data = {"error": getattr(r, "text", "")}
    if status != 200:
        raise WebCtlError(f"bench-web-ctl {method} {path}: {status} {data.get('error', data)}")
    return data


async def reset(ep: Any, url: str = "about:blank") -> dict:
    return await ctl(ep, "POST", "/reset", {"url": url})


async def evaluate_js(ep: Any, expression: str, *, await_promise: bool = False) -> Any:
    return (await ctl(ep, "POST", "/eval", {"expression": expression, "await": await_promise}))["value"]


def agent_answer(session: Any) -> str:
    return str(getattr(session, "final_answer", "") or "")


def agent_actions(session: Any) -> list[str]:
    history = getattr(session, "action_history", None) or []
    return [a if isinstance(a, str) else json.dumps(a, default=str) for a in history]


class BenchWebAdapter(BenchAdapter):
    """Base for adapters on ``bench-web`` (image, server, ports)."""

    kinds = ("container", "vm")
    server = ServerSpec(port=CTL_PORT, health="/healthz")
    ports = (CDP_PORT, MINIWOB_PORT)
    reset = "fresh-claim"
    action_mode = "driver"
    width, height = 1280, 800

    def __init__(self) -> None:
        self.image = images.image("BENCH_WEB")


class LiveWebAdapter(BenchWebAdapter):
    """Live-web tasks: start at ``metadata["start_url"]``, judge by an LLM.

    Evidence for :meth:`judge`: the last ``screenshots_kept`` screenshots the
    agent left in ``session.screenshots`` (a list of PNG bytes, optional)
    plus the final screen, the agent's answer and its action history.
    """

    requires = frozenset({"egress", "openai"})
    screenshots_kept = 3

    async def setup(self, task: Any, session: Any, ep: Any) -> None:
        await reset(ep, task.metadata["start_url"])

    async def evidence(self, session: Any) -> dict:
        shots = [s for s in (getattr(session, "screenshots", None) or []) if s]
        final = await session.screenshot()
        shots = (shots + [final])[-self.screenshots_kept:]
        return {"screenshots": shots, "answer": agent_answer(session),
                "actions": agent_actions(session)}

    async def judge(self, task: Any, evidence: dict) -> Optional[float]:
        raise NotImplementedError

    async def evaluate(self, task: Any, session: Any, ep: Any) -> Optional[float]:
        return await self.judge(task, await self.evidence(session))
