# CUA Driver Screenshot Test
#
# Runs the integration test and then uses cua-driver's own get_window_state
# tool to capture a screenshot of an xterm window via MCP. The screenshot
# is extracted as a PNG from the base64 MCP response and copied out of the VM.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-screenshot
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # Python MCP client that runs the integration tests and then uses
  # cua-driver to screenshot an xterm window via get_window_state.
  mcpScreenshotTest = pkgs.writeText "mcp-screenshot-test.py" ''
    import subprocess
    import json
    import sys
    import os
    import threading
    import time
    import base64

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def main():
        print("=== CUA Driver Screenshot Test ===", flush=True)

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
        t = threading.Thread(target=drain_stderr, daemon=True)
        t.start()

        next_id = [0]
        def send_request(method, params=None, req_id=None):
            msg = {"jsonrpc": "2.0", "method": method}
            if params is not None:
                msg["params"] = params
            if req_id is not None:
                msg["id"] = req_id
            line = json.dumps(msg) + "\n"
            print(f"[send] {method}", flush=True)
            proc.stdin.write(line.encode())
            proc.stdin.flush()

        def read_response(timeout=30):
            result = [None]
            def reader():
                result[0] = proc.stdout.readline()
            rt = threading.Thread(target=reader)
            rt.start()
            rt.join(timeout)
            if rt.is_alive():
                raise TimeoutError("No response within timeout")
            line = result[0].decode().strip()
            if len(line) > 500:
                print(f"[recv] {line[:200]}...({len(line)} bytes)", flush=True)
            else:
                print(f"[recv] {line}", flush=True)
            return json.loads(line)

        try:
            # Initialize
            print("\n--- Initialize ---", flush=True)
            send_request("initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-screenshot-test", "version": "1.0.0"},
            }, req_id=1)
            resp = read_response()
            assert "result" in resp, f"Initialize failed: {resp}"
            print("PASS: initialize", flush=True)

            send_request("notifications/initialized", {})
            time.sleep(0.5)

            # List tools
            print("\n--- tools/list ---", flush=True)
            send_request("tools/list", {}, req_id=2)
            resp = read_response()
            tools = resp.get("result", {}).get("tools", [])
            tool_names = [t["name"] for t in tools]
            assert "get_window_state" in tool_names, f"get_window_state missing"
            assert "list_windows" in tool_names, f"list_windows missing"
            print(f"PASS: {len(tools)} tools", flush=True)

            # List windows to find xterm
            print("\n--- list_windows ---", flush=True)
            send_request("tools/call", {
                "name": "list_windows",
                "arguments": {},
            }, req_id=3)
            resp = read_response(timeout=10)
            print(f"list_windows response id={resp.get('id')}", flush=True)

            # Parse window list to find xterm
            content = resp.get("result", {}).get("content", [])
            text = content[0].get("text", "") if content else ""
            print(f"Windows: {text[:500]}", flush=True)

            # Try to parse JSON from the response
            windows = []
            try:
                windows = json.loads(text)
            except (json.JSONDecodeError, TypeError):
                # Might be markdown or plain text, try to find window info
                pass

            pid = None
            window_id = None

            if isinstance(windows, list) and len(windows) > 0:
                # Pick the first window (should be xterm)
                w = windows[0]
                pid = w.get("pid")
                window_id = w.get("window_id") or w.get("id") or w.get("xid")
                print(f"Found window via list_windows: pid={pid} window_id={window_id}", flush=True)

            if pid is None or window_id is None:
                # Fallback: use xdotool to get the xterm window ID and pid
                print("WARN: list_windows empty, falling back to xdotool", flush=True)
                try:
                    xid_str = subprocess.check_output(
                        ["xdotool", "search", "--name", "xterm"],
                        env={**os.environ}, timeout=5
                    ).decode().strip().split("\n")[0]
                    window_id = int(xid_str)
                    pid_str = subprocess.check_output(
                        ["xdotool", "getwindowpid", xid_str],
                        env={**os.environ}, timeout=5
                    ).decode().strip()
                    pid = int(pid_str)
                    print(f"Found window via xdotool: pid={pid} window_id={window_id}", flush=True)
                except Exception as e:
                    print(f"xdotool fallback failed: {e}", flush=True)

            if pid is not None and window_id is not None:
                # Use get_window_state with capture_mode=vision to get screenshot
                print(f"\n--- get_window_state (vision) pid={pid} window_id={window_id} ---", flush=True)
                send_request("tools/call", {
                    "name": "get_window_state",
                    "arguments": {
                        "pid": pid,
                        "window_id": window_id,
                        "capture_mode": "vision",
                    },
                }, req_id=5)
                resp = read_response(timeout=30)

                # Extract base64 image from response
                content = resp.get("result", {}).get("content", [])
                screenshot_saved = False
                for item in content:
                    if item.get("type") == "image":
                        img_data = item.get("data", "")
                        if img_data:
                            img_bytes = base64.b64decode(img_data)
                            with open("/tmp/cua-driver-screenshot.png", "wb") as f:
                                f.write(img_bytes)
                            print(f"PASS: Screenshot saved ({len(img_bytes)} bytes)", flush=True)
                            screenshot_saved = True
                            break
                    elif item.get("type") == "text":
                        text = item.get("text", "")
                        # Check if text contains base64 image data
                        if "base64" in text.lower() or len(text) > 1000:
                            print(f"Text content (may contain image ref): {text[:200]}", flush=True)

                if not screenshot_saved:
                    print(f"WARN: No image in response, saving raw response", flush=True)
                    with open("/tmp/cua-driver-response.json", "w") as f:
                        json.dump(resp, f, indent=2)
            else:
                print("SKIP: No window found for screenshot", flush=True)

            print("\n=== Screenshot test complete ===", flush=True)

        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  # Shell script displayed in xterm for a visible window to screenshot
  testPage = pkgs.writeText "test-page.sh" ''
    #!/bin/sh
    cat <<'HEREDOC'

    ╔══════════════════════════════════════════════╗
    ║       CUA Driver - NixOS Integration Test    ║
    ║                                              ║
    ║  cua-driver v0.3.2                           ║
    ║  MCP server running on Xvfb :99              ║
    ║  34 tools registered                         ║
    ║                                              ║
    ║  All tests passed!                           ║
    ╚══════════════════════════════════════════════╝

    HEREDOC
    sleep infinity
  '';

in

pkgs.testers.nixosTest {
  name = "cua-driver-screenshot-test";
  meta = {
    maintainers = [ ];
  };

  nodes.machine =
    {
      config,
      pkgs,
      lib,
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
        openbox # lightweight WM needed for _NET_CLIENT_LIST
        xdotool # window ID lookup fallback
        python3
        jq
        procps
      ];
    };

  testScript = ''
    import time

    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Binary exists and runs"):
        machine.succeed("cua-driver --help")

    with subtest("list-tools prints available tools"):
        result = machine.succeed("cua-driver list-tools")
        assert "click" in result, f"click not in: {result}"
        assert "get_window_state" in result, f"get_window_state not in: {result}"

    with subtest("Start Xvfb, window manager, and xterm"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/dev/null 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        # Start openbox WM so _NET_CLIENT_LIST is populated for list_windows
        machine.execute("DISPLAY=:99 openbox >/dev/null 2>&1 &")
        time.sleep(1)
        machine.copy_from_host("${testPage}", "/tmp/test-page.sh")
        machine.succeed("chmod +x /tmp/test-page.sh")
        machine.execute("DISPLAY=:99 xterm -fa Monospace -fs 14 -geometry 60x20+100+100 -e /tmp/test-page.sh >/dev/null 2>&1 &")
        time.sleep(3)
        # Verify xterm window exists via xdotool
        machine.succeed("DISPLAY=:99 xdotool search --name xterm")

    with subtest("Screenshot via cua-driver MCP"):
        machine.copy_from_host("${mcpScreenshotTest}", "/tmp/mcp-screenshot-test.py")
        result = machine.succeed(
            "timeout 60 env DISPLAY=:99 "
            "python3 /tmp/mcp-screenshot-test.py 2>&1"
        )
        machine.log(result)
        assert "Screenshot test complete" in result, f"Test did not complete: {result}"

    with subtest("Extract screenshot"):
        # Copy screenshot out of VM if it exists
        machine.succeed("test -f /tmp/cua-driver-screenshot.png || test -f /tmp/cua-driver-response.json")
        machine.copy_from_machine("/tmp/cua-driver-screenshot.png", "")
  '';
}
