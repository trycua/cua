"""Cua Driver trajectory observer for local desktop participation smokes."""

from __future__ import annotations

import json
import os
import queue
import re
import shutil
import signal
import subprocess
import sys
import threading
import unicodedata
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from cua_bench_runtime.adapters import ObserverAdapter
from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.errors import HarnessFailure
from cua_bench_runtime.model import AgentOutcome, EnvironmentHandle, ObserverReport, TrialContext
from cua_bench_runtime.process import clean_environment

_ACTION_CAPABILITY = {
    "click": "pointer_input",
    "double_click": "pointer_input",
    "right_click": "pointer_input",
    "type_text": "keyboard_input",
    "press_key": "keyboard_input",
    "hotkey": "keyboard_input",
    "set_value": "keyboard_input",
}


def _platform_key() -> str:
    if sys.platform == "darwin":
        return "macos"
    if os.name == "nt":
        return "windows"
    return "linux"


class McpClient:
    def __init__(self, command: Path) -> None:
        creationflags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
        self.process = subprocess.Popen(
            [str(command), "mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            env=clean_environment({"CUA_DRIVER_RS_TELEMETRY_ENABLED": "false"}),
            shell=False,
            start_new_session=os.name != "nt",
            creationflags=creationflags,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise HarnessFailure("could not open Cua Driver MCP pipes")
        self.stdin = self.process.stdin
        self.responses: queue.Queue[str] = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.next_id = 2
        initialized = self._request(1, "initialize", {})
        if "error" in initialized:
            self.close()
            raise HarnessFailure("Cua Driver MCP initialize failed")
        self.server_version = initialized.get("result", {}).get("serverInfo", {}).get("version")

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.responses.put(line)

    def _request(self, request_id: int, method: str, params: dict[str, Any]) -> dict:
        request = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        }
        self.stdin.write(json.dumps(request, separators=(",", ":")) + "\n")
        self.stdin.flush()
        while True:
            try:
                line = self.responses.get(timeout=20.0)
            except queue.Empty as error:
                raise HarnessFailure(f"Cua Driver MCP timed out during {method}") from error
            try:
                response = json.loads(line)
            except json.JSONDecodeError:
                continue
            if response.get("id") == request_id:
                return response

    def call(self, tool: str, arguments: Mapping[str, Any]) -> dict:
        request_id = self.next_id
        self.next_id += 1
        response = self._request(
            request_id,
            "tools/call",
            {"name": tool, "arguments": dict(arguments)},
        )
        if "error" in response or response.get("result", {}).get("isError") is True:
            raise HarnessFailure(f"Cua Driver tool failed: {tool}")
        structured = response.get("result", {}).get("structuredContent", {})
        return structured if isinstance(structured, dict) else {}

    def close(self) -> None:
        if self.process.poll() is not None:
            return
        try:
            self.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=2.0)
            return
        except subprocess.TimeoutExpired:
            pass
        if os.name == "nt":
            self.process.terminate()
        else:
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                return
        try:
            self.process.wait(timeout=2.0)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=2.0)


def _normalized_text(value: str) -> str:
    return " ".join(unicodedata.normalize("NFKC", value).casefold().split())


def _fact_tokens(value: Any) -> list[str]:
    if isinstance(value, Mapping):
        return [token for item in value.values() for token in _fact_tokens(item)]
    if isinstance(value, Sequence) and not isinstance(value, (str, bytes, bytearray)):
        return [token for item in value for token in _fact_tokens(item)]
    if value is None:
        return ["null"]
    if isinstance(value, bool):
        return ["true" if value else "false"]
    return [_normalized_text(str(value))]


def _contains_facts(text: str, facts: Mapping[str, Any]) -> bool:
    haystack = _normalized_text(text)
    return all(
        token and re.search(rf"(?<!\w){re.escape(token)}(?!\w)", haystack) is not None
        for value in facts.values()
        for token in _fact_tokens(value)
    )


def merge_app_maps(initial: Mapping[int, str], final: Mapping[int, str]) -> dict[int, str]:
    """Retain stable PID identities and reject ambiguous PID reuse."""

    merged = dict(initial)
    for pid, bundle_id in final.items():
        previous = merged.get(pid)
        if previous is not None and previous != bundle_id:
            merged.pop(pid, None)
            continue
        merged[pid] = bundle_id
    return merged


class CuaRecordingObserver(ObserverAdapter):
    """Map Cua Driver's protected trajectory into content-free gate events.

    This diagnostic adapter is always ``non_certifying`` because the agent and
    observer share a host account. A certifying implementation must be a
    separate protected adapter with independently verifiable receipts; this
    class deliberately cannot be promoted by a constructor flag.

    A successful input turn may supply both the action and its observer-owned
    post-action readback when the resulting state already contains the declared
    facts; a later correlated observation is not required for this diagnostic.
    """

    name = "cua-driver-recording"

    @property
    def trust(self) -> str:
        return "non_certifying"

    def __init__(self) -> None:
        self.client: McpClient | None = None
        self.recording_dir: Path | None = None
        self.requirements: tuple[Mapping[str, Any], ...] = ()
        self.provider_version: str | None = None
        self.initial_apps: dict[int, str] = {}
        self.platform = _platform_key()

    def start(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        requirements: tuple[Mapping[str, Any], ...],
    ) -> None:
        platform = handle.facts.get("platform")
        self.platform = platform if platform in {"macos", "windows", "linux"} else _platform_key()
        command_name = "cua-driver.exe" if os.name == "nt" else "cua-driver"
        resolved = shutil.which(command_name)
        if resolved is None:
            raise HarnessFailure("cua-driver is not installed")
        observer_root = context.trial_dir / "observer"
        observer_root.mkdir()
        self.recording_dir = observer_root / "cua-driver-recording"
        self.recording_dir.mkdir()
        self.requirements = requirements
        self.client = McpClient(Path(resolved))
        self.provider_version = self.client.server_version
        self.initial_apps = self._apps_by_pid()
        self.client.call(
            "start_recording",
            {"output_dir": str(self.recording_dir), "record_video": False},
        )

    def _apps_by_pid(self) -> dict[int, str]:
        if self.client is None:
            return {}
        response = self.client.call("list_apps", {})
        apps = response.get("apps", [])
        return {
            app["pid"]: app["bundle_id"]
            for app in apps
            if isinstance(app, dict)
            and isinstance(app.get("pid"), int)
            and isinstance(app.get("bundle_id"), str)
            and app["bundle_id"]
        }

    def _events(self, apps: Mapping[int, str]) -> list[dict[str, Any]]:
        if self.recording_dir is None:
            return []
        provider: dict[str, str] = {"id": "trycua.cua-driver"}
        if self.provider_version:
            provider["version"] = self.provider_version
        events: list[dict[str, Any]] = []
        for turn in sorted(self.recording_dir.glob("turn-[0-9][0-9][0-9][0-9][0-9]")):
            try:
                action = json.loads((turn / "action.json").read_text(encoding="utf-8"))
                before = json.loads((turn / "before_state.json").read_text(encoding="utf-8"))
                after = json.loads((turn / "after_state.json").read_text(encoding="utf-8"))
            except (OSError, UnicodeDecodeError, json.JSONDecodeError):
                continue
            arguments = action.get("arguments", {})
            pid = after.get("pid", arguments.get("pid"))
            window_id = after.get("window_id", arguments.get("window_id"))
            if (
                action.get("result_error") is not False
                or not isinstance(pid, int)
                or pid <= 0
                or not isinstance(window_id, int)
                or window_id <= 0
                or pid not in apps
            ):
                continue
            before_text = before.get("tree_markdown", "")
            after_text = after.get("tree_markdown", "")
            if not isinstance(before_text, str) or not isinstance(after_text, str):
                continue
            bundle_id = apps[pid]
            for requirement in self.requirements:
                target = requirement["target"]
                accepted = target.get("platform_application_ids", {})
                platform_ids = accepted.get(self.platform, [])
                if accepted and (not platform_ids or bundle_id not in platform_ids):
                    continue
                correlation = f"cua-recording:{requirement['id']}:{pid}:{window_id}"
                event_target = {
                    "application_id": target["application_id"],
                    "surface_id": target["surface_id"],
                    "platform_application_id": bundle_id,
                    "process_id": pid,
                    "window_id": str(window_id),
                }
                for step in requirement["sequence"]:
                    facts = step.get("required_facts", {})
                    kind = step["kind"]
                    if kind == "act":
                        if action.get("tool") not in _ACTION_CAPABILITY:
                            continue
                        matched = _contains_facts(
                            json.dumps(action, sort_keys=True) + "\n" + before_text,
                            facts,
                        )
                    elif kind in {"observe", "readback"}:
                        matched = _contains_facts(after_text, facts)
                    else:
                        matched = False
                    if not matched:
                        continue
                    events.append(
                        {
                            "provider": provider,
                            "capability_class": (
                                _ACTION_CAPABILITY.get(action.get("tool"), "composite")
                                if kind == "act"
                                else "accessibility_observation"
                            ),
                            "target": event_target,
                            "kind": kind,
                            "correlation_id": correlation,
                            "fact_digests": {
                                key: digest_json(value) for key, value in sorted(facts.items())
                            },
                        }
                    )
        return events

    def finish(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome | None,
    ) -> ObserverReport:
        del context, handle, outcome
        if self.client is None:
            return ObserverReport(
                name=self.name,
                trust="unavailable",
                detail="Cua Driver observer did not start",
            )
        try:
            apps = merge_app_maps(self.initial_apps, self._apps_by_pid())
            try:
                self.client.call("stop_recording", {})
            except HarnessFailure:
                # The daemon currently ends a global local recording when any
                # participating MCP transport closes. A local non-certifying
                # smoke may still map the completed immutable turns. A
                # certifying adapter must fail closed on this discontinuity.
                pass
            events = tuple(self._events(apps))
            return ObserverReport(
                name=self.name,
                trust=self.trust,
                events=events,
                detail=f"mapped {len(events)} normalized events",
            )
        finally:
            self.client.close()
            self.client = None
