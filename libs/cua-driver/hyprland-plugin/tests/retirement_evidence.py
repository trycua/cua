"""Fail-closed, pure evidence oracles for native input-seat retirement.

Passing these helpers on synthetic data is not native compositor evidence.
"""
import re


NAMES = {"Cua-Test-Agent", "Cua-Test-Agent-2"}


def active_experiment(status):
    experiment = status.get("experiment", {})
    if (status.get("name") != "cua-hyprland-plugin"
            or status.get("state") != "input_experiment"
            or status.get("configured") is not True
            or status.get("abi", {}).get("match") is not True
            or status.get("transport", {}).get("ready") is not True
            or experiment.get("transport_ready") is not True
            or experiment.get("test_only") is not True
            or experiment.get("protocol") != 0
            or experiment.get("seat_lifetime") != "compositor"
            or experiment.get("upgrade") != "desktop_restart"):
        raise AssertionError("require the active, ABI-matched restart-required experiment")
    lanes = experiment.get("lanes", [])
    if len(lanes) != 2 or {lane.get("lane") for lane in lanes} != {0, 1}:
        raise AssertionError("require both input lanes")
    for lane in lanes:
        if any(lane.get(field) != 0 for field in ("held_button", "held_keys")) or any(
                lane.get(field) is not False for field in ("lease_active", "drag_active", "reserved")):
            raise AssertionError("input lanes must be idle and unclaimed before the test")


def replacement_refusal(status):
    """A missing plugin/disabled config/ABI error cannot pass retirement refusal.

PLUGIN_INIT registers status successfully; the enabled config reload then
refuses seat creation and records its runtime error. A load exit code alone
does not prove this path was reached.
"""
    transport = status.get("transport", {})
    errors = {
        "input seat lifetime unavailable; restart the desktop before loading a replacement plugin",
        "restart desktop before replacing input seats",
    }
    if (status.get("name") != "cua-hyprland-plugin"
            or status.get("configured") is not True
            or status.get("abi", {}).get("match") is not True
            or status.get("state") != "discovery_only"
            or status.get("compositor_epoch") != 0
            or transport.get("ready") is not False
            or transport.get("last_error") not in errors
            or status.get("capabilities", {}).get("enabled") != []
            or "experiment" in status):
        raise AssertionError("missing explicit configured runtime retirement refusal")
    return {"runtime_restart_refusal": True, "transport_ready": False}


def bound_seats(wire):
    """Resolve both named synthetic seats to registry globals and client IDs."""
    bindings = {}
    for match in re.finditer(
            r'wl_registry[#@]\d+\.bind\((\d+), "wl_seat", \d+, new id [^#@\n]+[#@](\d+)\)', wire):
        global_id, resource = map(int, match.groups())
        bindings[resource] = global_id
    named = {}
    for match in re.finditer(r'wl_seat[#@](\d+)\.name\("([^"]+)"\)', wire):
        resource, name = match.groups()
        if name in NAMES:
            resource = int(resource)
            if name in named or resource not in bindings:
                raise AssertionError("ambiguous or missing synthetic seat binding")
            if not re.search(rf'wl_seat[#@]{resource}\.capabilities\([1-7]\)', wire):
                raise AssertionError("synthetic seat has no enabled capabilities evidence")
            named[name] = (resource, bindings[resource])
    if set(named) != NAMES or len(set(named.values())) != 2:
        raise AssertionError("both distinct synthetic seats must be bound before unload")
    return named


def retired_seats(seats, wire):
    if set(seats) != NAMES or len(set(seats.values())) != 2:
        raise AssertionError("require both distinct preflight seat bindings")
    for resource, global_id in seats.values():
        if not re.search(rf'wl_seat[#@]{resource}\.capabilities\(0\)', wire):
            raise AssertionError("missing zero-capability notification for retired seat")
        if not re.search(rf'wl_registry[#@]\d+\.global_remove\({global_id}\)', wire):
            raise AssertionError("missing retired seat global removal")
        if re.search(rf'wl_seat[#@]{resource}\.capabilities\([1-7]\)', wire):
            raise AssertionError("retired seat capabilities re-enabled")
    if re.search(r'wl_registry[#@]\d+\.global\(\d+, "wl_seat",', wire):
        raise AssertionError("unexpected new seat global after retirement")
    return {"retired_seats": len(seats), "capabilities_zero": True, "globals_removed": True}


def socket_closed(client):
    """Require EOF/reset; timeouts and arbitrary I/O errors are failures."""
    try:
        if client.recv(2048) != b"":
            raise AssertionError("old connection still returned data")
    except ConnectionResetError:
        pass
    return True


def service_owns_socket(unix_table, fd_links, path):
    """Bind the requested MCP socket to the selected live service, not a label."""
    rows = [line.split(maxsplit=7) for line in unix_table.splitlines()]
    matches = [row for row in rows if len(row) == 8 and row[7] == str(path)]
    if len(matches) != 1 or "socket:[" + matches[0][6] + "]" not in fd_links:
        raise AssertionError("Driver socket is not uniquely owned by the selected service")
