#!/usr/bin/env python3
"""One chaos operation of a cua-chaos plan with the unmodified Daytona SDK.
Same contract as driver_e2b.py: step JSON in $CHAOS_STEP, state in $CHAOS_DIR/slots.
`abandon_after_ms` is the SDK giving up (create timeout), `kill_after_ms` is
enforced by the runner (SIGKILL of this process)."""

import json
import os
import sys
import threading
import time

from daytona import CreateSandboxFromSnapshotParams, Daytona

step = json.loads(os.environ["CHAOS_STEP"])
args = step.get("args", {})
actor, slot = step["actor"], args.get("slot", 0)
d = os.path.join(os.environ["CHAOS_DIR"], "slots")
os.makedirs(d, exist_ok=True)
path = os.path.join(d, f"{actor}-{slot}.json")
dt = Daytona()


def held():
    try:
        return json.load(open(path))["id"]
    except Exception:
        return None


def remember(sb):
    tmp = path + ".tmp"
    json.dump({"id": sb.id}, open(tmp, "w"))
    os.replace(tmp, path)


def forget():
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


name = step["name"]
ab = args.get("abandon_after_ms")
try:
    if name == "create":
        # Daytona sandboxes outlive the client: short auto-stop/auto-delete so a
        # killed client's sandbox goes away like the real service's would.
        p = CreateSandboxFromSnapshotParams(auto_stop_interval=1, auto_delete_interval=2)
        remember(dt.create(p, timeout=(ab / 1000.0 if ab else 120)))
    elif name == "exec":
        sid = held()
        if sid:
            dt.get(sid).process.exec(f"sleep {args.get('seconds', 1)}; echo done", timeout=30)
    elif name == "upload":
        sid = held()
        if sid:
            dt.get(sid).fs.upload_file(os.urandom(int(args.get("kib", 1)) * 1024), "/tmp/chaos.bin")
    elif name == "stream":
        sid = held()
        if sid:
            dt.get(sid).process.exec(
                f"for i in $(seq 1 {int(args.get('seconds', 2)) * 5}); do echo l$i; sleep 0.2; done",
                timeout=60,
            )
    elif name == "delete":
        sid = held()
        if sid:
            dt.get(sid).delete()
            forget()
    elif name == "double_delete":
        sid = held()
        if sid:
            sb = dt.get(sid)
            ts = [threading.Thread(target=lambda: sb.delete()) for _ in range(2)]
            [t.start() for t in ts]
            [t.join() for t in ts]
            forget()
    elif name == "delete_during_create":
        t = threading.Thread(
            target=lambda: remember(
                dt.create(CreateSandboxFromSnapshotParams(auto_delete_interval=2), timeout=120)
            )
        )
        t.start()
        time.sleep(0.3)
        for sb in dt.list().items:
            try:
                sb.delete()
            except Exception:
                pass
        t.join()
    elif name == "ttl_exec":
        sb = dt.create(
            CreateSandboxFromSnapshotParams(auto_stop_interval=1, auto_delete_interval=2),
            timeout=120,
        )
        remember(sb)
        try:
            sb.process.exec(f"sleep {int(args.get('seconds', 10))}", timeout=60)
        except Exception as e:
            print("ttl_exec ended:", type(e).__name__)
    else:
        sys.exit(3)
except Exception as e:
    print(f"{name} failed: {type(e).__name__}: {e}")
    sys.exit(1)
