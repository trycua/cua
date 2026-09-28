#!/usr/bin/env python3
"""Qualify the synthetic demo's typed SDK and foreground-service ownership."""
import argparse
import json
import pathlib
import re
import shlex
import subprocess
import time
import xml.etree.ElementTree as ET


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True)
    parser.add_argument("--driver", type=pathlib.Path, required=True)
    parser.add_argument("--evidence-dir", type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    adb = ["adb", "-s", args.device]
    package = "ai.cua.android.demo"
    records = {}

    def shell(*command):
        return subprocess.check_output(adb + ["shell", shlex.join(map(str, command))], text=True, timeout=15)

    def state():
        return json.loads(shell("content", "query", "--uri", "content://" + package + ".state").split("json=", 1)[1])

    def poll(predicate, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                result = predicate()
                if result:
                    return result
            except (KeyError, IndexError, json.JSONDecodeError):
                pass
            time.sleep(.15)
        raise AssertionError("Timed out; latest fixture state: " + json.dumps(state()))

    def remember(name):
        records[name] = state()
        (args.evidence_dir / "lifecycle.json").write_text(json.dumps(records, indent=2) + "\n")
        return records[name]

    def tap(control):
        point = state()["controls"][control]
        shell("input", "-d", "0", "tap", point["x"], point["y"])
        return state()

    def open_demo():
        shell("am", "start", "--display", "0", "-n", package + "/.MainActivity", "-a", "android.intent.action.MAIN")
        poll(lambda: state().get("controls", {}).get("start"))

    def running():
        value = state()
        return value if (value.get("status") == "Running" and value.get("preview_frames", 0) >= 3
                         and value.get("service", {}).get("owner_process_alive")) else None

    def task_removed(task_id):
        return not re.search(r"taskId=" + str(task_id) + r":", shell("am", "stack", "list"))

    def stopped(task_id):
        poll(lambda: state().get("status") == "Stopped")
        poll(lambda: task_removed(task_id))

    def notification_stop():
        shell("cmd", "statusbar", "expand-notifications")
        try:
            shell("uiautomator", "dump", "/data/local/tmp/cua-driver/notification.xml")
            tree = ET.fromstring(shell("cat", "/data/local/tmp/cua-driver/notification.xml"))
            rows = [row for row in tree.iter("node")
                    if row.get("resource-id") == "com.android.systemui:id/expandableNotificationRow"
                    and any(node.get("text") == "Cua Android session" for node in row.iter("node"))]
            assert len(rows) == 1, "Expected one Cua session notification"
            buttons = [node for node in rows[0].iter("node")
                       if node.get("text") == "Stop" and node.get("clickable") == "true"]
            assert len(buttons) == 1, "Expected the expanded notification's Stop action"
            x1, y1, x2, y2 = map(int, re.findall(r"\d+", buttons[0].get("bounds")))
            shell("input", "-d", "0", "tap", (x1 + x2) // 2, (y1 + y2) // 2)
            poll(lambda: state().get("service", {}).get("status") == "Stopped")
        finally:
            shell("cmd", "statusbar", "collapse")

    # Permission is confined to this synthetic development APK on the selected guest.
    shell("pm", "grant", package, "android.permission.POST_NOTIFICATIONS")
    open_demo()
    if state().get("service", {}).get("status") in ("Running", "Starting", "Stopping"):
        tap("stop")
        poll(lambda: state().get("status") == "Stopped")
    shell("am", "force-stop", package)
    open_demo()
    poll(lambda: state().get("status") == "Stopped")
    tap("start")
    first = poll(running)
    owner = first["service"]
    sid, task_id = owner["session_id"], owner["task_id"]
    tap("editor")
    shell("input", "-d", "0", "text", "PersistentHumanText")
    poll(lambda: state().get("text") == "PersistentHumanText")
    remember("started")

    refused = subprocess.run([str(args.driver), "--device", args.device, "--session", sid,
                              "session", "inspect"], capture_output=True, text=True, timeout=15)
    assert refused.returncode == 3 and json.loads(refused.stdout)["error"]["reason"] == "session_owner_mismatch"
    for iteration in range(3):
        before = state()
        shell("am", "start", "--activity-single-top", "-n", package + "/.MainActivity",
              "-a", "ai.cua.android.demo.RECREATE")
        poll(lambda: state().get("activity_generation") != before["activity_generation"] and running())
        after = remember("recreated_" + str(iteration))
        assert after["text"] == "PersistentHumanText", after
        assert after["controller"]["owner_generation"] == owner["owner_generation"], after
        assert after["controller"]["session_id"] == sid and after["controller"]["task_id"] == task_id, after
        assert after["controller"]["runtime_generation"] == owner["runtime_generation"], after
    print("Typed SDK session and three Activity recreations passed", flush=True)

    shell("input", "keyevent", "KEYCODE_HOME")
    background = remember("background_before")
    print("Backgrounding for 65 seconds with no host requests (longer than the runtime lease)", flush=True)
    time.sleep(35)
    print("Background interval continuing; no host requests", flush=True)
    time.sleep(30)
    after = remember("background_after")
    assert after["service"]["owner_process_alive"] and after["service"]["status"] == "Running", after
    assert after["service"]["session_id"] == sid and after["service"]["task_id"] == task_id, after
    assert after["service"]["renewals"] >= background["service"]["renewals"] + 5, after
    assert after["service"]["preview_frames"] >= background["service"]["preview_frames"] + 40, after
    open_demo()
    resumed = poll(running)
    assert resumed["text"] == "PersistentHumanText" and resumed["controller"]["session_id"] == sid, resumed
    # Repeated Start must preserve the owner and target, not create another session.
    tap("start")
    assert poll(running)["controller"]["session_id"] == sid
    screenshot = subprocess.check_output(adb + ["exec-out", "screencap", "-p"], timeout=15)
    (args.evidence_dir / "demo-resumed.png").write_bytes(screenshot)
    notification_stop()
    open_demo()
    stopped(task_id)
    remember("stopped")
    print("Background lease renewal, resume, repeated Start, and notification Stop passed", flush=True)

    tap("start")
    restarted = poll(running)
    assert restarted["service"]["session_id"] != sid
    task_id = restarted["service"]["task_id"]
    remember("before_owner_death")
    shell("am", "force-stop", package)
    dead = remember("owner_dead")
    assert not dead["service"]["owner_process_alive"], dead
    print("Checking task cleanup after controller-process death and lease expiry", flush=True)
    poll(lambda: task_removed(task_id), timeout=65)
    open_demo()
    poll(lambda: state().get("status") == "Stopped")
    assert state()["controller"]["session_id"] is None, "New process adopted a lost session"
    tap("start")
    fresh = poll(running)
    assert fresh["controller"]["owner_generation"] != owner["owner_generation"], fresh
    assert fresh["controller"]["session_id"] != restarted["controller"]["session_id"], fresh
    remember("fresh_after_owner_death")
    tap("stop")
    stopped(fresh["controller"]["task_id"])
    remember("final")
    print("Controller-death cleanup and explicit fresh start passed", flush=True)


if __name__ == "__main__":
    main()
