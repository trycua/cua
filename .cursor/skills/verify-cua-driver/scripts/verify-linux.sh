#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$ROOT"
LANE="${1:-x11}"
shift "$(( $# > 0 ? 1 : 0 ))"
case "$LANE" in x11|wayland) ;; *) echo 'usage: verify-linux.sh x11|wayland [--no-build]' >&2; exit 2 ;; esac
mkdir -p artifacts/cua-driver/verify
export CUA_E2E_ARTIFACT_DIR
CUA_E2E_ARTIFACT_DIR="$(mktemp -d "$ROOT/artifacts/cua-driver/verify/run-XXXXXXXX")"
export CUA_E2E_INTERNAL_LANE=verify
export CUA_E2E_HARNESS_FILTER="${CUA_E2E_HARNESS_FILTER:-electron}"
export CUA_E2E_CELL_FILTER="${CUA_E2E_CELL_FILTER:-linux-electron-left-click-ax-background}"
echo "Evidence: $CUA_E2E_ARTIFACT_DIR"
set +e
if [[ "$LANE" == wayland ]]; then
  timeout 1200 scripts/ci/linux/run-rust-e2e-wayland.sh "$@"
else
  timeout 1200 xvfb-run -a --server-args='-screen 0 1920x1080x24' \
    dbus-run-session -- bash -euo pipefail -c '
      unset WAYLAND_DISPLAY
      export XDG_SESSION_TYPE=x11
      pids=()
      cleanup() {
        for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
        for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
      }
      trap cleanup EXIT
      trap "exit 143" TERM
      trap "exit 130" INT
      openbox >"$CUA_E2E_ARTIFACT_DIR/openbox.log" 2>&1 & pids+=("$!")
      picom --backend xrender --config /dev/null >"$CUA_E2E_ARTIFACT_DIR/picom.log" 2>&1 & pids+=("$!")
      timeout 15 bash -c '\''until xprop -root _NET_SUPPORTING_WM_CHECK | grep -q "window id"; do sleep .1; done'\''
      timeout 15 gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
        --method org.a11y.Bus.GetAddress >"$CUA_E2E_ARTIFACT_DIR/at-spi-address.txt"
      scripts/ci/linux/run-rust-e2e.sh "$@"
    ' verify-x11 "$@"
fi 2>&1 | tee "$CUA_E2E_ARTIFACT_DIR/launch.log"
status=${PIPESTATUS[0]}
set -e
mapfile -t owned < <(
  find "$CUA_E2E_ARTIFACT_DIR" -name trajectory.json -exec jq -r '.daemon_pid // empty' {} +
  find "$CUA_E2E_ARTIFACT_DIR" -name tool-calls.jsonl -exec jq -r '
    select(.request.params.name == "list_windows") |
    .response.result.structuredContent.windows[]? |
    select(.title | startswith("CuaTestHarness")) | .pid' {} +
)
for pid in "${owned[@]}"; do
  if kill -0 "$pid" 2>/dev/null; then
    echo "Owned PID still exists after cleanup: $pid" >&2
    status=1
  fi
done
test -r "$CUA_E2E_ARTIFACT_DIR/launch.log"
echo "Evidence retained after cleanup: $CUA_E2E_ARTIFACT_DIR"
exit "$status"
