#!/usr/bin/env bash
# Start one native, headless Sway session and run the complete Rust E2E matrix.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RUNTIME_DIR="$(mktemp -d)"
SWAY_CONFIG="$(mktemp)"
SWAY_LOG="${REPO_ROOT}/artifacts/cua-driver/linux/sway.log"
SWAY_PID=""
DBUS_PID=""

cleanup() {
  if [[ -n "${SWAY_PID}" ]]; then
    kill "${SWAY_PID}" 2>/dev/null || true
    wait "${SWAY_PID}" 2>/dev/null || true
  fi
  if [[ -n "${DBUS_PID}" ]]; then
    kill "${DBUS_PID}" 2>/dev/null || true
  fi
  rm -rf "${RUNTIME_DIR}"
  rm -f "${SWAY_CONFIG}"
}
trap cleanup EXIT

mkdir -p "$(dirname "${SWAY_LOG}")"
chmod 700 "${RUNTIME_DIR}"
cat > "${SWAY_CONFIG}" <<'EOF'
xwayland disable
output HEADLESS-1 mode 1920x1080
seat seat0 fallback true
focus_follows_mouse no
default_border none
default_floating_border none
for_window [title="^CuaTestHarness"] floating enable, resize set 940 780, move position 0 0
for_window [title="CuaTestHarness Sentinel"] fullscreen enable
EOF

unset DISPLAY
unset WAYLAND_DISPLAY
export XDG_RUNTIME_DIR="${RUNTIME_DIR}"
export XDG_SESSION_TYPE=wayland
export XDG_CURRENT_DESKTOP=sway
export XDG_SESSION_DESKTOP=sway
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_HEADLESS_OUTPUTS=1
export CUA_DRIVER_RS_ENABLE_WAYLAND=1
export CUA_WAYLAND_RECORDING_OUTPUT=HEADLESS-1
export ELECTRON_OZONE_PLATFORM_HINT=wayland
export GDK_BACKEND=wayland
export QT_QPA_PLATFORM=wayland
export NO_AT_BRIDGE=0

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  dbus_daemon="$(command -v dbus-daemon)"
  dbus_prefix="$(dirname "$(dirname "$(readlink -f "${dbus_daemon}")")")"
  dbus_config="${dbus_prefix}/share/dbus-1/session.conf"
  if [[ ! -f "${dbus_config}" ]]; then
    echo "Nix DBus session config is missing: ${dbus_config}" >&2
    exit 1
  fi
  dbus_info="$(dbus-daemon --config-file="${dbus_config}" --fork --print-address=1 --print-pid=1)"
  export DBUS_SESSION_BUS_ADDRESS="$(sed -n '1p' <<< "${dbus_info}")"
  DBUS_PID="$(sed -n '2p' <<< "${dbus_info}")"
fi

sway --unsupported-gpu --config "${SWAY_CONFIG}" > "${SWAY_LOG}" 2>&1 &
SWAY_PID=$!

deadline=$((SECONDS + 20))
while ((SECONDS < deadline)); do
  if ! kill -0 "${SWAY_PID}" 2>/dev/null; then
    echo "Sway exited before its Wayland socket became ready" >&2
    cat "${SWAY_LOG}" >&2
    exit 1
  fi
  socket="$(find "${XDG_RUNTIME_DIR}" -maxdepth 1 -type s -name 'wayland-*' -print -quit)"
  if [[ -n "${socket}" ]]; then
    export WAYLAND_DISPLAY="$(basename "${socket}")"
    break
  fi
  sleep 0.2
done

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo "Sway did not create a Wayland socket within 20 seconds" >&2
  cat "${SWAY_LOG}" >&2
  exit 1
fi

export SWAYSOCK="$(find "${XDG_RUNTIME_DIR}" -maxdepth 1 -type s -name 'sway-ipc.*.sock' -print -quit)"
if [[ -z "${SWAYSOCK}" ]]; then
  echo "Sway did not expose its IPC socket" >&2
  cat "${SWAY_LOG}" >&2
  exit 1
fi

echo "Native Wayland E2E session: ${WAYLAND_DISPLAY}"
set +e
"${SCRIPT_DIR}/run-rust-e2e.sh" "$@"
status=$?
set -e
if [[ "${status}" != 0 ]]; then
  swaymsg -t get_tree > "${REPO_ROOT}/artifacts/cua-driver/linux/sway-tree.json" 2>/dev/null || true
fi
exit "${status}"
