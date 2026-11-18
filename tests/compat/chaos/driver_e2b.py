#!/usr/bin/env python3
"""One chaos operation of a cua-chaos plan, performed with the unmodified E2B SDK.

cua-chaos runs this once per plan step with the step in $CHAOS_STEP (JSON), the
run directory in $CHAOS_DIR and E2B_API_KEY / E2B_DOMAIN / E2B_API_URL pointing
at the gateway. Sandboxes a client holds live in $CHAOS_DIR/slots/<actor>-<slot>.json
so a later step (or the cleanup) can reach them, like a long-lived program would.
`kill_after_ms` is enforced by the runner (SIGKILL of this process mid-operation);
`abandon_after_ms` is the SDK giving up (request_timeout), the client then exits.
"""

import json
import os
import sys
import threading
import time

from e2b import Sandbox

step = json.loads(os.environ["CHAOS_STEP"])
args = step.get("args", {})
actor = step["actor"]
slot = args.get("slot", 0)
d = os.path.join(os.environ["CHAOS_DIR"], "slots")
os.makedirs(d, exist_ok=True)
path = os.path.join(d, f"{actor}-{slot}.json")


def held():
    try:
        with open(path) as f:
            return json.load(f)["id"]
    except Exception:
        return None


def remember(sb):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump({"id": sb.sandbox_id}, f)
    os.replace(tmp, path)


def forget():
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def connect():
    sid = held()
    # connect() extends the sandbox's timeout (E2B default 300 s); keep it short
    # so the run's settle window covers the longest lifetime.
    return Sandbox.connect(sid, timeout=20) if sid else None


abandon = args.get("abandon_after_ms")
rt = (abandon / 1000.0) if abandon else None
name = step["name"]
try:
    if name == "create":
        sb = Sandbox.create(timeout=int(args.get("timeout_s", 30)), request_timeout=rt)
        remember(sb)
    elif name == "exec":
        sb = connect()
        if sb:
            sb.commands.run(f"sleep {args.get('seconds', 1)}; echo done", timeout=30)
    elif name == "upload":
        sb = connect()
        if sb:
            sb.files.write("/tmp/chaos.bin", os.urandom(int(args.get("kib", 1)) * 1024))
    elif name == "stream":
        sb = connect()
        if sb:
            sb.commands.run(
                f"for i in $(seq 1 {int(args.get('seconds', 2)) * 5}); do echo line$i; sleep 0.2; done",
                on_stdout=lambda _l: None,
                timeout=60,
            )
    elif name == "delete":
        sid = held()
        if sid:
            Sandbox.kill(sid)
            forget()
    elif name == "double_delete":
        sid = held()
        if sid:
            ts = [threading.Thread(target=lambda: Sandbox.kill(sid)) for _ in range(2)]
            [t.start() for t in ts]
            [t.join() for t in ts]
            forget()
    elif name == "delete_during_create":
        t = threading.Thread(target=lambda: remember(Sandbox.create(timeout=30)))
        t.start()
        time.sleep(0.3)
        for info in Sandbox.list().next_items():
            try:
                Sandbox.kill(info.sandbox_id)
            except Exception:
                pass
        t.join()
    elif name == "ttl_exec":
        # The sandbox's timeout expires while a command is running.
        sb = Sandbox.create(timeout=int(args.get("timeout_s", 3)))
        remember(sb)
        try:
            sb.commands.run(f"sleep {int(args.get('seconds', 10))}", timeout=60)
        except Exception as e:
            print("ttl_exec ended:", type(e).__name__)
    else:
        print("unknown op", name)
        sys.exit(3)
except Exception as e:  # a failed operation is data, not a harness error
    print(f"{name} failed: {type(e).__name__}: {e}")
    sys.exit(1)
