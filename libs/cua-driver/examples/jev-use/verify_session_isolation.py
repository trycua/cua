"""Real two-session browser authority isolation proof for the jev-use CI lane.

The proof uses one Driver MCP process, two named Cua sessions, two independently
owned isolated Chromium profiles, and two independent loopback fixture journals.
A cross-session target/ref must refuse before it can mutate the other fixture.
Ending session A must revoke A without making session B unusable.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

BASE = Path(__file__).resolve().parent
sys.path.insert(0, str(BASE / "python"))

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from driver_env import driver_environment
from run import Driver, select_tab_id, wait_for_window
from tasks import fixture_state, reset_fixture
from verify_setup import fixture


@dataclass
class BrowserBinding:
    driver: Driver
    url: str
    pid: int
    window_id: int
    target_id: str
    tab_id: str
    field_ref: str
    submit_ref: str


def _refs(snapshot: dict[str, Any]) -> tuple[str, str]:
    refs = snapshot.get("refs") or []
    fields = [
        ref
        for ref in refs
        if ref.get("role") == "textbox"
        and ref.get("name") == "verification value"
        and ref.get("ref")
    ]
    submits = [
        ref
        for ref in refs
        if ref.get("role") == "button"
        and ref.get("name") == "Submit"
        and ref.get("ref")
    ]
    if len(fields) != 1 or len(submits) != 1:
        raise RuntimeError(
            f"expected one field and one Submit ref, got {len(fields)} / {len(submits)}"
        )
    return str(fields[0]["ref"]), str(submits[0]["ref"])


async def _snapshot(binding: BrowserBinding) -> dict[str, Any]:
    return await binding.driver.call(
        "get_browser_state",
        {
            "target_id": binding.target_id,
            "tab_id": binding.tab_id,
            "snapshot_format": "semantic_v2",
        },
    )


async def _refresh_refs(binding: BrowserBinding) -> None:
    snapshot = await _snapshot(binding)
    binding.field_ref, binding.submit_ref = _refs(snapshot)


async def _prepare(driver: Driver, url: str) -> BrowserBinding:
    prepared = await driver.call(
        "browser_prepare",
        {"allow_launch": True, "profile": {"mode": "isolated_new"}},
    )
    pid = int(prepared["prepared_pid"])
    window = await wait_for_window(driver, pid)
    window_id = int(window["window_id"])
    state = await driver.call(
        "get_browser_state",
        {"pid": pid, "window_id": window_id},
    )
    target_id = str(state["target_id"])
    tab_id = select_tab_id(state["tabs"])
    await driver.call(
        "browser_navigate",
        {"target_id": target_id, "tab_id": tab_id, "url": url},
    )
    snapshot = await driver.call(
        "get_browser_state",
        {
            "target_id": target_id,
            "tab_id": tab_id,
            "snapshot_format": "semantic_v2",
        },
    )
    field_ref, submit_ref = _refs(snapshot)
    return BrowserBinding(
        driver=driver,
        url=url,
        pid=pid,
        window_id=window_id,
        target_id=target_id,
        tab_id=tab_id,
        field_ref=field_ref,
        submit_ref=submit_ref,
    )


async def _expect_refusal(
    driver: Driver,
    tool: str,
    arguments: dict[str, Any],
    expected_code: str,
) -> str:
    """Accept both ordinary ToolResult refusals and typed ActionResult refusals."""
    result = await driver.session.call_tool(
        tool,
        {**arguments, "session": driver.label},
    )
    structured = getattr(result, "structuredContent", None)
    structured = structured if isinstance(structured, dict) else {}
    refusal = structured.get("refusal")
    refusal = refusal if isinstance(refusal, dict) else {}
    action_error = structured.get("error")
    action_error = action_error if isinstance(action_error, dict) else {}
    code = (
        structured.get("code")
        or refusal.get("code")
        or action_error.get("code")
    )
    refused = (
        getattr(result, "isError", False)
        or structured.get("status") == "refused"
        or structured.get("effect") == "refused"
        or bool(refusal)
    )
    if not refused:
        raise RuntimeError(
            f"{tool} unexpectedly succeeded; expected refusal {expected_code!r}"
        )
    if code != expected_code:
        raise RuntimeError(
            f"{tool} refused for the wrong reason: {code!r}; "
            f"expected {expected_code!r}"
        )
    return str(code)


async def _expect_foreign_refusal(
    foreign_driver: Driver,
    tool: str,
    arguments: dict[str, Any],
) -> str:
    return await _expect_refusal(
        foreign_driver,
        tool,
        arguments,
        "browser_binding_stale",
    )


def _type_args(binding: BrowserBinding, ref: str, text: str) -> dict[str, Any]:
    return {
        "target_id": binding.target_id,
        "tab_id": binding.tab_id,
        "ref": ref,
        "text": text,
        "replace": True,
    }


def _click_args(binding: BrowserBinding, ref: str) -> dict[str, Any]:
    return {
        "target_id": binding.target_id,
        "tab_id": binding.tab_id,
        "ref": ref,
        "input_route": "dom_event",
    }


async def _type_own(binding: BrowserBinding, token: str) -> None:
    await binding.driver.call(
        "browser_type",
        _type_args(binding, binding.field_ref, token),
    )
    await _refresh_refs(binding)


async def _submit_own(binding: BrowserBinding) -> None:
    await binding.driver.call(
        "browser_click",
        _click_args(binding, binding.submit_ref),
    )


async def run(output: Path) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "complete": False,
        "topology": "one_mcp_process_two_named_sessions_two_isolated_profiles",
        "cross_pre_mutation": {},
        "cross_fresh_completion": {},
        "session_end": {},
    }
    params = StdioServerParameters(
        command=os.getenv("CUA_DRIVER_BIN", "cua-driver"),
        args=["mcp"],
        env=driver_environment(),
    )

    with fixture() as url_a, fixture() as url_b:
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                driver_a = Driver(session, "jev-isolation-a")
                driver_b = Driver(session, "jev-isolation-b")
                a_ended = False
                b_ended = False
                try:
                    a = await _prepare(driver_a, url_a)
                    b = await _prepare(driver_b, url_b)

                    # Pre-mutation capability cross-use: try to type into A
                    # through B's Cua session and vice versa. Then use the
                    # owner's Submit ref as a target-owned journal probe. If
                    # the foreign type landed, the fixture would submit it.
                    code_b_to_a = await _expect_foreign_refusal(
                        driver_b,
                        "browser_type",
                        _type_args(a, a.field_ref, "foreign-b-to-a"),
                    )
                    await _submit_own(a)
                    state_a = fixture_state(url_a)
                    if state_a != {"submitted": None}:
                        raise RuntimeError(f"B mutated A through A's ref: {state_a}")

                    code_a_to_b = await _expect_foreign_refusal(
                        driver_a,
                        "browser_type",
                        _type_args(b, b.field_ref, "foreign-a-to-b"),
                    )
                    await _submit_own(b)
                    state_b = fixture_state(url_b)
                    if state_b != {"submitted": None}:
                        raise RuntimeError(f"A mutated B through B's ref: {state_b}")

                    summary["cross_pre_mutation"] = {
                        "a_to_b_refusal": code_a_to_b,
                        "b_to_a_refusal": code_b_to_a,
                        "a_journal_unchanged": True,
                        "b_journal_unchanged": True,
                    }

                    # Re-snapshot after those owner clicks, perform the first
                    # mutation in each owning session, then deliberately cross
                    # the freshly minted completion refs. The independent
                    # fixture journals must remain unsubmitted.
                    await _refresh_refs(a)
                    await _refresh_refs(b)
                    token_a = "session-a-owned"
                    token_b = "session-b-owned"
                    await _type_own(a, token_a)
                    await _type_own(b, token_b)

                    code_b_to_a_fresh = await _expect_foreign_refusal(
                        driver_b,
                        "browser_click",
                        _click_args(a, a.submit_ref),
                    )
                    code_a_to_b_fresh = await _expect_foreign_refusal(
                        driver_a,
                        "browser_click",
                        _click_args(b, b.submit_ref),
                    )
                    if fixture_state(url_a) != {"submitted": None}:
                        raise RuntimeError("foreign fresh A completion mutated fixture A")
                    if fixture_state(url_b) != {"submitted": None}:
                        raise RuntimeError("foreign fresh B completion mutated fixture B")

                    await _submit_own(a)
                    if fixture_state(url_a) != {"submitted": token_a}:
                        raise RuntimeError("A could not complete through A-owned authority")
                    if fixture_state(url_b) != {"submitted": None}:
                        raise RuntimeError("A-owned completion changed fixture B")

                    await _submit_own(b)
                    if fixture_state(url_b) != {"submitted": token_b}:
                        raise RuntimeError("B could not complete through B-owned authority")

                    summary["cross_fresh_completion"] = {
                        "a_to_b_refusal": code_a_to_b_fresh,
                        "b_to_a_refusal": code_b_to_a_fresh,
                        "a_to_a_verified": True,
                        "b_to_b_verified": True,
                        "cross_mutation_count": 0,
                    }

                    # Ending A must revoke A's old target/ref namespace without
                    # retiring B. Reset A's target-owned journal first so a
                    # replay would be externally visible.
                    reset_fixture(url_a)
                    ended = await driver_a.call("end_session", {})
                    a_ended = True
                    if ended.get("active") is not False:
                        raise RuntimeError(f"session A did not end: {ended}")

                    ended_a_refusal = await _expect_refusal(
                        driver_a,
                        "browser_click",
                        _click_args(a, a.submit_ref),
                        "session_ended",
                    )
                    if fixture_state(url_a) != {"submitted": None}:
                        raise RuntimeError("ended session A replayed its old completion")

                    # Re-open the same public label as a new lifecycle episode.
                    # The dispatch gate should now admit the session, but the
                    # old browser target/tab/ref namespace must still be gone.
                    restarted = await driver_a.call("start_session", {})
                    a_ended = False
                    if restarted.get("active") is not True:
                        raise RuntimeError(f"session A did not restart: {restarted}")
                    old_a_capability_refusal = await _expect_foreign_refusal(
                        driver_a,
                        "browser_click",
                        _click_args(a, a.submit_ref),
                    )
                    if fixture_state(url_a) != {"submitted": None}:
                        raise RuntimeError(
                            "restarted session A inherited its old browser authority"
                        )

                    # Reuse B's existing target after A teardown/restart, not a newly
                    # prepared browser. This is the isolation property.
                    reset_fixture(url_b)
                    await driver_b.call(
                        "browser_navigate",
                        {
                            "target_id": b.target_id,
                            "tab_id": b.tab_id,
                            "url": url_b,
                        },
                    )
                    await _refresh_refs(b)
                    token_b_after = "session-b-after-a-end"
                    await _type_own(b, token_b_after)
                    await _submit_own(b)
                    if fixture_state(url_b) != {"submitted": token_b_after}:
                        raise RuntimeError("ending A invalidated B's still-live browser authority")

                    summary["session_end"] = {
                        "a_ended_session_refusal": ended_a_refusal,
                        "a_old_authority_after_restart_refusal": old_a_capability_refusal,
                        "a_old_journal_unchanged": True,
                        "b_existing_target_survived_a_end": True,
                    }
                    summary["complete"] = True
                finally:
                    if not a_ended:
                        try:
                            await driver_a.call("end_session", {})
                        except Exception:
                            pass
                    if not b_ended:
                        try:
                            await driver_b.call("end_session", {})
                            b_ended = True
                        except Exception:
                            pass

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    return summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    summary = asyncio.run(run(args.output))
    print(json.dumps(summary, indent=2, sort_keys=True))
    raise SystemExit(0 if summary["complete"] else 1)


if __name__ == "__main__":
    main()
