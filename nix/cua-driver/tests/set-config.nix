# CUA Driver set_config Persistence Test
#
# Regression test for #1923 (fixed in PR #1928): on Linux the `set_config`
# tool only read the legacy per-field keys, so a `{"key":..., "value":...}`
# write (the shape the Swift/macOS and Windows callers send) was silently
# dropped. This boots a NixOS container, drives the driver over MCP stdio,
# writes config via the `{key, value}` shape, then reads it back with
# `get_config` and asserts the new value persisted.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-set-config
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # Python MCP client that drives cua-driver over stdio.
  # Writes config via the {key, value} shape, reads it back, asserts.
  setConfigTest = pkgs.writeText "set-config-test.py" ''
    import subprocess
    import json
    import sys
    import os
    import threading
    import time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def main():
        print("=== CUA Driver set_config Persistence Test ===", flush=True)

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

        def call_tool(name, arguments, req_id):
            send_request("tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
            resp = read_response()
            assert resp.get("id") == req_id, f"Expected id={req_id}, got {resp.get('id')}"
            assert "result" in resp, f"Expected result in response: {resp}"
            return resp["result"]

        def get_config(req_id):
            result = call_tool("get_config", {}, req_id)
            sc = result.get("structuredContent", {})
            assert sc, f"get_config returned no structuredContent: {result}"
            return sc

        try:
            # Initialize
            send_request("initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-set-config-test", "version": "1.0.0"},
            }, req_id=1)
            resp = read_response()
            assert resp.get("id") == 1 and "result" in resp, f"initialize failed: {resp}"
            send_request("notifications/initialized", {})
            time.sleep(0.5)

            # Baseline: confirm the value we will write differs from the default
            # (max_image_dimension default is 1568), so a passing read-back can
            # only mean the write took effect — not a coincidental default.
            print("\n--- Baseline get_config ---", flush=True)
            before = get_config(2)
            print(f"before: max_image_dimension={before.get('max_image_dimension')} "
                  f"capture_mode={before.get('capture_mode')}", flush=True)
            assert before.get("max_image_dimension") != 800, \
                f"baseline already 800; cannot prove persistence: {before}"

            # The regression: write via the {key, value} shape (#1923). This is
            # the exact path that was silently dropped on Linux before #1928.
            print("\n--- set_config {key, value} ---", flush=True)
            call_tool("set_config", {"key": "max_image_dimension", "value": 800}, 3)
            call_tool("set_config", {"key": "capture_mode", "value": "ax"}, 4)

            # Read back: get_config must reflect the {key, value} writes.
            print("\n--- get_config read-back ---", flush=True)
            after = get_config(5)
            print(f"after: max_image_dimension={after.get('max_image_dimension')} "
                  f"capture_mode={after.get('capture_mode')}", flush=True)
            assert after.get("max_image_dimension") == 800, \
                f"set_config {{key,value}} did not persist max_image_dimension: {after}"
            assert after.get("capture_mode") == "ax", \
                f"set_config {{key,value}} did not persist capture_mode: {after}"

            print("\n=== set_config persistence test passed! ===", flush=True)

        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

in

pkgs.testers.nixosTest {
  name = "cua-driver-set-config-test";
  meta = {
    maintainers = [ ];
  };

  containers.machine =
    {
      config,
      pkgs,
      lib,
      ...
    }:
    {
      imports = [ cuaDriverModule ];
      services.cua-driver.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver # Xvfb for headless X11
        python3 # MCP client test script
      ];
    };

  testScript = ''
    # cache-bust 2026-06-08.1: this comment is part of the test derivation, so
    # bumping it changes the output path and forces the test to actually re-run
    # instead of being substituted from the binary cache. Bump again to re-run.
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Binary exists and runs"):
        machine.succeed("cua-driver --help")

    with subtest("Start Xvfb"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/dev/null 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)

    with subtest("set_config {key, value} persists and reads back via get_config"):
        machine.copy_from_host("${setConfigTest}", "/tmp/set-config-test.py")
        result = machine.succeed(
            "timeout 60 env DISPLAY=:99 "
            "python3 /tmp/set-config-test.py 2>&1"
        )
        machine.log(result)
        assert "set_config persistence test passed" in result, f"set_config test failed: {result}"
  '';
}
