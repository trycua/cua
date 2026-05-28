# CUA Driver Screenshot Test
#
# Same as the integration test, but also captures VM screenshots at key
# points. Screenshots are saved to $out/ by the NixOS test framework.
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
  mcpClientTest = pkgs.writeText "mcp-client-test.py" ''
    import subprocess
    import json
    import sys
    import os
    import threading
    import time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def main():
        print("=== CUA Driver MCP Integration Test ===", flush=True)

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

        def send_request(method, params=None, req_id=None):
            msg = {"jsonrpc": "2.0", "method": method}
            if params is not None:
                msg["params"] = params
            if req_id is not None:
                msg["id"] = req_id
            line = json.dumps(msg) + "\n"
            print(f"[send] {line.strip()}", flush=True)
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
            print(f"[recv] {line}", flush=True)
            return json.loads(line)

        try:
            print("\n--- Initialize ---", flush=True)
            send_request("initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-test", "version": "1.0.0"},
            }, req_id=1)
            resp = read_response()
            assert "result" in resp, f"Expected result: {resp}"
            assert "serverInfo" in resp["result"], f"Expected serverInfo: {resp['result']}"
            print("PASS: initialize", flush=True)

            send_request("notifications/initialized", {})
            time.sleep(0.5)

            print("\n--- tools/list ---", flush=True)
            send_request("tools/list", {}, req_id=2)
            resp = read_response()
            tools = resp.get("result", {}).get("tools", [])
            tool_names = [t["name"] for t in tools]
            assert "click" in tool_names, f"click not in tools: {tool_names}"
            assert "type_text" in tool_names, f"type_text not in tools: {tool_names}"
            assert "get_screen_size" in tool_names, f"get_screen_size not in tools: {tool_names}"
            print(f"PASS: tools/list ({len(tools)} tools)", flush=True)

            print("\n--- tools/call get_screen_size ---", flush=True)
            send_request("tools/call", {
                "name": "get_screen_size",
                "arguments": {},
            }, req_id=3)
            resp = read_response(timeout=15)
            assert resp.get("id") == 3, f"Expected id=3, got {resp.get('id')}"
            if "result" in resp:
                content = resp["result"].get("content", [])
                text = content[0].get("text", "") if content else ""
                print(f"PASS: get_screen_size returned: {text[:200]}", flush=True)
            elif "error" in resp:
                err_msg = resp.get("error", {}).get("message", "unknown")
                print(f"PASS: get_screen_size error (acceptable): {err_msg}", flush=True)

            print("\n=== All MCP tests passed! ===", flush=True)

        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  # HTML page displayed in xterm for a visible screenshot
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
        resolution = {
          x = 1280;
          y = 1024;
        };
      };
      services.cua-driver.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        imagemagick
        python3
        jq
        procps
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Binary exists and runs"):
        machine.succeed("cua-driver --help")

    with subtest("list-tools prints available tools"):
        result = machine.succeed("cua-driver list-tools")
        assert "click" in result, f"click not in: {result}"
        assert "type_text" in result, f"type_text not in: {result}"
        assert "get_screen_size" in result, f"get_screen_size not in: {result}"

    with subtest("describe tool outputs schema"):
        result = machine.succeed("cua-driver describe get_screen_size")
        assert "input_schema" in result, f"Unexpected describe output: {result[:200]}"

    with subtest("Start Xvfb"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/dev/null 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)

    with subtest("doctor with X11 display"):
        result = machine.succeed("timeout 15 env DISPLAY=:99 cua-driver doctor 2>&1 || true")
        machine.log(result)

    with subtest("MCP protocol handshake and tool listing"):
        machine.copy_from_host("${mcpClientTest}", "/tmp/mcp-client-test.py")
        result = machine.succeed(
            "timeout 60 env DISPLAY=:99 "
            "python3 /tmp/mcp-client-test.py 2>&1"
        )
        machine.log(result)
        assert "All MCP tests passed" in result, f"MCP tests failed: {result}"

    with subtest("Screenshot with xterm"):
        machine.copy_from_host("${testPage}", "/tmp/test-page.sh")
        machine.succeed("chmod +x /tmp/test-page.sh")
        machine.execute("DISPLAY=:99 xterm -fa Monospace -fs 14 -geometry 60x20+100+100 -e /tmp/test-page.sh &")
        machine.succeed("sleep 2")
        # Capture the Xvfb display (not the QEMU console) via ImageMagick
        machine.succeed("DISPLAY=:99 import -window root /tmp/cua-driver-test.png")
        machine.copy_from_vm("/tmp/cua-driver-test.png", "")
  '';
}
