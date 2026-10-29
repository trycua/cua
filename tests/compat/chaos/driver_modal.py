#!/usr/bin/env python3
"""One chaos operation of a cua-chaos plan with the unmodified Modal SDK.
Same contract as driver_e2b.py. Sandboxes of a long-lived app (App.lookup) are
kept in $CHAOS_DIR/slots; `create_ephemeral` runs inside `app.run()` so that a
SIGKILL of this process stops the heartbeats and the gateway must end the
App's Sandboxes (Modal semantics), with no help from the client."""

import json
import os
import sys
import threading
import time

import modal

step = json.loads(os.environ["CHAOS_STEP"])
args = step.get("args", {})
actor, slot = step["actor"], args.get("slot", 0)
d = os.path.join(os.environ["CHAOS_DIR"], "slots")
os.makedirs(d, exist_ok=True)
path = os.path.join(d, f"{actor}-{slot}.json")
IMG = modal.Image.from_registry(os.environ.get("CHAOS_IMAGE", "ghcr.io/trycua/linux:24.04"))


def app():
    return modal.App.lookup("chaos", create_if_missing=True)


def held():
    try:
        return json.load(open(path))["id"]
    except Exception:
        return None


def remember(sb):
    tmp = path + ".tmp"
    json.dump({"id": sb.object_id}, open(tmp, "w"))
    os.replace(tmp, path)


def forget():
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def sandbox():
    sid = held()
    return modal.Sandbox.from_id(sid) if sid else None


name = step["name"]
try:
    if name == "create":
        remember(
            modal.Sandbox.create(
                "sleep", "3600", app=app(), image=IMG, timeout=int(args.get("timeout_s", 30))
            )
        )
    elif name == "create_ephemeral":
        a = modal.App("chaos-eph")
        with a.run():
            modal.Sandbox.create("sleep", "3600", app=a, image=IMG, timeout=300)
            time.sleep(int(args.get("seconds", 20)))
    elif name == "exec":
        sb = sandbox()
        if sb:
            p = sb.exec("sh", "-c", f"sleep {args.get('seconds', 1)}; echo done", timeout=30)
            p.wait()
    elif name == "upload":
        sb = sandbox()
        if sb:
            with sb.open("/tmp/chaos.bin", "wb") as f:
                f.write(os.urandom(int(args.get("kib", 1)) * 1024))
    elif name == "stream":
        sb = sandbox()
        if sb:
            p = sb.exec(
                "sh",
                "-c",
                f"for i in $(seq 1 {int(args.get('seconds', 2)) * 5}); do echo l$i; sleep 0.2; done",
                timeout=60,
            )
            for _ in p.stdout:
                pass
    elif name == "delete":
        sb = sandbox()
        if sb:
            sb.terminate()
            forget()
    elif name == "double_delete":
        sb = sandbox()
        if sb:
            ts = [threading.Thread(target=lambda: sb.terminate()) for _ in range(2)]
            [t.start() for t in ts]
            [t.join() for t in ts]
            forget()
    elif name == "delete_during_create":
        t = threading.Thread(
            target=lambda: remember(
                modal.Sandbox.create("sleep", "3600", app=app(), image=IMG, timeout=30)
            )
        )
        t.start()
        time.sleep(0.3)
        for sb in modal.Sandbox.list(app_id=app().app_id):
            try:
                sb.terminate()
            except Exception:
                pass
        t.join()
    elif name == "ttl_exec":
        sb = modal.Sandbox.create(
            "sleep", "3600", app=app(), image=IMG, timeout=int(args.get("timeout_s", 3))
        )
        remember(sb)
        try:
            sb.exec("sh", "-c", f"sleep {int(args.get('seconds', 10))}", timeout=60).wait()
        except Exception as e:
            print("ttl_exec ended:", type(e).__name__)
    else:
        sys.exit(3)
except Exception as e:
    print(f"{name} failed: {type(e).__name__}: {e}")
    sys.exit(1)
