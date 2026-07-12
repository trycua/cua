#!/usr/bin/env bash
# Start one native, headless Sway session and run the complete Rust E2E matrix.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RUNTIME_DIR="$(mktemp -d)"
SWAY_CONFIG="$(mktemp)"
SWAY_LOG="${REPO_ROOT}/artifacts/cua-driver/linux/sway.log"
ATSPI_LOG="${REPO_ROOT}/artifacts/cua-driver/linux/at-spi-bus.log"
SWAY_PID=""
DBUS_PID=""
ATSPI_PID=""

cleanup() {
  if [[ -n "${SWAY_PID}" ]]; then
    kill "${SWAY_PID}" 2>/dev/null || true
    wait "${SWAY_PID}" 2>/dev/null || true
  fi
  if [[ -n "${DBUS_PID}" ]]; then
    kill "${DBUS_PID}" 2>/dev/null || true
  fi
  if [[ -n "${ATSPI_PID}" ]]; then
    kill "${ATSPI_PID}" 2>/dev/null || true
    wait "${ATSPI_PID}" 2>/dev/null || true
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
export WLR_RENDERER=gles2
export WLR_RENDERER_ALLOW_SOFTWARE=1
export LIBGL_ALWAYS_SOFTWARE=1
export MESA_LOADER_DRIVER_OVERRIDE=llvmpipe
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_HEADLESS_OUTPUTS=1
export CUA_DRIVER_RS_ENABLE_WAYLAND=1
export CUA_WAYLAND_RECORDING_OUTPUT=HEADLESS-1
export ELECTRON_OZONE_PLATFORM_HINT=wayland
export GDK_BACKEND=wayland
export QT_QPA_PLATFORM=wayland
# The headless compositor uses Mesa software GLES so WebKit can create its EGL
# display without depending on a runner GPU or a DRM render node. DMA-BUF still
# has no useful device in this lane, so keep that transport disabled.
export WEBKIT_DISABLE_DMABUF_RENDERER=1
# This isolated CI session uses a private runtime directory and no user home
# namespaces. Modern WebKitGTK ignores WEBKIT_FORCE_SANDBOX; use its explicit
# test-only escape hatch so the WebProcess can publish its AT-SPI subtree.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export NO_AT_BRIDGE=0
export ACCESSIBILITY_ENABLED=1

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

# A private session bus does not activate the desktop accessibility stack by
# itself. Start the repo-pinned AT-SPI launcher, then require org.a11y.Bus to
# answer before any fixture or driver process inherits this session.
ATSPI_LAUNCHER="${CUA_AT_SPI_BUS_LAUNCHER:-$(command -v at-spi-bus-launcher || true)}"
if [[ ! -x "${ATSPI_LAUNCHER}" ]]; then
  echo "AT-SPI bus launcher is unavailable: ${ATSPI_LAUNCHER:-<not found>}" >&2
  exit 1
fi
"${ATSPI_LAUNCHER}" --launch-immediately > "${ATSPI_LOG}" 2>&1 &
ATSPI_PID=$!
deadline=$((SECONDS + 15))
while ((SECONDS < deadline)); do
  if ! kill -0 "${ATSPI_PID}" 2>/dev/null; then
    echo "AT-SPI bus launcher exited before org.a11y.Bus became ready" >&2
    cat "${ATSPI_LOG}" >&2
    exit 1
  fi
  if gdbus call --session \
      --dest org.a11y.Bus \
      --object-path /org/a11y/bus \
      --method org.a11y.Bus.GetAddress >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
if ! gdbus call --session \
    --dest org.a11y.Bus \
    --object-path /org/a11y/bus \
    --method org.a11y.Bus.GetAddress >/dev/null 2>&1; then
  echo "org.a11y.Bus did not become ready within 15 seconds" >&2
  cat "${ATSPI_LOG}" >&2
  exit 1
fi

# The bus launcher and the registry daemon are separate services. Resolve the
# private accessibility bus and require the registry to activate on it before
# any toolkit inherits this session.
a11y_address="$(
  gdbus call --session \
    --dest org.a11y.Bus \
    --object-path /org/a11y/bus \
    --method org.a11y.Bus.GetAddress \
    | sed -e "s/^('//" -e "s/',)$//"
)"
export AT_SPI_BUS_ADDRESS="${a11y_address}"
if [[ -z "${a11y_address}" ]] || ! gdbus call \
    --address "${a11y_address}" \
    --dest org.a11y.atspi.Registry \
    --object-path /org/a11y/atspi/accessible/root \
    --method org.freedesktop.DBus.Properties.Get \
    org.a11y.atspi.Accessible ChildCount >/dev/null 2>&1; then
  echo "AT-SPI registry did not activate on the accessibility bus" >&2
  cat "${ATSPI_LOG}" >&2
  exit 1
fi
if ! gdbus call --session \
    --dest org.a11y.Bus \
    --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Set \
    org.a11y.Status IsEnabled '<true>' >/dev/null 2>&1; then
  echo "Could not enable accessibility on org.a11y.Bus" >&2
  cat "${ATSPI_LOG}" >&2
  exit 1
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
