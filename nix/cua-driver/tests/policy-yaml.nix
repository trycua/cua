# NixOS VM integration test for YAML permission-policy enforcement.
{
  pkgs,
  cuaDriver,
  ...
}:

let
  testCommand = ''
    set -euo pipefail
    cat > /tmp/policy.yaml <<'POLICY'
    allow:
      tools:
        - screenshot
      rules:
        - tool: click
          constraints:
            x: { min: 0, max: 1920 }
            y: { min: 0, max: 1080 }
    deny:
      tools:
        - shell_execute
    POLICY

    socket=/tmp/cua-driver-policy-yaml.sock
    env \
      CUA_DRIVER_POLICY_FILE=/tmp/policy.yaml \
      CUA_DRIVER_RS_TELEMETRY_ENABLED=false \
      cua-driver serve --socket "$socket" --no-permissions-gate --no-overlay \
      >/tmp/daemon.log 2>&1 &
    daemon_pid=$!
    trap 'kill "$daemon_pid" 2>/dev/null || true; if [[ -n "''${DRIVER_PID:-}" ]]; then kill "$DRIVER_PID" 2>/dev/null || true; fi' EXIT
    for _ in $(seq 1 200); do
      cua-driver status --socket "$socket" >/dev/null 2>&1 && break
      sleep 0.05
    done
    cua-driver status --socket "$socket" >/dev/null

    coproc DRIVER {
      env CUA_DRIVER_RS_TELEMETRY_ENABLED=false \
        cua-driver mcp --socket "$socket" 2>/tmp/driver.log
    }
    exec 3>&"''${DRIVER[1]}"
    exec 4<&"''${DRIVER[0]}"

    request() {
      printf '%s\n' "$1" >&3
      IFS= read -r response <&4
    }

    request '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
    jq -e '.id == 1 and .result != null' <<<"$response" >/dev/null

    request '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"screenshot","arguments":{}}}'
    jq -e '.id == 2 and .error == null and .result != null' <<<"$response" >/dev/null

    request '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shell_execute","arguments":{}}}'
    jq -e '.id == 3 and .error == null and .result.isError == true and (.result.content[0].text | startswith("Permission denied:"))' <<<"$response" >/dev/null

    request '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"click","arguments":{"x":1921,"y":100}}}'
    jq -e '.id == 4 and .error == null and .result.isError == true and (.result.content[0].text | startswith("Permission denied:"))' <<<"$response" >/dev/null
  '';
in
pkgs.testers.runNixOSTest {
  name = "cua-driver-policy-yaml";

  nodes.machine = {
    environment.systemPackages = [
      cuaDriver
      pkgs.jq
    ];
  };

  testScript = ''
    start_all()
    machine.succeed(${builtins.toJSON testCommand})
  '';
}
