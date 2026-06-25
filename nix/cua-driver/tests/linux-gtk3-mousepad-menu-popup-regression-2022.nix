# GTK3/X11 regression for issue #2022.
#
# Real GTK3 app repro on X11/Openbox: keep a control xterm focused, launch
# Mousepad in the background, and ask cua-driver to click the File menubar item.
# The click call itself reports success, but current main still injects X11
# ButtonPress/ButtonRelease via XSendEvent; GTK ignores those synthetic button
# events for menu activation, so the File popup never appears. The RED
# expectation here is visible behavior: a cropped before/after screenshot around
# the menu area must show the popup appearing.
#
# Intended flake attr if wired in later:
#   checks.x86_64-linux.cua-driver-linux-gtk3-mousepad-menu-popup-regression-2022
#
# To run without editing flake.nix, evaluate/import this file directly with the
# same `pkgs` + `cuaDriverModule` wiring the flake uses.
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  openboxRc = import ./openbox-rc.nix { inherit pkgs; };

  mcpGtkMenuRegressionTest = pkgs.writeText "mcp-gtk3-menu-popup-regression-2022.py" ''
    import json
    import os
    import re
    import subprocess
    import sys
    import threading
    import time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")
    DISPLAY = os.environ.get("DISPLAY", ":99")
    MENU_CLICK_X = 36.0
    MENU_CLICK_Y = 18.0
    CROP_WIDTH = 320
    CROP_HEIGHT = 220
    MIN_CHANGED_PIXELS = 1500

    ENV = {**os.environ, "DISPLAY": DISPLAY}

    def run(argv, check=True):
        return subprocess.run(argv, check=check, text=True, capture_output=True, env=ENV)

    def read_int(path):
        with open(path, "r", encoding="utf-8") as f:
            text = f.read().strip().splitlines()[0]
        return int(text)

    def start_driver():
        proc = subprocess.Popen(
            [DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ},
        )

        def drain_stderr():
            for line in proc.stderr:
                sys.stderr.buffer.write(line)
                sys.stderr.buffer.flush()

        threading.Thread(target=drain_stderr, daemon=True).start()
        return proc

    def send(proc, method, params=None, req_id=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if req_id is not None:
            msg["id"] = req_id
        proc.stdin.write((json.dumps(msg) + "\n").encode())
        proc.stdin.flush()

    def recv(proc, timeout=30):
        result = [None]

        def reader():
            result[0] = proc.stdout.readline()

        thread = threading.Thread(target=reader)
        thread.start()
        thread.join(timeout)
        if thread.is_alive():
            raise TimeoutError("No response within timeout")
        line = result[0].decode().strip()
        if not line:
            raise RuntimeError("Driver returned an empty response")
        return json.loads(line)

    def call_tool(proc, req_id, name, arguments):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc, timeout=45)
        if "error" in resp and resp["error"] is not None:
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp

    def get_window_geometry(window_id):
        info = run(["xwininfo", "-id", str(window_id)]).stdout

        def extract(pattern):
            match = re.search(pattern, info)
            if not match:
                raise RuntimeError(f"Could not parse /xwininfo/ output for {pattern!r}:\n{info}")
            return int(match.group(1))

        return {
            "abs_x": extract(r"Absolute upper-left X:\s+(-?\d+)"),
            "abs_y": extract(r"Absolute upper-left Y:\s+(-?\d+)"),
            "width": extract(r"Width:\s+(\d+)"),
            "height": extract(r"Height:\s+(\d+)"),
        }

    def focused_window():
        return int(run(["xdotool", "getwindowfocus"]).stdout.strip())

    def capture_crop(path, abs_x, abs_y):
        run(
            [
                "import",
                "-window",
                "root",
                "-crop",
                f"{CROP_WIDTH}x{CROP_HEIGHT}+{abs_x}+{abs_y}",
                path,
            ]
        )

    def compare_changed_pixels(before_path, after_path):
        proc = run(
            ["compare", "-metric", "AE", before_path, after_path, "null:"],
            check=False,
        )
        metric_text = (proc.stderr or proc.stdout).strip()
        if not metric_text:
            raise RuntimeError(f"compare returned no metric output: rc={proc.returncode}")
        try:
            return int(float(metric_text))
        except ValueError as exc:
            raise RuntimeError(f"Unexpected compare metric output {metric_text!r}") from exc

    def response_text(resp):
        content = resp.get("result", {}).get("content", [])
        if not content:
            return json.dumps(resp)
        return "\n".join(item.get("text", "") for item in content)

    def main():
        window_id = read_int("/tmp/mousepad-xid.txt")
        pid = read_int("/tmp/mousepad-pid.txt")
        control_window_id = read_int("/tmp/control-menu-xid.txt")

        geom = get_window_geometry(window_id)
        crop_x = max(geom["abs_x"], 0)
        crop_y = max(geom["abs_y"], 0)

        before_path = "/tmp/mousepad-menu-before.png"
        after_path = "/tmp/mousepad-menu-after.png"

        focus_before = focused_window()
        capture_crop(before_path, crop_x, crop_y)

        proc = start_driver()
        try:
            send(
                proc,
                "initialize",
                {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "nixos-gtk3-menu-popup-regression-2022",
                        "version": "1.0.0",
                    },
                },
                req_id=1,
            )
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            click_resp = call_tool(
                proc,
                2,
                "click",
                {
                    "pid": pid,
                    "window_id": window_id,
                    "x": MENU_CLICK_X,
                    "y": MENU_CLICK_Y,
                },
            )
            click_text = response_text(click_resp)
            print(f"CLICK_RESPONSE: {click_text}", flush=True)
            time.sleep(1.2)
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

        focus_after = focused_window()
        capture_crop(after_path, crop_x, crop_y)
        changed_pixels = compare_changed_pixels(before_path, after_path)

        print(
            json.dumps(
                {
                    "mousepad_window_id": window_id,
                    "control_window_id": control_window_id,
                    "focus_before": focus_before,
                    "focus_after": focus_after,
                    "crop_x": crop_x,
                    "crop_y": crop_y,
                    "crop_width": CROP_WIDTH,
                    "crop_height": CROP_HEIGHT,
                    "changed_pixels": changed_pixels,
                },
                sort_keys=True,
            ),
            flush=True,
        )

        assert focus_before == control_window_id, (
            f"Precondition failed: expected control xterm to hold focus, "
            f"got {focus_before}, wanted {control_window_id}"
        )
        assert focus_after == control_window_id, (
            f"click should not steal focus from the control xterm: "
            f"focus_after={focus_after}, control={control_window_id}"
        )
        assert changed_pixels >= MIN_CHANGED_PIXELS, (
            "Expected Mousepad File menu popup to become visibly present after cua-driver click, "
            f"but screenshot delta was only {changed_pixels} pixels "
            f"(threshold {MIN_CHANGED_PIXELS}). "
            "This is the GTK/X11 send_event regression: click reported success but the UI did not react."
        )

        print("GTK3 menu popup regression reproduced cleanly", flush=True)

    if __name__ == "__main__":
        main()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-gtk3-mousepad-menu-popup-regression-2022-test";
  meta.maintainers = [ ];

  containers.machine =
    {
      pkgs,
      ...
    }:
    {
      imports = [ cuaDriverModule ];
      services.cua-driver.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xorg.xwininfo
        xterm
        openbox
        xdotool
        imagemagick
        python3
        procps
        mousepad
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11/Openbox, Mousepad, and a focused control xterm"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox --config-file ${openboxRc} >/tmp/openbox.log 2>&1 &")

        machine.execute(
            "sh -lc \"DISPLAY=:99 ${pkgs.mousepad}/bin/mousepad --disable-server "
            ">/tmp/mousepad.log 2>&1 & echo \\$! >/tmp/mousepad-pid.txt\""
        )
        machine.execute(
            "sh -lc \"DISPLAY=:99 xterm -T 'Control Menu' -fa Monospace -fs 14 "
            "-geometry 70x24+760+120 >/tmp/control-menu.log 2>&1 & "
            "echo \\$! >/tmp/control-menu-pid.txt\""
        )

        machine.wait_until_succeeds(
            "DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/mousepad-pid.txt) >/tmp/mousepad-xid.txt",
            timeout=30,
        )
        machine.wait_until_succeeds(
            "DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/control-menu-pid.txt) >/tmp/control-menu-xid.txt",
            timeout=20,
        )

        machine.succeed("DISPLAY=:99 xdotool windowsize --sync $(head -1 /tmp/mousepad-xid.txt) 1000 700")
        machine.succeed("DISPLAY=:99 xdotool windowmove $(head -1 /tmp/mousepad-xid.txt) 80 120")
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-menu-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/control-menu-xid.txt)")
        machine.log(machine.succeed("DISPLAY=:99 xdotool getwindowfocus"))

    with subtest("cua-driver click should open Mousepad File menu popup"):
        machine.copy_from_host("${mcpGtkMenuRegressionTest}", "/tmp/mcp-gtk3-menu-popup-regression-2022.py")
        status, result = machine.execute(
            "timeout 120 env DISPLAY=:99 python3 /tmp/mcp-gtk3-menu-popup-regression-2022.py 2>&1"
        )
        machine.log(result)
        assert status == 0, result
        assert "GTK3 menu popup regression reproduced cleanly" in result, result
  '';
}
