#!/usr/bin/env python3
"""Fixture-only Android qualification. ADB input is synthetic, not human IME proof."""
import argparse
import base64
import concurrent.futures
import copy
import json
import pathlib
import re
import shlex
import subprocess
import threading
import time
import uuid


def verify_concurrency(main, target, display, human):
    assert main["text"] == human, main
    assert main["display_id"] == 0 and main["window_focus"] and main["editor_focus"], main
    assert target["counter"] == 5 and target["text"] == "", target
    touches = [e for e in target["events"] if e["kind"] == "touch"]
    assert len(touches) == 10, "Missing touch receipt"
    assert all(e["receiver_display_id"] == display for e in touches)
    ht = [e["time_ms"] for e in main["events"] if e["kind"] == "text"]
    at = [e["time_ms"] for e in touches]
    assert ht and max(min(ht), min(at)) <= min(max(ht), max(at)), "Input intervals did not overlap"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--device", required=True)
    p.add_argument("--driver", type=pathlib.Path, required=True)
    p.add_argument("--evidence-dir", type=pathlib.Path, required=True)
    p.add_argument("--runs", type=int, default=5)
    args = p.parse_args()
    assert args.runs > 0
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    adb = ["adb", "-s", args.device]
    records = []

    def shell(*command, check=True):
        return subprocess.run(adb + ["shell", shlex.join(map(str, command))], check=check,
                              capture_output=True, text=True, timeout=15)

    def state(package):
        output = shell("content", "query", "--uri", "content://" + package + ".state").stdout
        return json.loads(output.split("json=", 1)[1])

    def poll(predicate, timeout=8):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                value = predicate()
                if value:
                    return value
            except (KeyError, IndexError, json.JSONDecodeError):
                pass
            time.sleep(.1)
        raise AssertionError("State predicate did not become true")

    def call(*command, expected=0):
        r = subprocess.run([str(args.driver), "--device", args.device, *map(str, command)],
                           capture_output=True, text=True, timeout=15)
        result = json.loads(r.stdout)
        assert r.returncode == expected, result
        assert result["exit_code"] == expected, result
        if "image_base64" in result.get("data", {}):
            result["data"].pop("image_base64")
        records.append(result)
        return result

    def send_raw(req):
        payload = base64.b64encode(json.dumps(req).encode()).decode()
        return shell("env", "CLASSPATH=/data/local/tmp/cua-driver/runtime.apk",
                     "app_process", "/", "ai.cua.driver.ClientMain", payload, check=False)

    def raw(operation, sid=None, params=None, request_id=None):
        req = dict(contract_version="cua.android.v0", request_id=request_id or str(uuid.uuid4()),
                   operation=operation, params=params if params is not None else {})
        if sid:
            req["session_id"] = sid
        return send_raw(req), req

    def local(*command, expected=0):
        r = shell("/data/local/tmp/cua-driver/cua-driver", "--local", *command, check=False)
        result = json.loads(r.stdout)
        assert r.returncode == expected and result["exit_code"] == expected, result
        result.get("data", {}).pop("image_base64", None)
        records.append(result)
        return result

    def task_removed(task_id):
        return not re.search(r"taskId=" + str(task_id) + r":", shell("am", "stack", "list").stdout)

    def tap_main(control):
        point = state("ai.cua.android.demo")["controls"][control]
        shell("input", "-d", "0", "tap", point["x"], point["y"])

    call("doctor")
    assert local("doctor")["data"]["backend"] == "android"
    cases = pathlib.Path(__file__).resolve().parents[1] / "contract/invalid-requests.json"
    for case in json.loads(cases.read_text()):
        rejected = send_raw(case["request"])
        assert rejected.returncode == 2, (case["name"], rejected.stdout)
    malformed, _ = raw("snapshot", "bad", {"target_id": 1})
    assert malformed.returncode == 2, malformed.stdout
    unknown, _ = raw("session.create", params={"allowed_apps": ["ai.cua.fixture.notes"], "display_id": 0})
    assert unknown.returncode == 2, unknown.stdout

    for run in range(args.runs):
        shell("am", "force-stop", "ai.cua.android.demo")
        shell("am", "start", "--display", "0", "-n", "ai.cua.android.demo/.MainActivity")
        poll(lambda: state("ai.cua.android.demo").get("controls", {}).get("editor"))
        tap_main("editor")
        created = call("session", "create", "--allow-app", "ai.cua.fixture.notes")
        sid = created["data"]["session_id"]
        display = created["data"]["display_id"]
        try:
            call("--session", sid, "app", "launch", "--package", "com.android.settings", expected=3)
            launched = call("--session", sid, "app", "launch", "--package", "ai.cua.fixture.notes")["data"]
            target = launched["target_id"]
            poll(lambda: state("ai.cua.fixture.notes").get("display_id") == display
                 and state("ai.cua.fixture.notes").get("controls", {}).get("increment"))
            barrier = threading.Barrier(2)
            human = "HumanTypingRun" + str(run)

            def type_human():
                barrier.wait()
                for char in human:
                    shell("input", "-d", "0", "text", char)
                    time.sleep(.15)

            def agent():
                barrier.wait()
                for i in range(5):
                    snap = call("--session", sid, "snapshot", "--target", target)["data"]["snapshot_id"]
                    point = state("ai.cua.fixture.notes")["controls"]["increment"]
                    call("--session", sid, "tap", "--snapshot", snap, "--x", point["x"], "--y", point["y"])
                    call("--session", sid, "tap", "--snapshot", snap, "--x", point["x"], "--y", point["y"], expected=3)

            with concurrent.futures.ThreadPoolExecutor(2) as pool:
                h = pool.submit(type_human); a = pool.submit(agent)
                h.result(); a.result()
            poll(lambda: state("ai.cua.fixture.notes").get("counter") == 5)
            main = state("ai.cua.android.demo"); target_state = state("ai.cua.fixture.notes")
            (args.evidence_dir / ("state-" + str(run) + ".json")).write_text(json.dumps(dict(main=main, target=target_state), indent=2))
            verify_concurrency(main, target_state, display, human)
            assert target_state["authorization_probe"] == "denied", target_state
            # Verify that the oracle itself detects corrupted/missing event evidence.
            for corrupt in ("wrong_display", "missing_event"):
                negative = copy.deepcopy(target_state)
                touch = next(e for e in negative["events"] if e["kind"] == "touch")
                if corrupt == "wrong_display":
                    touch["receiver_display_id"] = 0
                else:
                    negative["events"].remove(touch)
                try:
                    verify_concurrency(main, negative, display, human)
                except AssertionError:
                    pass
                else:
                    raise AssertionError("Oracle accepted " + corrupt)
            image = args.evidence_dir / ("target-" + str(run) + ".png")
            call("--session", sid, "snapshot", "--target", target, "--image", image)
        finally:
            call("--session", sid, "session", "stop")
        call("--session", sid, "session", "stop")
        poll(lambda: task_removed(launched["task_id"]))
        call("--session", sid, "snapshot", "--target", target, expected=3)
        print("Concurrent synthetic fixture run", run + 1, "passed", flush=True)

    # Exercise explicit phone-local geometry, swipe, PNG output and mutation deduplication.
    sid = local("session", "create", "--size", "1080x1920", "--density", "320",
                "--allow-app", "ai.cua.fixture.notes")["data"]["session_id"]
    try:
        target = local("--session", sid, "app", "launch", "ai.cua.fixture.notes")["data"]["target_id"]
        poll(lambda: state("ai.cua.fixture.notes").get("counter") == 0)
        remote_png = "/data/local/tmp/cua-driver/smoke-" + str(uuid.uuid4()) + ".png"
        snap = local("--session", sid, "snapshot", "--target", target, "--image", remote_png)["data"]["snapshot_id"]
        png = subprocess.run(adb + ["exec-out", "cat", remote_png], capture_output=True, check=True, timeout=15).stdout
        assert png[:8] == b"\x89PNG\r\n\x1a\n"
        (args.evidence_dir / "phone-local.png").write_bytes(png)
        local("--session", sid, "snapshot", "--target", target, "--image", remote_png, expected=4)
        snap = local("--session", sid, "snapshot", "--target", target)["data"]["snapshot_id"]
        # Stationary swipe exercises the duration grammar and DOWN/MOVE/UP path.
        point = state("ai.cua.fixture.notes")["controls"]["increment"]
        local("--session", sid, "gesture", "swipe", "--snapshot", snap,
              "--from-x", str(point["x"]), "--from-y", str(point["y"]),
              "--to-x", str(point["x"]), "--to-y", str(point["y"]), "--duration-ms", "100")
        poll(lambda: state("ai.cua.fixture.notes").get("counter") == 1)
        snap = local("--session", sid, "snapshot", "--target", target)["data"]["snapshot_id"]
        result, request = raw("tap", sid, {"snapshot_id": snap, "x": point["x"], "y": point["y"]})
        assert result.returncode == 0, result.stdout
        duplicate = send_raw(request)
        assert duplicate.returncode == 0 and json.loads(duplicate.stdout) == json.loads(result.stdout)
        poll(lambda: state("ai.cua.fixture.notes").get("counter") == 2)
        request["params"]["x"] += 1
        assert send_raw(request).returncode == 2
        assert state("ai.cua.fixture.notes")["counter"] == 2
        time.sleep(6)
        stale = local("--session", sid, "snapshot", "--target", target, expected=3)
        assert stale["error"]["reason"] == "frame_stale", stale
    finally:
        local("--session", sid, "session", "stop")
    print("Phone-local geometry/swipe/PNG and duplicate-request checks passed", flush=True)

    # The app creates and renews its own session; no host client owns its lease.
    shell("am", "force-stop", "ai.cua.android.demo")
    shell("am", "start", "--display", "0", "-n", "ai.cua.android.demo/.MainActivity")
    poll(lambda: state("ai.cua.android.demo").get("status") == "Stopped")
    tap_main("start")
    poll(lambda: state("ai.cua.android.demo").get("preview_frames", 0) >= 2)
    first = state("ai.cua.android.demo")["preview_frames"]
    # Do not stop the shared host ADB server; absence of host requests proves
    # this bounded SDK interval without disconnecting unrelated devices.
    time.sleep(12)
    second = state("ai.cua.android.demo")
    assert second["preview_frames"] >= first + 5 and second["status"] == "Running", second
    tap_main("stop")
    poll(lambda: state("ai.cua.android.demo").get("status") == "Stopped")
    (args.evidence_dir / "sdk-demo.json").write_text(json.dumps(second, indent=2))
    # Expiry must destroy the target task instead of relocating it to display 0.
    created = call("session", "create", "--allow-app", "ai.cua.fixture.notes")["data"]
    sid = created["session_id"]
    launched = call("--session", sid, "app", "launch", "ai.cua.fixture.notes")["data"]
    print("Checking the unrenewed 60-second session lease", flush=True)
    poll(lambda: task_removed(launched["task_id"]), timeout=65)
    call("--session", sid, "session", "inspect", expected=3)
    call("--session", sid, "session", "stop")
    (args.evidence_dir / "results.json").write_text(json.dumps(records, indent=2))
    print("Local CLI, repeated display/input/cleanup, and autonomous SDK preview checks passed.")


if __name__ == "__main__":
    main()
