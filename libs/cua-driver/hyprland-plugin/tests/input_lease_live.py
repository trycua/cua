"""Opt-in expiry/disconnect checks against the disposable-VM input socket.

The operator signs request.json outside the guest and supplies only grant.json.
No input should be delivered: every attempted click must refuse. This probes
lease invalidation, not held-input cleanup or application compatibility.
"""
import argparse
import json
from pathlib import Path
import time

from input_transport_test import connect, exchange, refused, approved_packet


def run(args):
    args.evidence.mkdir(parents=True, exist_ok=False)
    client, hello = connect(args.socket, claim=True)
    operator, _ = connect(args.socket)
    try:
        target = exchange(client, f"TARGET {args.pid} {args.address}")
        assert target.get("ok"), target
        request = {**hello, **target, "capabilities": 1}
        (args.evidence / "request.json").write_text(json.dumps(request))
        print(json.dumps({"event": "grant_required", **request}), flush=True)
        path = args.evidence / "grant.json"
        deadline = time.monotonic() + 120
        while not path.exists():
            if time.monotonic() >= deadline:
                raise TimeoutError("public operator grant did not arrive")
            time.sleep(0.05)
        grant = json.loads(path.read_text())
        packet = approved_packet(grant, hello, target)
        assert exchange(operator, packet).get("ok"), "approval failed"
        if args.case == "expiry":
            remaining = int(packet.split()[3]) / 1000 - time.time()
            if not 0 < remaining <= 20:
                raise ValueError("expiry probe needs a grant with at most 20 seconds remaining")
            time.sleep(remaining + 0.2)
        else:
            client.close()
            time.sleep(0.1)
            client, fresh = connect(args.socket, claim=True)
            assert fresh["challenge"] != hello["challenge"]
            assert fresh["epoch"] == hello["epoch"]
            target = exchange(client, f"TARGET {args.pid} {args.address}")
            assert target.get("ok"), target
            refused(exchange(operator, packet), "stale_target")
        response = exchange(client, f"CLICK 1 {target['target']} {target['revision']} 200 300 272 1")
        refused(response, "pending_operator_approval")
        result = {"result": "passed", "case": args.case, "input_refused": True,
                  "held_state_cleanup_tested": False}
        (args.evidence / "result.json").write_text(json.dumps(result))
        print(json.dumps(result), flush=True)
    finally:
        try:
            exchange(operator, "STOP")
        finally:
            operator.close()
            client.close()


if __name__ == "__main__":
    if not __debug__:
        raise RuntimeError("assertions must be enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=["expiry", "disconnect"], required=True)
    parser.add_argument("--socket", type=Path, required=True)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--address", required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    run(parser.parse_args())
