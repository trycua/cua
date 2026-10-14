"""Fail-closed evidence for native desktop-state faults, separate from dispatch.

Destroyed surfaces cannot acknowledge release. Their case instead requires a
prompt typed refusal, compositor-confirmed disappearance, and cleared agent
state. Surviving surfaces must additionally journal a matching release.
"""
from collections import Counter
import math

from lifecycle_evidence import held_release, primary_wire_events
from primary_trace import analyze


CASES = ("destroy", "move", "resize", "lock", "dpms")
REFUSALS = {
    "destroy": {"cancelled", "stale_target"},
    "move": {"cancelled", "stale_target"},
    "resize": {"cancelled", "stale_target"},
    "lock": {"cancelled", "background_unavailable", "desktop_changed"},
    "dpms": {"cancelled", "background_unavailable", "desktop_changed"},
}
PRIMARY_FIELDS = {"application_delta", "wire_inputs", "wire_focus", "compositor_inputs",
                  "compositor_focus", "max_cursor_displacement", "cursor_path", "wm_before", "wm_after"}


def focus_transitions(events):
    streams = {"pointer": [], "keyboard": []}
    for event in events:
        family = "pointer" if "pointer" in event else "keyboard"
        if not streams[family] or streams[family][-1] != event:
            streams[family].append(event)
    return streams


def agent_cleared(status, lane):
    matches = [row for row in status["experiment"]["lanes"] if row["lane"] == lane]
    assert len(matches) == 1, "missing exact agent lane"
    state = matches[0]
    for name in ("lease_active", "drag_active", "pointer_focus", "keyboard_focus"):
        assert state.get(name) is False, "agent state not cleared: " + name
    for name in ("held_button", "held_keys"):
        assert type(state.get(name)) is int and state[name] == 0, "held agent state: " + name
    return {"agent_authority_cleared": True, "agent_input_state_cleared": True}


def resource_counts(status):
    lanes = status["experiment"]["lanes"]
    assert {row["lane"] for row in lanes} == {0, 1} and len(lanes) == 2
    result = []
    for row in sorted(lanes, key=lambda item: item["lane"]):
        counts = {name: row[name] for name in ("seat_resources", "pointer_resources", "keyboard_resources")}
        assert all(type(value) is int and value >= 0 for value in counts.values())
        result.append({"lane": row["lane"], **counts})
    return result


def destroyed_resources_pruned(baseline, before, after):
    # The host captures this baseline before launching the exact target client,
    # after the observer, foreground fixture, and overlay have settled.
    assert baseline == resource_counts({"experiment": {"lanes": baseline}})
    initial, final = resource_counts(before), resource_counts(after)
    for base, live in zip(baseline, initial):
        for name in ("seat_resources", "pointer_resources", "keyboard_resources"):
            assert live[name] > base[name], "target resources were not observed before destruction"
    assert final == baseline, "destroyed client resources were not pruned to pre-target baseline"
    return {"destroyed_client_resources_pruned": True}


def cancellation(case, rows, *, fault_ns, observed_ns, response_ns, response,
                 status_ns, status, lane, duration_ms=2000, maximum_latency_ms=750,
                 target_destroyed=False):
    assert case in CASES
    stamps = (fault_ns, observed_ns, response_ns, status_ns)
    assert all(type(value) is int and value > 0 for value in stamps)
    presses = [row for row in rows if row["kind"] == "button-press"]
    assert len(presses) == 1, "need exactly one received press"
    press = presses[0]["time"]
    assert type(press) is int and press < fault_ns <= observed_ns, "fault must follow received press"
    normal_end = press + duration_ms * 1_000_000
    deadline = fault_ns + maximum_latency_ms * 1_000_000
    assert deadline < normal_end, "natural completion could satisfy cancellation deadline"
    assert fault_ns <= response_ns <= deadline, "typed cancellation was not prompt"
    assert fault_ns <= status_ns <= deadline, "agent cleanup was not observed promptly"
    assert response.get("isError") is True, "normal drag success is not cancellation"
    reason = response.get("structuredContent", {}).get("reason")
    assert reason in REFUSALS[case], "missing case-specific structured refusal"
    result = agent_cleared(status, lane)
    assert not any(row["kind"] in ("key-press", "key-release", "scroll") for row in rows), "unexpected non-drag input delivery"
    if case == "destroy":
        assert target_destroyed is True, "target disappearance not independently observed"
        result["release_oracle"] = "destroyed_surface_not_receivable"
    else:
        result.update(held_release(rows, fault_ns=fault_ns, maximum_latency_ms=maximum_latency_ms))
        result["release_oracle"] = "application_journal"
    return {**result, "action_refusal": reason,
            "refusal_latency_ms": (response_ns - fault_ns) / 1_000_000,
            "cleanup_latency_ms": (status_ns - fault_ns) / 1_000_000}


def recovery_input(rows, status, lane):
    keys = [row for row in rows if row["kind"] in ("key-press", "key-release")]
    assert [row["kind"] for row in keys] == ["key-press", "key-release"], "recovery key pair missing or duplicated"
    assert all(row.get("key") == "Escape" for row in keys), "unexpected recovery key"
    assert keys[0]["time"] <= keys[1]["time"], "unordered recovery keys"
    assert not any(row["kind"] in ("button-press", "button-release", "scroll") for row in rows), "unexpected recovery pointer input"
    selected = [row for row in status["experiment"]["lanes"] if row["lane"] == lane]
    assert len(selected) == 1 and selected[0].get("held_keys") == 0 and selected[0].get("held_button") == 0
    return {"freshly_approved_recovery_received": True, "recovery_key_balanced": True}


def primary_effect(before, after, wire, trace, wm_before, wm_after):
    """Summarize measured effects, without assuming locks preserve focus/grabs.

    Focus transition categories ignore duplicate compositor bookkeeping. Input
    event counts and press/release states remain exact, so normalization cannot
    erase a transient injected click or warp-and-return.
    """
    report = analyze(trace)
    assert report.get("telemetry_complete") is True, "missing complete compositor trace"
    assert before.get("held") is True, "primary grab not established"
    for item in (before, after):
        assert all(key in item for key in ("clicks", "keys", "held", "scroll", "motion"))
    assert after["keys"].startswith(before["keys"]), "foreground key history was replaced"
    primary = [row for row in trace["events"] if row[5] == 0]
    inputs = Counter((row[2], row[6]) for row in primary
                     if row[2] in ("pointer_button", "pointer_axis", "keyboard_key"))
    focus = focus_transitions(row[2] for row in primary if "focus" in row[2]
                              or row[2] in ("pointer_enter", "pointer_leave", "keyboard_enter", "keyboard_leave"))
    events = primary_wire_events(wire)
    wire_inputs = Counter(event for event in events
                          if event.rsplit(".", 1)[1] not in ("enter", "leave"))
    delta = {key: after[key] - before[key] for key in ("clicks", "scroll", "motion")}
    assert all(type(value) is int and value >= 0 for value in delta.values())
    return {"application_delta": {**delta, "keys": after["keys"][len(before["keys"]):],
                                  "held": after["held"]},
            "wire_inputs": sorted([key, value] for key, value in wire_inputs.items()),
            "wire_focus": focus_transitions(event for event in events if event not in wire_inputs),
            "compositor_inputs": sorted([kind, state, count] for (kind, state), count in inputs.items()),
            "compositor_focus": focus,
            "max_cursor_displacement": report["max_primary_displacement_px"],
            "cursor_path": [row[3:5] for row in primary if row[2] == "cursor"],
            "wm_before": wm_before, "wm_after": wm_after}


def compare_control(control, actual, identity):
    assert control.get("result") == "passed" and control.get("mode") == "control", "unproven control"
    assert control.get("identity") == identity, "control belongs to another fixture, fault, or candidate"
    expected = control["primary_effect"]
    assert set(expected) == PRIMARY_FIELDS and set(actual) == PRIMARY_FIELDS, "incomplete foreground evidence"
    for key in expected:
        assert key in actual, "missing independent evidence: " + key
        if key == "max_cursor_displacement":
            assert math.isfinite(actual[key]) and abs(actual[key] - expected[key]) <= 0.01, "extra primary cursor displacement"
        else:
            assert actual[key] == expected[key], "agent added foreground effects: " + key
    return {"fault_only_control_matched": True, "no_extra_foreground_effects": True}
