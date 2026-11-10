#!/usr/bin/env python3
"""Load a built plugin into an owned headless Hyprland session and smoke-test input.

Runs as an unprivileged user with XDG_RUNTIME_DIR set (qualify.py arranges it).
Load: Hyprland starts on the container's DRM device, `hyprctl plugin load`
must answer ok, and `hyprctl -j cua:status` must report a matching ABI, the
input v3 state and a ready transport.

Smoke: the window-targeted background input check from the Omarchy validation
record, reduced to native Wayland clients. A holder window keeps primary focus
while background-seat KEY input over cua-input-v3 types into a separate target
window; the target application's own output is the oracle, and primary focus
must not move.

Driver smoke (when --driver names a Cua Driver release tree): the released
`cua-driver` serves the same session with native Wayland on and is asked for
the four calls an agent starts with, against a fresh native window:
list_windows finds it, get_window_state returns a screenshot, click moves the
pointer to it, and type_text reaches the application, whose own output is again
the oracle. A failure here fails the build's smoke.
"""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import time

TARGET_CLASS = "cua-requalify-target"
HOLDER_CLASS = "cua-requalify-holder"
# evdev codes: c u a Enter.
KEYS = (46, 22, 30, 28)
TYPED = "cua"
DRIVER_CLASS = "cua-requalify-driver"
DRIVER_HOLDER_CLASS = "cua-requalify-driver-holder"
DRIVER_TEXT = "driver"
CONFIG = """hl.config({
  plugin = { cua = { enabled = true } },
  input = { kb_rules = "evdev", kb_model = "pc105", kb_layout = "us",
            kb_variant = "", kb_options = "", numlock_by_default = false },
  ecosystem = { no_update_news = true, no_donation_nag = true },
})
"""


def wait_for(check, timeout, what):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError, OSError) as error:
            last = error
        time.sleep(0.25)
    raise TimeoutError(f"{what} (last error: {last})" if last else what)


class Session:
    def __init__(self, out, binary="Hyprland"):
        self.out = out
        self.binary = binary
        self.process = None
        self.instance = None
        self.clients = []
        self.config = out / "hyprland.lua"
        self.config.write_text(CONFIG)
        self.log = (out / "compositor.log").open("a")

    def ctl(self, *args, timeout=10):
        return subprocess.check_output(["hyprctl", "-i", self.instance, *args], text=True, timeout=timeout).strip()

    def json(self, *args):
        return json.loads(self.ctl("-j", *args))

    def start(self):
        env = dict(os.environ)
        env.pop("HYPRLAND_INSTANCE_SIGNATURE", None)
        env.pop("WAYLAND_DISPLAY", None)
        self.process = subprocess.Popen([self.binary, "--config", str(self.config)], env=env,
                                        stdout=self.log, stderr=subprocess.STDOUT)

        def instance():
            if self.process.poll() is not None:
                raise RuntimeError(f"Hyprland exited with {self.process.returncode}")
            listed = json.loads(subprocess.check_output(["hyprctl", "-j", "instances"], text=True, timeout=10))
            match = [i for i in listed if i["pid"] == self.process.pid]
            return match[0] if match else None

        info = wait_for(instance, 60, "Hyprland did not register an instance")
        self.instance, self.wayland = info["instance"], info["wl_socket"]
        wait_for(lambda: self.json("version"), 30, "Hyprland did not answer hyprctl")
        if not wait_for_monitors(self, 10):
            self.ctl("output", "create", "headless", "CUA-REQUALIFY-1")
            wait_for(lambda: self.json("monitors"), 20, "no monitor after creating a headless output")
        return info

    def spawn(self, app_id, *command):
        env = dict(os.environ, WAYLAND_DISPLAY=self.wayland, HYPRLAND_INSTANCE_SIGNATURE=self.instance)
        process = subprocess.Popen(["foot", f"--app-id={app_id}", "--", *command], env=env,
                                   stdout=self.log, stderr=subprocess.STDOUT)
        self.clients.append(process)

        def mapped():
            if process.poll() is not None:
                raise RuntimeError(f"{app_id} exited with {process.returncode}")
            found = [c for c in self.json("clients") if c.get("class") == app_id and c.get("mapped", True)]
            return found[0] if found else None

        return wait_for(mapped, 30, f"{app_id} window did not map")

    def stop(self):
        for process in self.clients:
            if process.poll() is None:
                process.terminate()
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()


def wait_for_monitors(session, timeout):
    try:
        return wait_for(lambda: session.json("monitors"), timeout, "no monitors")
    except TimeoutError:
        return None


def load(session, module):
    answer = session.ctl("plugin", "load", str(module))
    if answer != "ok":
        raise RuntimeError(f"plugin load answered {answer!r}")
    session.ctl("reload")
    status = wait_for(lambda: (lambda s: s if s["transport"]["ready"] else None)(session.json("cua:status")),
                      30, "cua:status transport never became ready")
    checks = {
        "abi_match": status["abi"]["match"] is True,
        "input_v3_state": status.get("state") == "input_v3_candidate",
        "transport_ready": status["transport"]["ready"] is True,
        "compositor_epoch": status.get("compositor_epoch", 0) != 0,
        "keyboard_layout_independent": status.get("keyboard_layout_independent") is True,
    }
    return status, checks


class InputLane:
    """Minimal cua-input-v3 client (protocol/cua-input-v3.md)."""

    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.socket.settimeout(10)
        self.socket.connect(str(path))
        self.sequence = 0
        self.transcript = []

    def request(self, packet):
        self.socket.send(packet.encode())
        reply = json.loads(self.socket.recv(4096).decode())
        self.transcript.append({"request": packet, "reply": reply})
        return reply

    def target(self, client, capability=2):
        return self.request(f"TARGET {client['pid']} {client['address'].removeprefix('0x')} {capability}")

    def key(self, target, key, modifiers=0):
        self.sequence += 1
        return self.request(f"KEY {self.sequence} {target['target']} {target['revision']} {key} {modifiers}")

    def close(self):
        self.socket.close()


def smoke(session):
    typed = session.out / "typed.txt"
    target = session.spawn(TARGET_CLASS, "sh", "-c", f'IFS= read -r line && printf %s "$line" > "{typed}"')
    holder = session.spawn(HOLDER_CLASS, "sleep", "600")
    focused = wait_for(lambda: (lambda a: a if a.get("address") == holder["address"] else None)(session.json("activewindow")),
                       10, "the holder window did not take primary focus")
    time.sleep(1)  # Let the target's shell reach its read.
    path = Path(os.environ["XDG_RUNTIME_DIR"]) / "hypr" / session.instance / "cua-input-v3.sock"
    lane = InputLane(path)
    checks, observations = {}, {}
    try:
        hello = lane.request("HELLO")
        checks["hello_protocol_3"] = hello.get("ok") is True and hello.get("protocol") == 3
        checks["claim"] = lane.request("CLAIM").get("ok") is True
        # Informational: the plugin refuses a target whose client holds the primary
        # seat's keyboard or pointer focus, but a headless seat has no keyboard or
        # pointer device, so Hyprland's active window need not hold either.
        refused = lane.target(holder)
        observations["focused_holder_target"] = refused
        delivered = []
        for key in KEYS:
            grant = lane.target(target)
            if not grant.get("ok"):
                raise RuntimeError(f"TARGET refused: {grant}")
            delivered.append(lane.key(grant, key))
        checks["keys_acknowledged"] = all(d.get("ok") and d.get("route") == "synthetic_events" for d in delivered)
    finally:
        lane.close()
    try:
        text = wait_for(lambda: typed.read_text() if typed.exists() else None, 10, "target wrote nothing")
    except TimeoutError:
        text = None
    checks["application_received_text"] = text == TYPED
    checks["primary_focus_unchanged"] = session.json("activewindow").get("address") == focused["address"]
    return {"target": {k: target[k] for k in ("address", "pid", "class")},
            "holder": {k: holder[k] for k in ("address", "pid", "class")},
            "typed": text, "checks": checks, "observations": observations, "transcript": lane.transcript}


class Driver:
    """The released cua-driver, serving this headless session."""

    def __init__(self, binary, session):
        self.binary = str(binary)
        self.session = session
        self.env = dict(os.environ, WAYLAND_DISPLAY=session.wayland, HYPRLAND_INSTANCE_SIGNATURE=session.instance,
                        CUA_DRIVER_RS_ENABLE_WAYLAND="1", CUA_TELEMETRY="0", DO_NOT_TRACK="1")
        self.env.pop("DISPLAY", None)
        self.log = (session.out / "driver.log").open("a")
        self.process = None
        self.calls = []

    def start(self):
        self.process = subprocess.Popen([self.binary, "serve"], env=self.env, stdout=self.log,
                                        stderr=subprocess.STDOUT)

    def call(self, tool, args=None, *extra, timeout=60):
        command = [self.binary, "call", tool, json.dumps({"session": "cua-requalify", **(args or {})}), *extra]
        done = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=timeout)
        try:
            payload = json.loads(done.stdout)
        except json.JSONDecodeError:
            payload = {"raw": done.stdout[-2000:]}
        entry = {"tool": tool, "exit": done.returncode, "stderr": done.stderr[-500:], "reply": json.dumps(payload)[:1500]}
        if not self.calls or {k: self.calls[-1][k] for k in entry} != entry:  # A polling loop logs one answer.
            self.calls.append(entry)
        return done.returncode, payload

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()


def find_window(node, pid):
    """The innermost dict anywhere in a tool reply whose pid is the window's process."""
    children = node.values() if isinstance(node, dict) else node if isinstance(node, list) else ()
    for child in children:
        found = find_window(child, pid)
        if found:
            return found
    if isinstance(node, dict) and str(node.get("pid")) == str(pid):
        return node
    return None


def png_size(path):
    data = Path(path).read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n" or len(data) < 33:
        return None
    return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big"), len(data)


def driver_smoke(session, binary):
    typed = session.out / "driver-typed.txt"
    window = session.spawn(DRIVER_CLASS, "sh", "-c", f'IFS= read -r line && printf %s "$line" > "{typed}"')
    # A newer window holds the primary seat's focus, so typing into the target is a background
    # delivery (the plugin refuses a target the primary seat is focused on); the click later
    # moves focus to the target.
    holder = session.spawn(DRIVER_HOLDER_CLASS, "sleep", "600")
    wait_for(lambda: session.json("activewindow").get("address") == holder["address"], 10,
             "the driver smoke's holder window did not take primary focus")
    time.sleep(1)  # Let the shell reach its read.
    driver = Driver(binary, session)
    checks, evidence = {}, {}
    try:
        driver.start()

        def listed():
            code, reply = driver.call("list_windows", {})
            return find_window(reply, window["pid"]) if code == 0 else None

        entry = wait_for(listed, 60, "list_windows never showed the new window")
        checks["driver_list_windows"] = True
        evidence["window"] = entry
        args = {key: entry[key] for key in ("pid", "window_id") if entry.get(key) is not None}

        shot = session.out / "driver-window.png"
        code, state = driver.call("get_window_state", args, "--screenshot-out-file", str(shot), timeout=120)
        size = png_size(shot) if shot.is_file() else None
        evidence["screenshot"] = {"exit": code, "png": size}
        checks["driver_screenshot"] = code == 0 and size is not None and size[0] > 0 and size[1] > 0 and size[2] > 1000

        code, _ = driver.call("type_text", {**args, "text": DRIVER_TEXT + "\n"})
        try:
            text = wait_for(lambda: typed.read_text() if typed.exists() else None, 10, "target wrote nothing")
        except TimeoutError:
            text = None
        evidence["typed"] = text
        checks["driver_type_text"] = code == 0 and text == DRIVER_TEXT

        x = round(window["at"][0] + window["size"][0] / 2)
        y = round(window["at"][1] + window["size"][1] / 2)
        code, clicked = driver.call("click", {"x": x, "y": y, "scope": "desktop"})
        try:
            evidence["cursor"] = wait_for(lambda: (lambda c: c if abs(c["x"] - x) <= 2 and abs(c["y"] - y) <= 2 else None)(
                session.json("cursorpos")), 10, "pointer did not reach the click point")
            checks["driver_click"] = code == 0 and "code" not in clicked
        except TimeoutError:
            evidence["cursor"] = {"wanted": [x, y], "actual": session.json("cursorpos")}
            checks["driver_click"] = False
    except Exception as error:
        # Keep every call the driver answered: they are the diagnosis.
        checks.setdefault("driver_list_windows", False)
        checks["driver_smoke"] = False
        evidence["error"] = f"{type(error).__name__}: {error}"
        evidence["driver_log_tail"] = tail(session.out / "driver.log")
    finally:
        driver.stop()
    return {"checks": checks, "evidence": evidence, "calls": driver.calls}


def tail(path, lines=60):
    try:
        return path.read_text(errors="replace").splitlines()[-lines:]
    except OSError:
        return []


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--module", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--driver", type=Path, help="a released cua-driver binary to smoke-test in the session")
    args = parser.parse_args()
    result = {"load": {"status": "fail"}, "smoke": {"status": "unavailable", "reason": "plugin did not load"}}
    session = Session(args.out)
    try:
        try:
            result["load"]["instance"] = session.start()
        except Exception as error:
            # No compositor on this runner's device is an environment limit, not a plugin failure.
            result["load"] = {"status": "unavailable", "reason": f"headless Hyprland did not start: {error}",
                              "compositor_log_tail": tail(args.out / "compositor.log")}
            result["smoke"]["reason"] = "no compositor"
            return
        try:
            status, checks = load(session, args.module)
            result["load"] = {"status": "pass" if all(checks.values()) else "fail",
                              "checks": checks, "cua_status": status}
        except Exception as error:
            result["load"] = {"status": "fail", "reason": str(error),
                              "compositor_log_tail": tail(args.out / "compositor.log")}
            return
        if result["load"]["status"] != "pass":
            return
        try:
            evidence = smoke(session)
            if args.driver:
                try:
                    driven = driver_smoke(session, args.driver)
                    evidence["checks"].update(driven["checks"])
                    evidence["driver"] = {"binary": args.driver.name, **driven}
                except Exception as error:
                    evidence["checks"]["driver_smoke"] = False
                    evidence["driver"] = {"error": f"{type(error).__name__}: {error}"}
            result["smoke"] = {"status": "pass" if all(evidence["checks"].values()) else "fail", **evidence}
        except Exception as error:
            result["smoke"] = {"status": "fail", "reason": f"{type(error).__name__}: {error}",
                               "compositor_log_tail": tail(args.out / "compositor.log")}
    finally:
        session.stop()
        (args.out / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
