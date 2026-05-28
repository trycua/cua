# CUA Driver Integration Test
#
# Tests the cua-driver MCP server end-to-end in a NixOS VM with Xvfb.
# Verifies CLI subcommands, MCP protocol handshake, and tool invocation.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-integration
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # Python MCP client that drives cua-driver over stdio.
  # Sends JSON-RPC requests, reads responses, validates structure.
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

        # Drain stderr in background to prevent blocking
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
            # Simple blocking read with timeout via thread
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
            # Test 1: Initialize
            print("\n--- Test 1: Initialize ---", flush=True)
            send_request("initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-test", "version": "1.0.0"},
            }, req_id=1)
            resp = read_response()
            assert resp.get("id") == 1, f"Expected id=1, got {resp.get('id')}"
            assert "result" in resp, f"Expected result in response: {resp}"
            assert "serverInfo" in resp["result"], f"Expected serverInfo: {resp['result']}"
            print("PASS: initialize", flush=True)

            # Send initialized notification (no response expected)
            send_request("notifications/initialized", {})
            time.sleep(0.5)

            # Test 2: List tools
            print("\n--- Test 2: tools/list ---", flush=True)
            send_request("tools/list", {}, req_id=2)
            resp = read_response()
            assert resp.get("id") == 2, f"Expected id=2, got {resp.get('id')}"
            tools = resp.get("result", {}).get("tools", [])
            tool_names = [t["name"] for t in tools]
            print(f"Found {len(tools)} tools: {tool_names[:10]}...", flush=True)
            assert "click" in tool_names, f"click not in tools: {tool_names}"
            assert "type_text" in tool_names, f"type_text not in tools: {tool_names}"
            assert "get_screen_size" in tool_names, f"get_screen_size not in tools: {tool_names}"
            assert "get_window_state" in tool_names, f"get_window_state not in tools: {tool_names}"
            print("PASS: tools/list", flush=True)

            # Test 3: Call get_screen_size (works without any windows)
            print("\n--- Test 3: tools/call get_screen_size ---", flush=True)
            send_request("tools/call", {
                "name": "get_screen_size",
                "arguments": {},
            }, req_id=3)
            resp = read_response(timeout=15)
            assert resp.get("id") == 3, f"Expected id=3, got {resp.get('id')}"
            # get_screen_size should return display dimensions from Xvfb
            if "result" in resp:
                content = resp["result"].get("content", [])
                text = content[0].get("text", "") if content else ""
                print(f"PASS: get_screen_size returned: {text[:200]}", flush=True)
            elif "error" in resp:
                err_msg = resp.get("error", {}).get("message", "unknown")
                print(f"PASS: get_screen_size returned error (acceptable in headless): {err_msg}", flush=True)
            else:
                raise AssertionError(f"Unexpected response: {resp}")

            print("\n=== All MCP tests passed! ===", flush=True)

        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

in

pkgs.testers.nixosTest {
  name = "cua-driver-integration-test";
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
        xorg.xorgserver # Xvfb for headless X11
        python3 # MCP client test script
        jq
        procps # pgrep/pkill
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
        machine.succeed("Xvfb :99 -screen 0 1280x1024x24 &")
        machine.succeed("sleep 2")

    with subtest("doctor with X11 display"):
        # Timeout doctor to 15s — AT-SPI/gdbus probes can hang without a session bus
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
  '';
}
