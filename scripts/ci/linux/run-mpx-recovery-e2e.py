#!/usr/bin/env python3
"""Focused MPX lifecycle regression for a disposable real-Xorg/uinput desktop.

Complements scripts/ci/linux/run-rust-e2e.sh; Xvfb/TigerVNC cannot run this test.
All window interactions use Cua Driver. Signals target only harness-owned daemons.
Requires Python GTK3 bindings, xinput, and an already-built Linux Driver.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import time


def fixture(oracle):
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk, Gdk

    def event(kind, **fields):
        with open(oracle, "a") as stream:
            stream.write(json.dumps(dict(time_ns=time.time_ns(), event=kind, **fields)) + "\n")

    window = Gtk.Window(title="CUA MPX lifecycle fixture")
    window.set_default_size(700, 500)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
    window.add(box)
    entry = Gtk.Entry()
    entry.get_accessible().set_name("Keyboard recovery oracle")
    entry.connect("changed", lambda widget: event("text", value=widget.get_text()))
    box.pack_start(entry, False, False, 0)
    canvas = Gtk.DrawingArea()
    canvas.get_accessible().set_name("MPX scroll canvas")
    canvas.set_size_request(650, 400)
    canvas.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)
    canvas.connect("scroll-event", lambda widget, ev: event("scroll") or True)
    box.pack_start(canvas, True, True, 0)
    window.connect("destroy", Gtk.main_quit)
    window.show_all()
    Gtk.main()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--driver", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--source-sha", required=True)
    args = parser.parse_args()
    repository = Path(__file__).resolve().parents[3]
    head = subprocess.check_output(["git", "-C", str(repository), "rev-parse", "HEAD"], text=True).strip()
    assert head == args.source_sha, "Harness source does not match the requested candidate"
    assert sys.platform == "linux" and os.environ.get("DISPLAY")
    assert not os.environ.get("WAYLAND_DISPLAY"), "Requires a dedicated X11 desktop"
    assert os.access("/dev/uinput", os.R_OK | os.W_OK), "Requires guest-local uinput access"
    driver = str(Path(args.driver).resolve(strict=True))
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=False)
    oracle = root / "fixture.jsonl"
    daemons, handles, actors = [], [], []
    stopped = None
    target = None
    observer = None
    serial = 0
    proof = {"source_sha": args.source_sha, "cases": []}

    def save(name, value):
        (root / name).write_text(json.dumps(value, indent=2) + "\n")

    def raw(endpoint, tool, params, image=None, text_result=False):
        argv = [driver, "--socket", endpoint, "call", tool, json.dumps(params)]
        if image:
            argv += ["--screenshot-out-file", str(image)]
        result = subprocess.run(argv, capture_output=True, text=True, timeout=25)
        assert result.returncode == 0, (tool, result.stdout, result.stderr)
        if text_result:
            return result.stdout
        return json.loads(result.stdout)

    def snapshot(endpoint, label):
        value = raw(endpoint, "get_window_state", target, root / (label + ".png"))
        save(label + ".json", value)
        assert value.get("window_id") == target["window_id"], value
        assert value.get("screenshot_width", 0) >= 650 and value.get("screenshot_height", 0) >= 450

    def start():
        nonlocal serial
        serial += 1
        endpoint = str(root / ("daemon-" + str(serial) + ".sock"))
        log = (root / ("daemon-" + str(serial) + ".log")).open("w")
        handles.append(log)
        process = subprocess.Popen([driver, "serve", "--socket", endpoint,
                                    "--dangerously-bypass-approvals"],
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        daemons.append((process, endpoint))
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert process.poll() is None, "daemon exited during startup"
            try:
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.connect(endpoint)
                return process, endpoint
            except OSError:
                time.sleep(.02)
        raise AssertionError("daemon startup timed out")

    def stop(process, endpoint):
        result = subprocess.run([driver, "--socket", endpoint, "stop"],
                                capture_output=True, text=True, timeout=20)
        assert result.returncode == 0, result.stderr
        process.wait(timeout=20)

    def devices(label):
        value = subprocess.check_output(["xinput", "list", "--short"], text=True)
        (root / (label + ".txt")).write_text(value)
        return value

    def masters(value):
        return {line.split("id=")[0].strip() for line in value.splitlines()
                if "CUA " in line and "[master pointer" in line}

    def owned(value, pid):
        found = set()
        for name in masters(value):
            match = re.search(r"CUA v1\.[A-Za-z0-9_-]+\.([A-Za-z0-9_-]+)\.[A-Za-z0-9_-]+ pointer", name)
            if match and int.from_bytes(base64.urlsafe_b64decode(match[1] + "==")[:4], "big") == pid:
                found.add(name)
        return found

    def events(kind):
        if not oracle.exists():
            return []
        return [row for line in oracle.read_text().splitlines()
                if (row := json.loads(line))["event"] == kind]

    def key(endpoint, label, letter):
        snapshot(endpoint, label + "-before")
        result = raw(endpoint, "press_key", dict(target, key=letter, delivery_mode="foreground"))
        save(label + "-action.json", result)
        snapshot(endpoint, label + "-after")
        assert events("text")[-1]["value"].endswith(letter), "keyboard oracle did not change"

    def scroll(endpoint, label, amount=3):
        snapshot(endpoint, label + "-before")
        before = len(events("scroll"))
        result = raw(endpoint, "scroll", dict(target, direction="down", amount=amount, x=350, y=250))
        save(label + "-action.json", result)
        snapshot(endpoint, label + "-after")
        assert len(events("scroll")) > before, "native fixture received no scroll events"
        return result

    def long_scroll(process, endpoint, label):
        snapshot(endpoint, label + "-before")
        before = len(events("scroll"))
        log = (root / (label + "-action.log")).open("w")
        handles.append(log)
        actor = subprocess.Popen([driver, "--socket", endpoint, "call", "scroll",
                                  json.dumps(dict(target, direction="down", amount=50, x=350, y=250))],
                                 stdout=log, stderr=log)
        actors.append(actor)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and actor.poll() is None:
            value = devices(label + "-during")
            if owned(value, process.pid) and "uinput pointer" in value and len(events("scroll")) > before:
                return actor, owned(value, process.pid)
            time.sleep(.01)
        raise AssertionError("No active owned MPX device with native scroll delivery")

    try:
        devices("initial")
        primary, endpoint = start()
        observer, observer_endpoint = start()
        baseline = masters(devices("baseline-after-startup"))
        launched = raw(endpoint, "launch_app", {"name": sys.executable,
                       "additional_arguments": [str(Path(__file__).resolve()), "--fixture", str(oracle)]})
        window = next(w for w in launched["windows"] if w["title"] == "CUA MPX lifecycle fixture")
        target = {"pid": window["pid"], "window_id": window["window_id"], "session": "mpx-lifecycle"}
        snapshot(endpoint, "initial-window")
        # Prove the refusal before using this disposable test's foreground route.
        refusal = raw(endpoint, "press_key", dict(target, key="a"))
        save("background-key.json", refusal)
        snapshot(endpoint, "background-key-after")
        if refusal.get("code") == "background_unavailable":
            key(endpoint, "initial-key", "a")
        else:
            assert events("text")[-1]["value"].endswith("a")
        scroll(endpoint, "normal-scroll")
        assert not owned(devices("normal-cleanup"), primary.pid)
        stop(primary, endpoint)
        snapshot(observer_endpoint, "clean-exit")
        assert not owned(devices("clean-exit-devices"), primary.pid)
        proof["cases"].append({"case": "normal-input-and-clean-exit", "passed": True})

        for sig, label in [(signal.SIGTERM, "term"), (signal.SIGKILL, "kill")]:
            primary, endpoint = start()
            actor, live = long_scroll(primary, endpoint, label)
            primary.send_signal(sig)
            primary.wait(timeout=10)
            actor.wait(timeout=15)
            orphan = owned(devices(label + "-after-exit"), primary.pid)
            assert orphan == live, "interruption did not retain the observed orphan"
            snapshot(observer_endpoint, label + "-window-survives")
            # Read-only registry construction must not perform recovery.
            subprocess.run([driver, "describe", "scroll"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, check=True, timeout=15)
            assert owned(devices(label + "-after-describe"), primary.pid) == orphan
            dead_pid = primary.pid
            primary, endpoint = start()
            after = devices(label + "-after-restart")
            assert not owned(after, dead_pid), "restart did not recover the orphan"
            assert baseline <= masters(after), "existing unowned master was removed"
            key(endpoint, label + "-keyboard", "t" if label == "term" else "k")
            proof["cases"].append({"case": label + "-restart-recovery", "passed": True,
                                    "orphan_count": len(orphan), "legacy_preserved": True})
            stop(primary, endpoint)

        primary, endpoint = start()
        actor, live = long_scroll(observer, observer_endpoint, "live-peer")
        observer.send_signal(signal.SIGSTOP)
        stopped = observer
        stop(primary, endpoint)
        primary, endpoint = start()
        assert owned(devices("live-peer-after-restart"), observer.pid) == live
        observer.send_signal(signal.SIGCONT)
        stopped = None
        assert actor.wait(timeout=15) == 0
        snapshot(observer_endpoint, "live-peer-completed")
        assert not owned(devices("live-peer-cleanup"), observer.pid)
        key(observer_endpoint, "live-peer-keyboard", "p")
        proof["cases"].append({"case": "paused-live-peer-preserved-and-resumed", "passed": True})
        assert masters(devices("final")) == baseline
        proof["passed"] = True
    finally:
        if stopped is not None and stopped.poll() is None:
            stopped.send_signal(signal.SIGCONT)
        if target is not None and observer is not None and observer.poll() is None:
            try:
                snapshot(observer_endpoint, "fixture-before-cleanup")
                # kill_app emits human-readable text; verify the independent inventory.
                cleanup = raw(observer_endpoint, "kill_app", {"pid": target["pid"]}, text_result=True)
                save("fixture-cleanup.json", {"response": cleanup})
                remaining = raw(observer_endpoint, "list_windows", {})
                assert not any(w["pid"] == target["pid"] for w in remaining["windows"])
            except Exception as error:
                proof["cleanup_error"] = str(error)
                proof["passed"] = False
        for process, endpoint in reversed(daemons):
            if process.poll() is None:
                stop(process, endpoint)
        for actor in actors:
            if actor.poll() is None:
                actor.terminate()
                actor.wait(timeout=10)
        for handle in handles:
            handle.close()
        save("proof.json", proof)
    assert "cleanup_error" not in proof, proof
    print(json.dumps(proof), flush=True)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--fixture":
        fixture(sys.argv[2])
    else:
        main()
