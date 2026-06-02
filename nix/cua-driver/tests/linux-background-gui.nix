# Linux background GUI input test — matrix over real apps.
#
# Proves cua-driver types into a GUI window via XSendEvent WITHOUT stealing
# focus, for real toolkit/browser apps (not just terminals — this is the
# "general computer use" coverage). Each app shows a focused text field that
# mirrors whatever it receives into its X11 window title; the test types a known
# string into the *inactive* app window and asserts:
#   (a) the window title became that string  -> background input landed
#   (b) focus stayed on a separate control window -> no focus steal
#
# The `app` argument selects one entry from `apps` below, so flake.nix can wire
# one independent matrix job per app.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-linux-background-gui-<app>
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  app,
  ...
}:

let
  # Known string typed into the target. Lowercase + digits so the assertion
  # doesn't depend on the X keymap's shift levels.
  typed = "cuatyped1234";

  # An autofocused input that mirrors its value into document.title — which the
  # browser publishes as the top-level X11 window title (WM_NAME).
  htmlPage = "data:text/html,<html><body><input autofocus oninput=\"document.title=this.value\"></body></html>";

  # A native Tk app (python stdlib tkinter) whose entry mirrors into the window
  # title on each keystroke — same readback contract as the browsers, but for a
  # classic toolkit rather than a web engine.
  tkAppPy = pkgs.writeText "cua-tk-entry.py" ''
    import tkinter as tk
    root = tk.Tk()
    root.title("cua-tk-initial")
    entry = tk.Entry(root, width=40)
    entry.pack()
    entry.focus_set()
    entry.bind("<KeyRelease>", lambda _e: root.title(entry.get() or "cua-tk-initial"))
    root.geometry("400x80+700+150")
    root.mainloop()
  '';
  pythonTk = pkgs.python3.withPackages (ps: [ ps.tkinter ]);
  tkApp = pkgs.writeShellScript "cua-tk-entry.sh" ''
    exec ${pythonTk}/bin/python3 ${tkAppPy}
  '';

  apps = {
    chromium = {
      packages = [ pkgs.chromium ];
      memoryMB = 4096;
      # --new-window + occlusion/backgrounding flags so a non-focused but visible
      # window keeps processing input and title updates.
      launch = ''chromium --no-sandbox --no-first-run --no-default-browser-check --disable-gpu --disable-backgrounding-occluded-windows --disable-renderer-backgrounding --user-data-dir=/tmp/cua-chromium --window-position=700,150 --window-size=480,360 --new-window "${htmlPage}"'';
    };
    firefox = {
      packages = [ pkgs.firefox ];
      memoryMB = 4096;
      launch = ''firefox --new-instance --profile /tmp/cua-firefox --window-size=480,360 "${htmlPage}"'';
    };
    tk = {
      packages = [ pythonTk ];
      memoryMB = 2048;
      launch = "${tkApp}";
    };
  };

  selected = apps.${app};

  mcpTest = pkgs.writeText "mcp-background-gui-test.py" ''
    import json, os, sys, threading, time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")
    TYPED = "${typed}"

    def start_driver():
        import subprocess
        proc = subprocess.Popen(
            [DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={**os.environ},
        )
        def drain():
            for line in proc.stderr:
                sys.stderr.buffer.write(line); sys.stderr.buffer.flush()
        threading.Thread(target=drain, daemon=True).start()
        return proc

    def send(proc, method, params=None, req_id=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if req_id is not None:
            msg["id"] = req_id
        proc.stdin.write((json.dumps(msg) + "\n").encode()); proc.stdin.flush()

    def recv(proc, timeout=45):
        result = [None]
        def reader():
            result[0] = proc.stdout.readline()
        t = threading.Thread(target=reader); t.start(); t.join(timeout)
        if t.is_alive():
            raise TimeoutError("No response within timeout")
        line = result[0].decode().strip()
        if not line:
            raise RuntimeError("Driver returned an empty response")
        return json.loads(line)

    def call_tool(proc, req_id, name, arguments):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc)
        if resp.get("error"):
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp

    def main():
        with open("/tmp/target-xid.txt") as f:
            target_xid = int(f.read().strip())
        with open("/tmp/target-pid.txt") as f:
            target_pid = int(f.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-background-gui-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)
            # Type into the *inactive* app window. No click/focus first — this is
            # the whole point: background delivery without selecting the window.
            call_tool(proc, 2, "type_text", {
                "pid": target_pid,
                "window_id": target_xid,
                "text": TYPED,
            })
            time.sleep(1.5)
            print("background GUI test typed", flush=True)
        finally:
            proc.stdin.close(); proc.terminate(); proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-background-gui-${app}-test";
  meta.maintainers = [ ];

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = selected.memoryMB;
        diskSize = 8192;
      };
      services.cua-driver.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        openbox
        picom
        xdotool
        python3
        jq
        procps
      ] ++ selected.packages;
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11 desktop + focused control terminal"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")
        machine.execute("sh -lc \"DISPLAY=:99 xterm -T 'Control' -geometry 60x20+40+120 >/tmp/control.log 2>&1 & echo \\$! >/tmp/control-pid.txt\"")
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/control-pid.txt) >/tmp/control-xid.txt", timeout=20)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/control-xid.txt)")

    with subtest("Launch target app (${app}) in the background"):
        machine.execute("sh -lc \"DISPLAY=:99 ${selected.launch} >/tmp/target.log 2>&1 & echo \\$! >/tmp/target-pid.txt\"")
        # Wait for the app's top-level window, then keep focus on the control.
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --onlyvisible --pid $(cat /tmp/target-pid.txt) | head -1 >/tmp/target-xid.txt && test -s /tmp/target-xid.txt", timeout=90)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/control-xid.txt)")

    with subtest("Type into the inactive window via cua-driver"):
        machine.copy_from_host("${mcpTest}", "/tmp/mcp-background-gui-test.py")
        result = machine.succeed("timeout 120 env DISPLAY=:99 python3 /tmp/mcp-background-gui-test.py 2>&1")
        machine.log(result)
        assert "background GUI test typed" in result, result

    with subtest("Input landed: window title mirrors the typed string"):
        target = machine.succeed("head -1 /tmp/target-xid.txt").strip()
        machine.wait_until_succeeds(
            f"DISPLAY=:99 xdotool getwindowname {target} | grep -F '${typed}'",
            timeout=30,
        )

    with subtest("Focus stayed on the control terminal"):
        control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert control == active, f"expected active window {control}, got {active}"
  '';
}
