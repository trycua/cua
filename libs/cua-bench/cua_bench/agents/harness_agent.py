"""A coding-agent harness (Claude Code, Codex, Gemini CLI, ...) as the agent.

"Harness X in image Y": the task's own sandbox (image Y) runs harness X
through the cua SDK's agents API (``cua-agents``, over the Agent Client
Protocol). The harness gets the sandbox's own MCP tools (cua-driver: screen,
mouse, keyboard, windows) plus any extra MCP servers, works until its turn
ends, and the task's evaluator scores the sandbox afterwards as usual.

    CUA_BENCH_HARNESS=claude-code CUA_BENCH_HARNESS_KEYS=ANTHROPIC_API_KEY \
        cb run task tasks/my_task --agent harness

Settings (constructor kwargs, else environment variables):

| kwarg | env | meaning |
| --- | --- | --- |
| ``harness`` | ``CUA_BENCH_HARNESS`` | harness id (default ``claude-code``; ``cua agent harnesses``) |
| ``env_from_host`` | ``CUA_BENCH_HARNESS_KEYS`` | provider key variables forwarded from this process (comma list) |
| ``model`` | ``CUA_BENCH_HARNESS_MODEL`` | model id (``cb run --model`` sets it) |
| ``base_url`` | ``CUA_BENCH_HARNESS_BASE_URL`` | custom endpoint (proxy, gateway, mock provider) |
| ``mcp`` | ``CUA_BENCH_HARNESS_MCP`` | extra MCP servers, ``NAME=URL`` (comma list) |
| ``timeout_s`` | ``CUA_BENCH_HARNESS_TIMEOUT`` | turn budget in seconds (default 1800) |
| ``spacesd_url`` / ``spacesd_token`` | ``CUA_BENCH_SPACESD_URL`` / ``_TOKEN`` | reach cua-spacesd directly instead of through the session |

Requirement: the session must be a cua-spacesd sandbox (the canonical images,
``cb run`` targets, ``Sandbox.connect`` refs). A legacy computer-server
session has no spacesd; pass ``spacesd_url``/``spacesd_token`` then.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import TYPE_CHECKING, Any, Optional

from . import register_agent
from .base import AgentResult, BaseAgent, FailureMode

if TYPE_CHECKING:
    from ..computers import DesktopSession

DEFAULT_HARNESS = "claude-code"
DEFAULT_TIMEOUT_S = 1800
#: Upper bound on event pages copied into the logging directory.
MAX_EVENT_PAGES = 500


def _split(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [v.strip() for v in value.split(",") if v.strip()]
    return [str(v) for v in value]


def parse_mcp(spec: str) -> dict[str, str]:
    """``NAME=URL`` (streamable HTTP, as reachable from inside the sandbox)."""
    name, sep, url = spec.partition("=")
    if not sep or not name or not url.startswith(("http://", "https://")):
        raise ValueError(f"MCP server {spec!r}: expected NAME=http(s)://...")
    return {"name": name, "url": url}


def usage_tokens(usage_json: Optional[str]) -> tuple[int, int]:
    """(input, output) tokens from an ACP usage object (camelCase)."""
    if not usage_json:
        return 0, 0
    try:
        u = json.loads(usage_json)
    except (TypeError, json.JSONDecodeError):
        return 0, 0
    if not isinstance(u, dict):
        return 0, 0

    def num(*keys: str) -> int:
        for k in keys:
            v = u.get(k)
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                return int(v)
        return 0

    return num("inputTokens", "input_tokens"), num("outputTokens", "output_tokens")


def failure_mode(status: str, stop_reason: Optional[str], error: Optional[str]) -> FailureMode:
    """How a finished run maps onto cua-bench's failure modes."""
    if status in ("failed", "crashed") or stop_reason in ("error", "refusal"):
        return FailureMode.API_ERROR if error else FailureMode.UNKNOWN
    if stop_reason in ("max_tokens", "max_turn_requests"):
        return FailureMode.MAX_STEPS_EXCEEDED
    if stop_reason == "cancelled":
        return FailureMode.UNKNOWN
    return FailureMode.NONE


@register_agent("harness")
class HarnessAgent(BaseAgent):
    """Runs a coding-agent harness inside the task's sandbox."""

    def __init__(self, **kwargs: Any):
        super().__init__(**kwargs)
        env = os.environ.get
        self.harness = kwargs.get("harness") or env("CUA_BENCH_HARNESS") or DEFAULT_HARNESS
        self.env_from_host = _split(kwargs.get("env_from_host", env("CUA_BENCH_HARNESS_KEYS")))
        self.env = dict(kwargs.get("env") or {})
        self.model = kwargs.get("model") or env("CUA_BENCH_HARNESS_MODEL")
        self.base_url = kwargs.get("base_url") or env("CUA_BENCH_HARNESS_BASE_URL")
        self.mcp = [parse_mcp(s) for s in _split(kwargs.get("mcp", env("CUA_BENCH_HARNESS_MCP")))]
        self.timeout_s = float(
            kwargs.get("timeout_s") or env("CUA_BENCH_HARNESS_TIMEOUT") or DEFAULT_TIMEOUT_S
        )
        self.spacesd_url = kwargs.get("spacesd_url") or env("CUA_BENCH_SPACESD_URL")
        self.spacesd_token = kwargs.get("spacesd_token") or env("CUA_BENCH_SPACESD_TOKEN")
        self.keep_run = bool(kwargs.get("keep_run", False))

    @staticmethod
    def name() -> str:
        return "harness"

    async def agents_for(self, session: "DesktopSession") -> Any:
        """The ``cua.Agents`` of the task's sandbox."""
        import cua

        if self.spacesd_url:
            guest = await cua.embedded().spacesd(self.spacesd_url, self.spacesd_token)
            return await guest.agents()
        sandbox = getattr(session, "sandbox", None)
        if sandbox is None and hasattr(session, "_ensure_computer"):
            await session._ensure_computer()
            sandbox = getattr(session, "sandbox", None)
        if sandbox is None or not hasattr(sandbox, "spacesd"):
            raise RuntimeError(
                "the harness agent needs a cua-spacesd sandbox session "
                "(or spacesd_url / CUA_BENCH_SPACESD_URL)"
            )
        guest = await sandbox.spacesd()
        return await guest.agents()

    def options(self) -> Any:
        import cua

        return cua.AgentRunOptions(
            env=self.env,
            env_from_host=self.env_from_host,
            model=self.model,
            base_url=self.base_url,
            mcp_servers=[cua.AgentRunMcpServer(**m) for m in self.mcp],
            label="cua-bench",
        )

    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        prompt = self._render_instruction(task_description)
        try:
            agents = await self.agents_for(session)
            run = await agents.run(self.harness, prompt, self.options())
        except Exception as e:  # noqa: BLE001 - reported as the agent's failure
            print(f"harness agent: could not start {self.harness}: {e}")
            return AgentResult(failure_mode=FailureMode.API_ERROR)

        timed_out = False
        try:
            res = await run.wait(int(self.timeout_s * 1000))
        except Exception as e:  # noqa: BLE001 - a timeout, or the guest went away
            timed_out = "Timeout" in type(e).__name__ or "still" in str(e)
            print(f"harness agent: {run.run_id()} did not finish: {e}")
            try:
                await run.interrupt()
            except Exception:  # noqa: BLE001
                pass
            try:
                res = await run.result()
            except Exception:  # noqa: BLE001
                res = None

        if logging_dir is not None:
            await self._save(run, res, Path(logging_dir))
        if not self.keep_run:
            try:
                await run.stop()
            except Exception:  # noqa: BLE001 - best effort; the sandbox goes away with the task
                pass

        if res is None:
            return AgentResult(failure_mode=FailureMode.UNKNOWN)
        tokens_in, tokens_out = usage_tokens(res.usage_json)
        mode = (
            FailureMode.MAX_STEPS_EXCEEDED
            if timed_out
            else failure_mode(res.status, res.stop_reason, res.error)
        )
        return AgentResult(
            total_input_tokens=tokens_in, total_output_tokens=tokens_out, failure_mode=mode
        )

    async def _save(self, run: Any, res: Any, out: Path) -> None:
        """Events (JSON Lines, bounded) and the result into the logging dir."""
        out.mkdir(parents=True, exist_ok=True)
        try:
            cursor = 0
            with (out / "harness-events.jsonl").open("w") as f:
                for _ in range(MAX_EVENT_PAGES):
                    page = await run.events(cursor, None)
                    cursor = page.cursor
                    for e in page.events:
                        f.write(e.json + "\n")
                    if page.caught_up or not page.events:
                        break
            summary = {"run_id": run.run_id(), "harness": self.harness}
            if res is not None:
                summary |= {
                    "status": res.status,
                    "turn": res.turn,
                    "stop_reason": res.stop_reason,
                    "text": res.text,
                    "tool_calls": res.tool_calls,
                    "usage": json.loads(res.usage_json) if res.usage_json else None,
                    "error": res.error,
                }
            (out / "harness-result.json").write_text(json.dumps(summary, indent=2) + "\n")
        except Exception as e:  # noqa: BLE001 - logs never fail the task
            print(f"harness agent: could not save logs: {e}")
