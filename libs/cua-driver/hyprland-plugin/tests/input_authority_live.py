"""Opt-in Stop/Cancel grant-boundary probe in a disposable Hyprland session.

No input action is dispatched. The operator signs grants outside the guest;
this probe tests pending/active authority, not application delivery. Use the
held-input Driver harness separately for release and primary-input evidence.
"""

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import re
import subprocess
import time

from input_transport_test import approved_packet, connect, exchange, refused


def lane_state(status):
    experiment = status.get("experiment", {})
    assert experiment.get("protocol") == 0 and experiment.get("test_only") is True
    assert experiment.get("transport_ready") is True
    lanes = experiment.get("lanes", [])
    assert len(lanes) == 2 and [lane["lane"] for lane in lanes] == [0, 1]
    for lane in lanes:
        assert type(lane.get("lease_active")) is bool
        assert type(lane.get("dispatches")) is int and lane["dispatches"] >= 0
        assert lane.get("held_button") == 0 and lane.get("held_keys") == 0
        assert lane.get("drag_active") is False
        assert lane.get("pointer_focus") is False and lane.get("keyboard_focus") is False
    return lanes


def exercise(command, clients, hellos, operators, targets, obtain_grants, read_status, send=exchange):
    """Exercise real transport by default; injected seams test the runner only."""
    assert command in ("STOP", "CANCEL")
    before = lane_state(read_status())
    assert not any(lane["lease_active"] for lane in before), "test requires idle input lanes"
    baseline = [lane["dispatches"] for lane in before]
    selected = [send(clients[i], f"TARGET {target['pid']} {target['address']}")
                for i, target in enumerate(targets)]
    assert all(item.get("ok") is True for item in selected)

    def request(name, lane, target):
        return {"name": name, "lane": lane, "capabilities": 1,
                "epoch": hellos[lane]["epoch"], "challenge": hellos[lane]["challenge"],
                "target": target["target"]}

    requests = [request("active-0", 0, selected[0]), request("renewal-0", 0, selected[0]),
                request("pending-1", 1, selected[1])]
    grants = obtain_grants("before", requests)
    packets = {row["name"]: approved_packet(grants[row["name"]], hellos[row["lane"]], selected[row["lane"]])
               for row in requests}
    assert int(packets["renewal-0"].split()[3]) > int(packets["active-0"].split()[3]), "renewal must have a later deadline"
    assert send(operators[0], packets["active-0"]).get("ok") is True, "initial positive-control grant refused"
    assert [lane["lease_active"] for lane in lane_state(read_status())] == [True, False]

    # Only the first grant has been redeemed. A high-water deadline check alone
    # cannot reject either the unused renewal or the other lane's pending grant.
    assert send(operators[0], command).get("ok") is True
    assert not any(lane["lease_active"] for lane in lane_state(read_status()))
    refused(send(operators[0], packets["active-0"]), "stale_target")
    refused(send(operators[0], packets["renewal-0"]), "stale_target")
    if command == "STOP":
        refused(send(operators[1], packets["pending-1"]), "stale_target")
        affected = [0, 1]
    else:
        assert send(operators[1], packets["pending-1"]).get("ok") is True, "Cancel invalidated the sibling lane"
        affected = [0]

    fresh = {}
    for lane in affected:
        target = targets[lane]
        fresh[lane] = send(clients[lane], f"TARGET {target['pid']} {target['address']}")
        assert fresh[lane].get("ok") is True
        assert fresh[lane]["target"] != selected[lane]["target"], "revocation retained target binding"
    # Rebinding on the same action connections must not revive old signatures.
    refused(send(operators[0], packets["renewal-0"]), "stale_target")
    if command == "STOP":
        refused(send(operators[1], packets["pending-1"]), "stale_target")
    requests = [request(f"fresh-{lane}", lane, fresh[lane]) for lane in affected]
    accepted_deadline = int(packets["active-0"].split()[3])
    requests[0]["expires_before_unix_ms"] = accepted_deadline
    grants = obtain_grants("after", requests)
    for row in requests:
        lane = row["lane"]
        packet = approved_packet(grants[row["name"]], hellos[lane], fresh[lane])
        if lane == 0:
            assert int(packet.split()[3]) < accepted_deadline, "fresh lane-0 grant must expire before the revoked grant"
        assert send(operators[lane], packet).get("ok") is True, "fresh positive-control grant refused"
    after = lane_state(read_status())
    assert all(lane["lease_active"] for lane in after)
    assert [lane["dispatches"] for lane in after] == baseline, "authority probe unexpectedly dispatched input"
    return {"result": "passed", "command": command, "unused_renewal_refused": True,
            "pending_sibling": "revoked" if command == "STOP" else "preserved",
            "fresh_grants_accepted": True, "shorter_fresh_grant_accepted": True,
            "control_connections_preserved": True,
            "input_dispatched": False, "held_input_cleanup_tested": False,
            "application_delivery_tested": False, "full_desktop_matrix": False}


def validate_targets(targets):
    if not isinstance(targets, list) or len(targets) != 2:
        raise ValueError("plan must name exactly two native disposable fixture targets")
    for target in targets:
        if (not isinstance(target, dict) or set(target) != {"pid", "address"}
                or type(target["pid"]) is not int or not 0 < target["pid"] < (1 << 31)
                or not isinstance(target["address"], str)
                or re.fullmatch(r"[0-9a-f]{1,16}", target["address"]) is None
                or int(target["address"], 16) == 0):
            raise ValueError("target needs a positive PID and lowercase hex address without 0x")
    if targets[0]["pid"] == targets[1]["pid"]:
        raise ValueError("targets must be separate application processes")


def run(args):
    if not args.disposable:
        raise ValueError("requires an explicitly selected disposable session")
    targets = json.loads(args.plan.read_text())
    validate_targets(targets)
    args.evidence.mkdir(parents=True, exist_ok=False)

    def read_status():
        return json.loads(subprocess.check_output(["hyprctl", "-j", "cua:status"], timeout=5))

    def obtain_grants(stage, requests):
        (args.evidence / f"request-{stage}.json").write_text(json.dumps(requests, indent=2))
        path = args.evidence / f"grants-{stage}.json"
        assert not path.exists(), "do not prepopulate grants for unobserved targets"
        print(f"GRANTS_REQUIRED {stage}", flush=True)
        deadline = time.monotonic() + 40
        while not path.exists():
            if time.monotonic() >= deadline:
                raise TimeoutError("external operator grants did not arrive")
            time.sleep(0.05)
        value = json.loads(path.read_text())
        assert isinstance(value, dict) and set(value) == {row["name"] for row in requests}
        return value

    result = {"result": "failed", "command": args.command}
    try:
        initial = lane_state(read_status())
        assert not any(lane["lease_active"] or lane.get("reserved") is not False for lane in initial), "test requires unowned idle lanes"
        with ExitStack() as stack:
            clients, hellos, operators = [], [], []
            for name in ("cua-input-test.sock", "cua-input-test-2.sock"):
                client, hello = connect(args.input_directory / name, claim=True)
                clients.append(stack.enter_context(client))
                hellos.append(hello)
                operator, _ = connect(args.input_directory / name)
                operators.append(stack.enter_context(operator))
                # Registered after the socket, so Stop runs before it closes,
                # including partial setup or a failed assertion/grant transfer.
                stack.callback(exchange, operator, "STOP")
            result = exercise(args.command, clients, hellos, operators, targets, obtain_grants, read_status)
        assert not any(lane["lease_active"] for lane in lane_state(read_status())), "cleanup left active authority"
        result["cleanup_verified"] = True
    except Exception as error:
        result = {"result": "failed", "command": args.command, "error_type": type(error).__name__}
        raise
    finally:
        (args.evidence / "result.json").write_text(json.dumps(result, indent=2))


if __name__ == "__main__":
    if not __debug__:
        raise RuntimeError("assertions must be enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", choices=["STOP", "CANCEL"], required=True)
    parser.add_argument("--input-directory", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--disposable", action="store_true")
    run(parser.parse_args())
