# Linux cursor click GIF test
#
# Records a GIF of the Linux overlay cursor moving as part of a click action
# and proves the click focused the target xterm and allowed a shell command to
# execute.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-linux-cursor-click-gif
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  mcpClickTest = pkgs.writeText "mcp-click-gif-test.py" ''
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
        with open("/tmp/target-click-xid.txt", "r", encoding="utf-8") as f:
            window_id = int(f.read().strip())
        with open("/tmp/target-click-pid.txt", "r", encoding="utf-8") as f:
            pid = int(f.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-click-gif-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            call_tool(proc, 2, "set_agent_cursor_enabled", {"enabled": True})
            call_tool(proc, 3, "move_cursor", {"x": 1100.0, "y": 900.0})
            time.sleep(0.8)
            call_tool(proc, 4, "click", {"pid": pid, "window_id": window_id, "x": 120.0, "y": 120.0})
            time.sleep(0.5)
            call_tool(proc, 5, "type_text", {
                "pid": pid,
                "window_id": window_id,
                "text": "echo click-focus > /tmp/click-focus.txt",
            })
            call_tool(proc, 6, "press_key", {"pid": pid, "window_id": window_id, "key": "enter"})
            time.sleep(1.5)
            print("click GIF test complete", flush=True)
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-cursor-click-gif-test";
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
        ffmpeg
        python3
        jq
        procps
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11 desktop and two xterms"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")
        machine.execute("sh -lc \"DISPLAY=:99 xterm -T 'Target Click' -fa Monospace -fs 14 -geometry 70x24+80+120 >/tmp/target-click.log 2>&1 & echo \\$! >/tmp/target-click-pid.txt\"")
        machine.execute("sh -lc \"DISPLAY=:99 xterm -T 'Control Click' -fa Monospace -fs 14 -geometry 70x24+700+120 >/tmp/control-click.log 2>&1 & echo \\$! >/tmp/control-click-pid.txt\"")
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/target-click-pid.txt) >/tmp/target-click-xid.txt", timeout=20)
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/control-click-pid.txt) >/tmp/control-click-xid.txt", timeout=20)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-click-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/control-click-xid.txt)")

    with subtest("Record click GIF and run driver actions"):
        machine.copy_from_host("${mcpClickTest}", "/tmp/mcp-click-gif-test.py")
        machine.execute(
            "sh -lc 'DISPLAY=:99 ffmpeg -y -video_size 1280x1024 -framerate 10 -f x11grab -i :99 "
            "-t 5 -vf \"fps=10,scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse\" "
            "/tmp/cua-driver-linux-cursor-click.gif >/tmp/ffmpeg-click.log 2>&1 & echo $! >/tmp/ffmpeg-click.pid'"
        )
        result = machine.succeed("timeout 60 env DISPLAY=:99 python3 /tmp/mcp-click-gif-test.py 2>&1")
        machine.log(result)
        assert "click GIF test complete" in result, result
        machine.wait_until_succeeds("! kill -0 $(cat /tmp/ffmpeg-click.pid) 2>/dev/null", timeout=60)
        machine.succeed("test -s /tmp/cua-driver-linux-cursor-click.gif")
        machine.wait_until_succeeds("test -f /tmp/click-focus.txt", timeout=20)

    with subtest("Verify click-focused window became active"):
        target = machine.succeed("head -1 /tmp/target-click-xid.txt").strip()
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert target == active, f"expected active window {target}, got {active}"

    with subtest("Copy GIF out of the VM"):
        machine.copy_from_machine("/tmp/cua-driver-linux-cursor-click.gif", "")
  '';
}
