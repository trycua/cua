"""Independent, fail-closed fixture oracles for held-input lifecycle tests.

Application journals prove received input; Wayland client logs prove primary
pointer/keyboard events. Plugin counters and Driver responses cannot substitute
for either. Use only disposable fixtures: raw wire logs can contain keycodes.
"""
import re


INPUT = re.compile(r"\b(wl_pointer|wl_keyboard)(?:#|@)(\d+)\.(\w+)\(")
POINTER_EVENTS = {"enter", "leave", "motion", "button", "axis"}
KEYBOARD_EVENTS = {"enter", "leave", "key", "modifiers"}


def primary_wire_events(text):
    """Return event categories only, never keycodes, text, or raw log lines."""
    events = []
    for line in text.splitlines():
        match = INPUT.search(line)
        if not match:
            continue
        interface, _resource, event = match.groups()
        allowed = POINTER_EVENTS if interface == "wl_pointer" else KEYBOARD_EVENTS
        if event in allowed:
            events.append(interface + "." + event)
    return events


def held_release(rows, *, fault_ns, maximum_latency_ms=750):
    """Require a real press before the fault and its matching release afterward.

An eventual release at normal drag completion is not prompt cancellation. The
caller must inject the fault with more than maximum_latency_ms left in the drag.
"""
    relevant = [row for row in rows if row["kind"] in ("button-press", "button-release")]
    if not relevant or relevant[0]["kind"] != "button-press":
        raise AssertionError("missing captured press before fault")
    held = set()
    fault_held = None
    release_ns = None
    previous = -1
    for row in relevant:
        stamp = row["time"]
        if type(stamp) is not int or stamp < previous:
            raise AssertionError("invalid or unordered application timestamps")
        previous = stamp
        if stamp >= fault_ns and fault_held is None:
            fault_held = set(held)
        button = row["button"]
        if row["kind"] == "button-press":
            if button in held or stamp >= fault_ns:
                raise AssertionError("duplicate or post-fault press")
            held.add(button)
        else:
            if button not in held:
                raise AssertionError("release without matching press")
            held.remove(button)
            if stamp >= fault_ns and not held:
                release_ns = stamp
    if held or not fault_held or release_ns is None:
        raise AssertionError("fault did not interrupt held input with a proven release")
    latency = (release_ns - fault_ns) / 1_000_000
    if not 0 <= latency <= maximum_latency_ms:
        raise AssertionError("held input was not released within the cancellation bound")
    return {"held_at_fault": True, "release_latency_ms": latency, "balanced_buttons": True}


def unchanged_primary(before, after, wire):
    for field in ("clicks", "keys", "held"):
        if field not in before or field not in after or before[field] != after[field]:
            raise AssertionError("foreground application state changed or is missing")
    if before["held"] is not True:
        raise AssertionError("foreground had no active grab")
    events = primary_wire_events(wire)
    if events:
        raise AssertionError("unexpected primary input events: " + ", ".join(events))
    return {"foreground_grab_preserved": True, "primary_wire_input_events": 0}
