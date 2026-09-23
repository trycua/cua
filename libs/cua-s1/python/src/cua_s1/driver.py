"""Synchronous, model-agnostic integration with the public Cua Driver tools."""

from __future__ import annotations

import asyncio
import json
import shutil
import subprocess
import threading
import time
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from typing import Any, Literal

from .schema import Element

DeliveryMode = Literal["background", "foreground"]


class DriverError(RuntimeError):
    """A structured driver failure suitable for an API response."""

    def __init__(
        self,
        code: str,
        message: str,
        *,
        tool: str | None = None,
        details: dict[str, Any] | None = None,
        outcome_unknown: bool = False,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.tool = tool
        self.details = details or {}
        self.outcome_unknown = outcome_unknown

    def to_dict(self) -> dict[str, Any]:
        return {
            "code": self.code,
            "message": self.message,
            "tool": self.tool,
            "outcome_unknown": self.outcome_unknown,
            "details": self.details,
        }


@dataclass(frozen=True)
class WindowTarget:
    pid: int
    window_id: int

    def __post_init__(self) -> None:
        if (
            isinstance(self.pid, bool)
            or isinstance(self.window_id, bool)
            or not isinstance(self.pid, int)
            or not isinstance(self.window_id, int)
            or self.pid <= 0
            or self.window_id <= 0
        ):
            raise ValueError("pid and window_id must be positive")

    def to_driver_target(self) -> dict[str, Any]:
        return {"kind": "window", "pid": self.pid, "window_id": self.window_id}


@dataclass
class WindowSnapshot:
    target: WindowTarget
    elements: list[Element]
    snapshot_id: str | None
    tree_markdown: str
    elements_complete: bool | None
    raw: dict[str, Any] = field(repr=False)

    def require_executable(self) -> None:
        """Refuse element actions unless the observation is complete and bound."""

        if not self.snapshot_id:
            raise DriverError(
                "snapshot_id_required",
                "Execution requires a snapshot-bound window observation",
            )
        if self.elements_complete is not True:
            raise DriverError(
                "incomplete_window_snapshot",
                "Execution requires a complete accessibility element snapshot",
                details={"elements_complete": self.elements_complete},
            )

    def element_for(self, element: Element) -> Element:
        """Resolve the planned element in this snapshot without exposing raw state."""

        self.require_executable()
        matches = [candidate for candidate in self.elements if candidate.index == element.index]
        if len(matches) != 1:
            raise DriverError(
                "element_not_uniquely_resolved",
                "The planned element is not unique in the fresh window snapshot",
                details={"element_index": element.index, "matches": len(matches)},
            )
        current = matches[0]
        if current.role != element.role or current.label != element.label:
            semantic_matches = [
                candidate
                for candidate in self.elements
                if candidate.role == element.role and candidate.label == element.label
            ]
            if len(semantic_matches) != 1:
                raise DriverError(
                    "element_identity_changed",
                    "The planned element could not be uniquely identified after re-observation",
                    details={
                        "element_index": element.index,
                        "role": element.role,
                        "label": element.label,
                        "matches": len(semantic_matches),
                    },
                )
            current = semantic_matches[0]
        return current

    def token_for(self, element: Element) -> str:
        """Resolve an element only within this snapshot, never by bare index."""

        current = self.element_for(element)
        if not current.element_token:
            raise DriverError(
                "element_token_missing",
                "Cua Driver did not provide a snapshot-bound token for the element",
                details={
                    "element_index": current.index,
                    "role": current.role,
                    "label": current.label,
                },
            )
        return current.element_token


@dataclass
class MutationResult:
    action: dict[str, Any]
    observation: WindowSnapshot


class BaseDriver:
    def __init__(self, session: str = "cua-s1", timeout_seconds: float = 120.0) -> None:
        self.session = session
        self.timeout_seconds = timeout_seconds
        self.timings: list[dict[str, Any]] = []
        self._available_tools: set[str] | None = None

    def call(self, tool: str, **args: Any) -> dict[str, Any]:  # pragma: no cover - abstract
        raise NotImplementedError

    def start_session(self) -> dict[str, Any]:
        return self.call("start_session", session=self.session)

    def end_session(self) -> dict[str, Any]:
        return self.call("end_session", session=self.session)

    def list_windows(self, pid: int | None = None) -> list[dict[str, Any]]:
        result = self.call("list_windows", pid=pid, on_screen_only=True)
        windows = result.get("windows")
        if not isinstance(windows, list):
            raise DriverError(
                "invalid_driver_response",
                "list_windows did not return a windows array",
                tool="list_windows",
                details={"response": _bounded_repr(result)},
            )
        return windows

    def find_window(
        self,
        title_contains: str | None = None,
        pid: int | None = None,
        window_id: int | None = None,
    ) -> dict[str, Any]:
        """Return exactly one on-screen window; ambiguity is a hard failure."""

        windows = self.list_windows(pid)
        if window_id is not None:
            windows = [window for window in windows if window.get("window_id") == window_id]
        if title_contains:
            needle = title_contains.casefold()
            windows = [
                window for window in windows if needle in str(window.get("title") or "").casefold()
            ]
        windows = [window for window in windows if window.get("is_on_screen", True)]
        selector = {"title_contains": title_contains, "pid": pid, "window_id": window_id}
        if not windows:
            raise DriverError(
                "window_not_found", "No window matched the requested target", details=selector
            )
        if len(windows) != 1:
            raise DriverError(
                "ambiguous_window",
                "More than one window matched; provide an exact pid and window_id",
                details={
                    **selector,
                    "matches": [
                        {
                            "pid": window.get("pid"),
                            "window_id": window.get("window_id"),
                            "app_name": window.get("app_name"),
                            "title": window.get("title"),
                        }
                        for window in windows
                    ],
                },
            )
        return windows[0]

    def window_state(self, target: WindowTarget) -> WindowSnapshot:
        state = self.call(
            "get_window_state",
            pid=target.pid,
            window_id=target.window_id,
            session=self.session,
            include_accessibility_tree=True,
            include_screenshot=False,
        )
        raw_elements = state.get("elements")
        if not isinstance(raw_elements, list):
            raise DriverError(
                "window_elements_unavailable",
                "The exact window snapshot did not include structured accessibility elements",
                tool="get_window_state",
                details={
                    "target": target.to_driver_target(),
                    "degradation": state.get("degradation"),
                    "response": _bounded_repr(state),
                },
            )
        markdown = str(state.get("tree_markdown") or "")
        elements = [_element_from_driver(raw, markdown) for raw in raw_elements]
        return WindowSnapshot(
            target=target,
            elements=elements,
            snapshot_id=_optional_string(state.get("snapshot_id")),
            tree_markdown=markdown,
            elements_complete=state.get("elements_complete")
            if isinstance(state.get("elements_complete"), bool)
            else None,
            raw=state,
        )

    def supports_value_mutation(self) -> bool:
        """Return whether value mutation is injected or runtime-advertised.

        ``set_value`` is optional and intentionally absent from the portable
        manifest contract. A subclass override is an explicit host injection;
        transport drivers fail closed unless live discovery advertises it.
        """

        if type(self).set_value is not BaseDriver.set_value:
            return True
        return self._available_tools is not None and "set_value" in self._available_tools

    def set_value(self, target: WindowTarget, element_token: str, value: str) -> MutationResult:
        """Replace an AX/UIA/AT-SPI value, then re-observe the exact window.

        The public ``set_value`` tool is an accessibility-addressed background
        operation and does not expose the pointer delivery-mode field.
        """

        if not self.supports_value_mutation():
            raise DriverError(
                "unsupported_driver_capability",
                "The connected Cua Driver runtime did not advertise token-based set_value",
                tool="set_value",
            )
        return self._mutate_then_observe(
            "set_value",
            target,
            pid=target.pid,
            window_id=target.window_id,
            element_token=_require_token(element_token),
            value=value,
            session=self.session,
        )

    def click(
        self,
        target: WindowTarget,
        element_token: str,
        *,
        delivery_mode: DeliveryMode,
    ) -> MutationResult:
        if delivery_mode not in ("background", "foreground"):
            raise DriverError(
                "invalid_delivery_mode",
                "delivery_mode must be 'background' or 'foreground'",
                tool="click",
                details={"delivery_mode": delivery_mode},
            )
        return self._mutate_then_observe(
            "click",
            target,
            target=target.to_driver_target(),
            element_token=_require_token(element_token),
            delivery_mode=delivery_mode,
            session=self.session,
            button="left",
            count=1,
        )

    def _mutate_then_observe(
        self,
        tool: str,
        window_target: WindowTarget,
        **payload: Any,
    ) -> MutationResult:
        try:
            action = self.call(tool, **payload)
        except DriverError as exc:
            # A timeout or transport failure may occur after delivery. Always
            # refresh state before exposing the unknown outcome to the caller.
            try:
                observation = self.window_state(window_target)
                exc.details["post_action_observation"] = _snapshot_summary(observation)
            except DriverError as observe_error:
                exc.details["observation_error"] = observe_error.to_dict()
            raise
        observation = self.window_state(window_target)
        effect = action.get("effect")
        if effect != "confirmed":
            raise DriverError(
                "action_effect_unconfirmed",
                "Cua Driver did not confirm the action effect",
                tool=tool,
                details={
                    "effect": effect if isinstance(effect, str) else "missing",
                    "route": action.get("route"),
                    "escalation": action.get("escalation"),
                    "post_action_observation": _snapshot_summary(observation),
                },
                outcome_unknown=effect not in {"refused", "suspected_noop"},
            )
        return MutationResult(action=action, observation=observation)

    def close(self) -> None:
        return None


class CliDriver(BaseDriver):
    def __init__(
        self,
        session: str = "cua-s1",
        binary: str | None = None,
        timeout_seconds: float = 120.0,
    ) -> None:
        super().__init__(session, timeout_seconds)
        self.binary = binary or shutil.which("cua-driver") or "cua-driver"
        self._available_tools = self._discover_tools()

    def _discover_tools(self) -> set[str] | None:
        try:
            process = subprocess.run(
                [self.binary, "list-tools"],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=min(self.timeout_seconds, 30.0),
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        if process.returncode != 0:
            return None
        return {
            line.split(":", 1)[0].strip()
            for line in process.stdout.splitlines()
            if line.split(":", 1)[0].strip()
        }

    def call(self, tool: str, **args: Any) -> dict[str, Any]:
        payload = {key: value for key, value in args.items() if value is not None}
        started = time.perf_counter()
        try:
            process = subprocess.run(
                [self.binary, "call", tool],
                input=json.dumps(payload),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=self.timeout_seconds,
                check=False,
            )
        except subprocess.TimeoutExpired as exc:
            raise DriverError(
                "driver_timeout",
                f"Cua Driver tool '{tool}' timed out",
                tool=tool,
                outcome_unknown=True,
            ) from exc
        except OSError as exc:
            raise DriverError(
                "driver_unavailable",
                "Could not start the Cua Driver CLI",
                tool=tool,
                details={"binary": self.binary},
            ) from exc
        finally:
            self.timings.append(
                {"tool": tool, "ms": round((time.perf_counter() - started) * 1000, 1)}
            )

        stdout = process.stdout.strip()
        data = _parse(stdout)
        if process.returncode != 0:
            raise DriverError(
                "driver_tool_failed",
                f"Cua Driver tool '{tool}' failed",
                tool=tool,
                details={
                    "returncode": process.returncode,
                    "stderr": process.stderr.strip()[:1000],
                    "response": _bounded_repr(data),
                },
            )
        if _response_is_error(data):
            raise DriverError(
                "driver_tool_refused",
                f"Cua Driver tool '{tool}' refused the request",
                tool=tool,
                details={"response": data},
            )
        return data


class McpDriver(BaseDriver):
    """One persistent ``cua-driver mcp`` process behind a synchronous facade."""

    def __init__(
        self,
        session: str = "cua-s1",
        binary: str | None = None,
        timeout_seconds: float = 120.0,
    ) -> None:
        super().__init__(session, timeout_seconds)
        self.binary = binary or shutil.which("cua-driver") or "cua-driver"
        self._loop = asyncio.new_event_loop()
        self._thread = threading.Thread(target=self._loop.run_forever, daemon=True)
        self._thread.start()
        self._session: Any = None
        self._stack: AsyncExitStack | None = None
        try:
            self._run(self._connect())
        except Exception:
            self._loop.call_soon_threadsafe(self._loop.stop)
            self._thread.join(timeout=1)
            raise

    def _run(self, coroutine: Any) -> Any:
        future = asyncio.run_coroutine_threadsafe(coroutine, self._loop)
        try:
            return future.result(timeout=self.timeout_seconds)
        except TimeoutError as exc:
            future.cancel()
            raise DriverError("driver_timeout", "Cua Driver MCP operation timed out") from exc

    async def _connect(self) -> None:
        try:
            from mcp import ClientSession, StdioServerParameters
            from mcp.client.stdio import stdio_client
        except ImportError as exc:
            raise DriverError(
                "mcp_dependency_missing",
                "The MCP transport requires the optional 'mcp' dependency",
            ) from exc

        self._stack = AsyncExitStack()
        read, write = await self._stack.enter_async_context(
            stdio_client(StdioServerParameters(command=self.binary, args=["mcp"]))
        )
        self._session = await self._stack.enter_async_context(ClientSession(read, write))
        await self._session.initialize()
        tools_result = await self._session.list_tools()
        self._available_tools = {
            str(tool.name)
            for tool in getattr(tools_result, "tools", ())
            if getattr(tool, "name", None)
        }

    async def _call(self, tool: str, payload: dict[str, Any]) -> tuple[dict[str, Any], bool]:
        result = await self._session.call_tool(tool, payload)
        structured = getattr(result, "structuredContent", None)
        if structured is None:
            structured = getattr(result, "structured_content", None)
        if structured is not None:
            data = structured if isinstance(structured, dict) else {"data": structured}
        else:
            text = "\n".join(
                str(getattr(content, "text", ""))
                for content in result.content
                if getattr(content, "text", None)
            )
            data = _parse(text)
        is_error = bool(getattr(result, "isError", getattr(result, "is_error", False)))
        return data, is_error

    def call(self, tool: str, **args: Any) -> dict[str, Any]:
        payload = {key: value for key, value in args.items() if value is not None}
        started = time.perf_counter()
        try:
            data, is_error = self._run(self._call(tool, payload))
        except DriverError:
            raise
        except Exception as exc:
            raise DriverError(
                "driver_transport_failed",
                f"Cua Driver MCP call '{tool}' failed",
                tool=tool,
                details={"error_type": type(exc).__name__, "error": str(exc)[:1000]},
                outcome_unknown=True,
            ) from exc
        finally:
            self.timings.append(
                {"tool": tool, "ms": round((time.perf_counter() - started) * 1000, 1)}
            )
        if is_error or _response_is_error(data):
            raise DriverError(
                "driver_tool_refused",
                f"Cua Driver tool '{tool}' refused the request",
                tool=tool,
                details={"response": data},
            )
        return data

    def close(self) -> None:
        if self._stack is not None:
            try:
                self._run(self._stack.aclose())
            except Exception:
                pass
        self._loop.call_soon_threadsafe(self._loop.stop)
        self._thread.join(timeout=1)


def CuaDriver(
    session: str = "cua-s1",
    transport: Literal["auto", "mcp", "cli"] = "auto",
    *,
    binary: str | None = None,
    timeout_seconds: float = 120.0,
) -> BaseDriver:
    """Create a driver without coupling the runtime to any inference model."""

    if transport not in ("auto", "mcp", "cli"):
        raise ValueError("transport must be 'auto', 'mcp', or 'cli'")
    if transport in ("auto", "mcp"):
        try:
            return McpDriver(session, binary=binary, timeout_seconds=timeout_seconds)
        except Exception:
            if transport == "mcp":
                raise
    return CliDriver(session, binary=binary, timeout_seconds=timeout_seconds)


def target_from_window(window: dict[str, Any]) -> WindowTarget:
    try:
        return WindowTarget(pid=int(window["pid"]), window_id=int(window["window_id"]))
    except (KeyError, TypeError, ValueError) as exc:
        raise DriverError(
            "invalid_window_record",
            "Window record lacks a valid pid and window_id",
            details={"window": _bounded_repr(window)},
        ) from exc


def _element_from_driver(raw: Any, markdown: str) -> Element:
    if not isinstance(raw, dict):
        raise DriverError(
            "invalid_driver_response",
            "Window element was not an object",
            tool="get_window_state",
            details={"element": _bounded_repr(raw)},
        )
    try:
        index = int(raw["element_index"])
    except (KeyError, TypeError, ValueError) as exc:
        raise DriverError(
            "invalid_driver_response",
            "Window element lacked a valid element_index",
            tool="get_window_state",
            details={"element": _bounded_repr(raw)},
        ) from exc
    value = str(raw.get("value") or _value_from_markdown(markdown, index))
    return Element(
        role=str(raw.get("role") or ""),
        label=str(raw.get("label") or ""),
        value=value,
        index=index,
        actions=tuple(str(action) for action in (raw.get("actions") or ())),
        checked=_checked(raw),
        frame=_frame(raw),
        element_token=_optional_string(raw.get("element_token")),
    )


def _parse(text: str) -> dict[str, Any]:
    if not text.strip():
        return {}
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        return {"text": text.strip()}
    return value if isinstance(value, dict) else {"data": value}


def _response_is_error(data: dict[str, Any]) -> bool:
    return data.get("isError") is True or data.get("is_error") is True or "error" in data


def _frame(raw: dict[str, Any]) -> tuple[int, int, int, int] | None:
    frame = raw.get("frame") or {}
    if not isinstance(frame, dict) or not frame:
        return None
    width = frame.get("width", frame.get("w", 0))
    height = frame.get("height", frame.get("h", 0))
    try:
        return int(frame.get("x", 0)), int(frame.get("y", 0)), int(width), int(height)
    except (TypeError, ValueError):
        return None


def _checked(raw: dict[str, Any]) -> bool | None:
    for key in ("toggle_state", "checked", "selected", "state"):
        value = raw.get(key)
        if isinstance(value, bool):
            return value
        if isinstance(value, str):
            lowered = value.casefold()
            if lowered in {"on", "checked", "true", "selected"}:
                return True
            if lowered in {"off", "unchecked", "false", "unselected"}:
                return False
    return None


def _value_from_markdown(markdown: str, index: int) -> str:
    tag = f"[{index}] "
    for line in markdown.splitlines():
        if tag in line and '[value="' in line:
            return line.split('[value="', 1)[1].split('"', 1)[0]
    return ""


def _optional_string(value: Any) -> str | None:
    return value if isinstance(value, str) and value else None


def _require_token(token: str) -> str:
    if not isinstance(token, str) or not token.strip():
        raise DriverError("element_token_missing", "A snapshot-bound element token is required")
    return token


def _bounded_repr(value: Any, limit: int = 1000) -> str:
    return repr(value)[:limit]


def _snapshot_summary(snapshot: WindowSnapshot) -> dict[str, Any]:
    return {
        "target": snapshot.target.to_driver_target(),
        "snapshot_id": snapshot.snapshot_id,
        "element_count": len(snapshot.elements),
        "elements_complete": snapshot.elements_complete,
    }
