# NixOS VM integration test for embedded Rego permission enforcement.
{
  pkgs,
  cuaDriver,
  ...
}:

let
  testCommand = ''
    set -euo pipefail
    mkdir -p /tmp/policy
    cat > /tmp/policy/base.rego <<'POLICY'
    package cua.policy

    default allow = false
    POLICY
    cat > /tmp/policy/tools.rego <<'POLICY'
    package cua.policy

    allow if input.tool == "screenshot"

    allow if {
      input.tool == "click"
      input.arguments.x >= 0
      input.arguments.x <= 1920
      input.arguments.y >= 0
      input.arguments.y <= 1080
    }
    POLICY

    socket=/tmp/cua-driver-policy-rego.sock
    env \
      CUA_DRIVER_POLICY_FILE=/tmp/policy \
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

    request '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"click","arguments":{"x":2000,"y":100}}}'
    jq -e '.id == 4 and .error == null and .result.isError == true and (.result.content[0].text | startswith("Permission denied:"))' <<<"$response" >/dev/null
  '';
in
pkgs.testers.runNixOSTest {
  name = "cua-driver-policy-rego";

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
