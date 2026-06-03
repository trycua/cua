# Linux background terminal GIF test
#
# Records a GIF while cua-driver types into an inactive xterm window and
# executes a shell command without stealing focus.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-linux-background-terminal-gif
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  mcpBackgroundTerminalTest = pkgs.writeText "mcp-background-terminal-gif-test.py" ''
    import json
    import os
    import subprocess
    import sys
    import threading
    import time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

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

    def main():
        with open("/tmp/background-target-xid.txt", "r", encoding="utf-8") as f:
            target_window_id = int(f.read().strip())
        with open("/tmp/background-target-pid.txt", "r", encoding="utf-8") as f:
            target_pid = int(f.read().strip())
        with open("/tmp/background-control-xid.txt", "r", encoding="utf-8") as f:
            control_window_id = int(f.read().strip())
        with open("/tmp/background-control-pid.txt", "r", encoding="utf-8") as f:
            control_pid = int(f.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-background-terminal-gif-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            call_tool(proc, 2, "set_agent_cursor_enabled", {"enabled": True})
            call_tool(proc, 3, "move_cursor", {"x": 50.0, "y": 900.0})
            time.sleep(0.5)
            call_tool(proc, 4, "click", {
                "pid": control_pid,
                "window_id": control_window_id,
                "x": 120.0,
                "y": 120.0,
            })
            time.sleep(0.6)
            call_tool(proc, 5, "type_text", {
                "pid": target_pid,
                "window_id": target_window_id,
                "text": "echo hello | tee /tmp/background-hello.txt",
            })
            call_tool(proc, 6, "press_key", {
                "pid": target_pid,
                "window_id": target_window_id,
                "key": "enter",
            })
            time.sleep(1.8)
            print("background terminal GIF test complete", flush=True)
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  recordGifScript = import ./record-x11-gif.nix { inherit pkgs; };
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-background-terminal-gif-test";
  meta.maintainers = [ ];

  nodes.machine =
    {
      pkgs,
      ...
    }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = 2048;
      };
      services.cua-driver.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        openbox
        picom
        xdotool
        imagemagick
        python3
        jq
        procps
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11 desktop and target/control xterms"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")
        machine.execute("sh -lc \"DISPLAY=:99 xterm -T 'Background Target' -fa Monospace -fs 14 -geometry 70x24+40+120 >/tmp/background-target.log 2>&1 & echo \\$! >/tmp/background-target-pid.txt\"")
        machine.execute("sh -lc \"DISPLAY=:99 xterm -T 'Background Control' -fa Monospace -fs 14 -geometry 70x24+690+120 >/tmp/background-control.log 2>&1 & echo \\$! >/tmp/background-control-pid.txt\"")
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/background-target-pid.txt) >/tmp/background-target-xid.txt", timeout=20)
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/background-control-pid.txt) >/tmp/background-control-xid.txt", timeout=20)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/background-control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/background-control-xid.txt)")

    with subtest("Record GIF and execute command in inactive terminal"):
        machine.copy_from_host("${mcpBackgroundTerminalTest}", "/tmp/mcp-background-terminal-gif-test.py")
        machine.execute(
            "sh -lc '${recordGifScript} :99 /tmp/background-frames /tmp/cua-driver-linux-background-terminal.gif "
            "/tmp/stop-background-recorder /tmp/ffmpeg-background.log 10 0.15 >/dev/null 2>&1 & echo $! >/tmp/ffmpeg-background.pid'"
        )
        result = machine.succeed("timeout 60 env DISPLAY=:99 python3 /tmp/mcp-background-terminal-gif-test.py 2>&1")
        machine.log(result)
        assert "background terminal GIF test complete" in result, result
        machine.succeed("touch /tmp/stop-background-recorder")
        machine.wait_until_succeeds("! kill -0 $(cat /tmp/ffmpeg-background.pid) 2>/dev/null", timeout=60)
        machine.log(machine.succeed("sh -lc 'cat /tmp/ffmpeg-background.log || true'"))
        machine.succeed("test -s /tmp/cua-driver-linux-background-terminal.gif")
        machine.wait_until_succeeds("grep -Fx 'hello' /tmp/background-hello.txt", timeout=20)

    with subtest("Verify focus stayed on control terminal"):
        control = machine.succeed("head -1 /tmp/background-control-xid.txt").strip()
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert control == active, f"expected active window {control}, got {active}"

    with subtest("Copy GIF out of the VM"):
        machine.copy_from_machine("/tmp/cua-driver-linux-background-terminal.gif", "")
  '';
}
