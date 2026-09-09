#!/usr/bin/env python3
"""Record/verify the device-owned visual agent; host actions only operate its controller.

The model relay must already be running. No target-app actions or oracle data
are sent by this harness. Main-display typing is explicitly synthetic ADB input.
"""
import argparse
import json
import pathlib
import re
import shlex
import subprocess
import time
import uuid


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--device", required=True)
    p.add_argument("--token-file", type=pathlib.Path, required=True)
    p.add_argument("--task-file", type=pathlib.Path, required=True)
    p.add_argument("--app", action="append", required=True)
    p.add_argument("--evidence-dir", type=pathlib.Path, required=True)
    p.add_argument("--record", action="store_true")
    p.add_argument("--stop-during-inference", action="store_true")
    p.add_argument("--background-seconds", type=int, default=0)
    p.add_argument("--timeout", type=int, default=170)
    args = p.parse_args()
    assert 15 <= args.timeout <= 170 and 0 <= args.background_seconds <= 30
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    adb = ["adb", "-s", args.device]
    events, owned_tasks, visited_apps = [], set(), set()
    started = time.monotonic()
    recording = None

    def shell(*cmd):
        return subprocess.check_output(adb + ["shell", shlex.join(map(str, cmd))], text=True, timeout=15)

    def state():
        return json.loads(shell("content", "query", "--uri", "content://ai.cua.android.demo.state").split("json=", 1)[1])

    def save(label):
        s = state()
        service = s.get("service", {})
        if service.get("task_id"):
            owned_tasks.add(service["task_id"])
        if service.get("status") == "Running" and service.get("current_package"):
            visited_apps.add(service["current_package"])
        events.append(dict(event=label, t=round(time.monotonic() - started, 3),
                           epoch_s=time.time(), state=s))
        (args.evidence_dir / "events.json").write_text(json.dumps(events, indent=2))
        return s

    def tap(control):
        s = save("before_" + control)
        point = s["controls"][control]
        shell("input", "-d", "0", "tap", point["x"], point["y"])
        return save("after_" + control)

    def poll(predicate, timeout=12):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            s = state()
            if predicate(s):
                return s
            time.sleep(.1)
        raise AssertionError("Controller condition did not become true")

    prior = state()
    service = prior.get("service", {})
    assert not service.get("owner_process_alive") or service.get("status") in ("Stopped", "Blocked") or service.get("status", "").startswith("Error"), "Stop the current controller before this run"
    # Starting a fresh controller does not reset the real target apps' data.
    shell("am", "force-stop", "ai.cua.android.demo")
    config = dict(endpoint="http://127.0.0.1:8788/decide", token=args.token_file.read_text().strip(),
                  task=args.task_file.read_text(), allowed_apps=args.app)
    subprocess.run(adb + ["shell", shlex.join(["run-as", "ai.cua.android.demo", "sh", "-c", "cat > files/agent-config.json"])],
                   input=json.dumps(config), text=True, check=True)
    subprocess.run(adb + ["reverse", "tcp:8788", "tcp:8788"], check=True, capture_output=True)
    shell("am", "start", "--display", "0", "-n", "ai.cua.android.demo/.MainActivity")
    poll(lambda s: s.get("activity_generation") != prior.get("activity_generation") and s.get("window_focus") and "run_agent" in s.get("controls", {}))
    remote = "/data/local/tmp/cua-driver/agent-" + uuid.uuid4().hex + ".mp4"
    if args.record:
        cmd = "echo $$; exec screenrecord --time-limit 180 --bit-rate 8000000 " + shlex.quote(remote)
        recording = subprocess.Popen(adb + ["shell", shlex.join(["sh", "-c", cmd])], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        pid = recording.stdout.readline().strip()
        assert pid.isdigit()
    started = time.monotonic()
    completed = False
    try:
        save("ready")
        time.sleep(2)
        tap("run_agent")
        running = poll(lambda s: s.get("service", {}).get("phase") == "inference")
        sid = running["service"]["session_id"]
        display = running["service"]["display_id"]
        owner = running["service"]["owner_generation"]
        assert display != 0
        save("inference_started")
        if args.stop_during_inference:
            tap("stop")
            poll(lambda s: s.get("service", {}).get("status") == "Stopped", timeout=8)
        else:
            tap("editor")
            poll(lambda s: s.get("editor_focus") and s.get("window_focus"))
            for char in "I can keep typing":
                save("before_text")
                shell("input", "-d", "0", "text", "%s" if char == " " else char)
                save("after_text")
                time.sleep(.08)
            assert state()["text"] == "I can keep typing"
            # Close the keyboard opened by the controller's editor, preserving
            # the whole preview in the recording. This is display-0 UI input.
            save("before_keyboard_hide")
            (args.evidence_dir / "main-typing.png").write_bytes(subprocess.check_output(adb + ["exec-out", "screencap", "-p"]))
            shell("input", "-d", "0", "keyevent", "KEYCODE_BACK")
            poll(lambda s: s.get("window_focus"))
            save("after_keyboard_hide")
            background_at = None
            background_done = False
            last_step = None
            while time.monotonic() - started < args.timeout:
                s = save("poll")
                c = s["service"]
                assert c["session_id"] == sid and c["owner_generation"] == owner
                assert s["text"] == "I can keep typing" and s["display_id"] == 0
                if background_at is None:
                    assert s["window_focus"], "Controller lost main-display focus"
                if c["agent_steps"] != last_step:
                    print(f"step={c['agent_steps']} phase={c['phase']} app={c['current_package']} renewals={c['renewals']}", flush=True)
                    last_step = c["agent_steps"]
                if args.background_seconds and not background_done and c["status"] == "Running" and c["agent_steps"] >= 2:
                    save("before_home")
                    shell("input", "-d", "0", "keyevent", "KEYCODE_HOME")
                    save("after_home")
                    background_at = time.monotonic()
                    background_done = True
                if background_at is not None and (time.monotonic() - background_at >= args.background_seconds or c["status"] != "Running"):
                    save("before_resume")
                    shell("am", "start", "--display", "0", "-n", "ai.cua.android.demo/.MainActivity")
                    poll(lambda value: value.get("window_focus"))
                    save("after_resume")
                    background_at = None
                if c["status"] != "Running":
                    if c["phase"] in ("cleanup", "stopping"):
                        time.sleep(.2)
                        continue
                    assert c["status"] == "Stopped" and c["phase"] == "done", c
                    assert c["model_status"] == "done" and c["preview_label"] == "Last frame · workspace closed"
                    completed = True
                    break
                time.sleep(.5)
            assert completed, "Agent did not complete within recording window"
            assert set(args.app) <= visited_apps, "Model declared done before visiting every requested app"
            (args.evidence_dir / "result.png").write_bytes(subprocess.check_output(adb + ["exec-out", "run-as", "ai.cua.android.demo", "cat", "files/agent-final.png"]))
        final = save("stopped")
        stack = shell("am", "stack", "list")
        assert all(not re.search(r"taskId=" + str(task) + r":", stack) for task in owned_tasks)
        assert not re.search(r"displayId=" + str(display) + r"\b", stack)
        (args.evidence_dir / "tasks-stopped.txt").write_text(stack)
        (args.evidence_dir / "main-final.png").write_bytes(subprocess.check_output(adb + ["exec-out", "screencap", "-p"]))
        time.sleep(3)
    finally:
        try:
            if state().get("service", {}).get("status") in ("Running", "Starting", "Stopping"):
                tap("stop")
                poll(lambda s: s.get("service", {}).get("status") == "Stopped")
        finally:
            if recording:
                if recording.poll() is None:
                    cmdline = shell("cat", "/proc/" + pid + "/cmdline")
                    assert remote in cmdline, "Recording PID identity changed"
                    shell("kill", "-2", pid)
                recording.communicate(timeout=15)
                subprocess.run(adb + ["pull", remote, str(args.evidence_dir / "raw.mp4")], check=True, timeout=30)
    print("PASS: model declared done, requested apps visited, main input preserved, workspace cleanup; inspect result.png to verify the task outcome" if completed else "PASS: Stop during inference cleaned the workspace")


if __name__ == "__main__":
    main()
