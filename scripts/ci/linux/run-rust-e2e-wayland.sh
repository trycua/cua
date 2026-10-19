#!/usr/bin/env bash
# Start one native, headless Wayland session and run the complete Rust E2E
# matrix. CUA_E2E_WAYLAND_SESSION selects sway (default), sway-xwayland,
# cua-compositor, or niri. niri has no headless backend, so the niri session
# runs niri's winit backend inside a headless cage.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RUNTIME_DIR="$(mktemp -d)"
SWAY_CONFIG="$(mktemp)"
NIRI_CONFIG="$(mktemp)"
SESSION_KIND="${CUA_E2E_WAYLAND_SESSION:-sway}"
export CUA_E2E_WAYLAND_SESSION="${SESSION_KIND}"
if [[ "${SESSION_KIND}" == sway-xwayland ]]; then
  # The testkit's compositor contract is still canonical Sway. Keep the
  # orchestration label separate so focus, z-order, and sentinel observers use
  # their established Sway paths while this wrapper additionally enables XWayland.
  export CUA_E2E_WAYLAND_SESSION=sway
fi
COMPOSITOR_LOG="${REPO_ROOT}/artifacts/cua-driver/linux/${SESSION_KIND}.log"
ATSPI_LOG="${REPO_ROOT}/artifacts/cua-driver/linux/at-spi-bus.log"
# cage serves only niri's winit window. Its private runtime directory keeps its
# Wayland socket out of the directory the tests search and inherit.
CAGE_RUNTIME_DIR="${RUNTIME_DIR}/cage"
CAGE_LOG="${REPO_ROOT}/artifacts/cua-driver/linux/cage.log"
CAGE_PID=""
COMPOSITOR_PID=""
DBUS_PID=""
ATSPI_PID=""
KEYRING_PID=""

# SIGTERM, then SIGKILL after a 5 second grace, so a wedged or stopped
# process cannot hold cleanup. Children of this shell are also reaped.
stop_process() {
  local pid="$1"
  kill -TERM "${pid}" 2>/dev/null || return 0
  local deadline=$((SECONDS + 5))
  while kill -0 "${pid}" 2>/dev/null \
      && [[ "$(ps -o stat= -p "${pid}" 2>/dev/null)" != Z* ]] \
      && ((SECONDS < deadline)); do
    sleep 0.1
  done
  kill -KILL "${pid}" 2>/dev/null || true
  wait "${pid}" 2>/dev/null || true
}

cleanup() {
  if [[ -n "${COMPOSITOR_PID}" ]]; then
    stop_process "${COMPOSITOR_PID}"
  fi
  if [[ -n "${CAGE_PID}" ]]; then
    # cage exits once its client does, and a SIGTERM exit removes the X11
    # socket and lock it binds in /tmp.
    pkill -TERM -P "${CAGE_PID}" 2>/dev/null || true
    stop_process "${CAGE_PID}"
  fi
  if [[ -n "${KEYRING_PID}" ]]; then
    stop_process "${KEYRING_PID}"
  fi
  if [[ -n "${DBUS_PID}" ]]; then
    stop_process "${DBUS_PID}"
  fi
  if [[ -n "${ATSPI_PID}" ]]; then
    stop_process "${ATSPI_PID}"
  fi
  rm -f "${SWAY_CONFIG}" "${NIRI_CONFIG}"
  rm -rf "${RUNTIME_DIR}"
}
trap cleanup EXIT

mkdir -p "$(dirname "${COMPOSITOR_LOG}")"
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
if [[ "${SESSION_KIND}" == sway-xwayland ]]; then
  sed -i 's/^xwayland disable$/xwayland force/' "${SWAY_CONFIG}"
fi
# The niri rules mirror the Sway ones: harness windows float at the top-left
# at 940x780 and the sentinel opens fullscreen. Animations stay off because
# niri's IPC reports destination geometry, not the animated position. Focus
# does not follow the mouse, so floating windows stay hidden behind the focused
# fullscreen sentinel; the testkit observer relies on that. CUA_E2E_NIRI_SCALE
# sets the output scale for a focused fractional-scale smoke. Harness sizes are
# logical, so divide them by the scale: the windows keep Sway's 940x780
# physical pixels and stay inside the smaller logical output. prefer-no-csd
# matches Sway's server-side decorations: with client-side ones a GTK 3 or
# Chromium accessible frame includes shadow margins that niri does not report,
# and the driver then publishes no element frames for that window.
NIRI_SCALE="${CUA_E2E_NIRI_SCALE:-1}"
NIRI_HARNESS_WIDTH="$(awk -v scale="${NIRI_SCALE}" 'BEGIN { printf "%d", 940 / scale }')"
NIRI_HARNESS_HEIGHT="$(awk -v scale="${NIRI_SCALE}" 'BEGIN { printf "%d", 780 / scale }')"
cat > "${NIRI_CONFIG}" <<EOF
hotkey-overlay {
    skip-at-startup
}
prefer-no-csd
xwayland-satellite {
    off
}
animations {
    off
}
output "winit" {
    scale ${NIRI_SCALE}
}
layout {
    focus-ring {
        off
    }
    border {
        off
    }
}
window-rule {
    match title="^CuaTestHarness"
    exclude title="^CuaTestHarness Sentinel"
    open-floating true
    default-floating-position x=0 y=0 relative-to="top-left"
    default-column-width {
        fixed ${NIRI_HARNESS_WIDTH}
    }
    default-window-height {
        fixed ${NIRI_HARNESS_HEIGHT}
    }
}
window-rule {
    match title="^CuaTestHarness Sentinel"
    open-fullscreen true
}
EOF

unset DISPLAY
unset WAYLAND_DISPLAY
# A runner started from a desktop inherits that session's sockets and buses.
# Export only this session's own so no test can reach the host compositor,
# Secret Service, accessibility bus, or portals.
unset WAYLAND_SOCKET NIRI_SOCKET SWAYSOCK I3SOCK HYPRLAND_INSTANCE_SIGNATURE
unset DBUS_SESSION_BUS_ADDRESS AT_SPI_BUS_ADDRESS
export XDG_RUNTIME_DIR="${RUNTIME_DIR}"
export XDG_SESSION_TYPE=wayland
if [[ "${SESSION_KIND}" == niri ]]; then
  export XDG_CURRENT_DESKTOP=niri
  export XDG_SESSION_DESKTOP=niri
else
  export XDG_CURRENT_DESKTOP=sway
  export XDG_SESSION_DESKTOP=sway
fi
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_HEADLESS_OUTPUTS=1
export CUA_DRIVER_RS_ENABLE_WAYLAND=1
if [[ "${SESSION_KIND}" == cua-compositor ]]; then
  export CUA_E2E_COMPOSITOR=cua-compositor-nested
  export CUA_E2E_INPUT_BACKENDS=atspi,cua-compositor-inject
  export CUA_E2E_HARNESS_FILTER=electron
  export CUA_INJECT_SOCKET="${XDG_RUNTIME_DIR}/cua-inject.sock"
elif [[ "${SESSION_KIND}" == niri ]]; then
  export CUA_E2E_COMPOSITOR=niri
  export CUA_E2E_INPUT_BACKENDS=atspi,wlr-virtual-pointer
else
  export CUA_E2E_COMPOSITOR=sway
  export CUA_E2E_INPUT_BACKENDS=atspi,wlr-virtual-pointer
  if [[ "${SESSION_KIND}" == sway-xwayland ]]; then
    export CUA_E2E_WAYLAND_XWAYLAND_REGRESSION=1
  fi
fi
if [[ "${SESSION_KIND}" == niri ]]; then
  export CUA_WAYLAND_RECORDING_OUTPUT=winit
else
  export CUA_WAYLAND_RECORDING_OUTPUT=HEADLESS-1
fi
export ELECTRON_OZONE_PLATFORM_HINT=wayland
export GDK_BACKEND=wayland
export QT_QPA_PLATFORM=wayland
# WebKitGTK's accelerated compositor requires a DRM render node. The canonical
# hosted lane has none, so keep its best-effort software settings explicit;
# native-Wayland WebKit coverage runs on the representative GNOME/KDE VMs.
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
# This isolated CI session uses a private runtime directory and no user home
# namespaces. Modern WebKitGTK ignores WEBKIT_FORCE_SANDBOX; use its explicit
# test-only escape hatch so the WebProcess can publish its AT-SPI subtree.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export NO_AT_BRIDGE=0
export ACCESSIBILITY_ENABLED=1

dbus_daemon="$(command -v dbus-daemon)"
dbus_prefix="$(dirname "$(dirname "$(readlink -f "${dbus_daemon}")")")"
dbus_config="${dbus_prefix}/share/dbus-1/session.conf"
if [[ ! -f "${dbus_config}" ]]; then
  echo "DBus session config is missing: ${dbus_config}" >&2
  exit 1
fi
dbus_info="$(dbus-daemon --config-file="${dbus_config}" --fork --print-address=1 --print-pid=1)"
export DBUS_SESSION_BUS_ADDRESS="$(sed -n '1p' <<< "${dbus_info}")"
DBUS_PID="$(sed -n '2p' <<< "${dbus_info}")"

# The shared lane exercises the encrypted Computer History lifecycle. Give its
# Linux key provider the same unlocked Secret Service that the canonical X11
# lane provisions, but on this lane's private disposable session bus.
if [[ "${CUA_E2E_INTERNAL_LANE:-all}" == "shared" \
    || "${CUA_E2E_INTERNAL_LANE:-all}" == "all" ]]; then
  if ! command -v gnome-keyring-daemon >/dev/null 2>&1; then
    echo "gnome-keyring-daemon is required for the shared Wayland history gate" >&2
    exit 1
  fi
  # The history gate purges the cua-driver-local credential from whichever
  # Secret Service it reaches. Give the daemon its own home and data
  # directories so the disposable keyring never touches the user's.
  mkdir -m 700 "${RUNTIME_DIR}/keyring-home"
  keyring_environment="$(
    printf 'test-password' \
      | HOME="${RUNTIME_DIR}/keyring-home" \
        XDG_DATA_HOME="${RUNTIME_DIR}/keyring-home/.local/share" \
        gnome-keyring-daemon --unlock --components=secrets
  )"
  eval "${keyring_environment}"
  deadline=$((SECONDS + 15))
  while ((SECONDS < deadline)); do
    if gdbus call --session \
        --dest org.freedesktop.secrets \
        --object-path /org/freedesktop/secrets \
        --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
      break
    fi
    sleep 0.2
  done
  if ! gdbus call --session \
      --dest org.freedesktop.secrets \
      --object-path /org/freedesktop/secrets \
      --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
    echo "Secret Service did not become ready on the private Wayland session bus" >&2
    exit 1
  fi
  # The daemon detaches from this shell; stop it by the bus name it owns.
  KEYRING_PID="$(
    gdbus call --session \
      --dest org.freedesktop.DBus \
      --object-path /org/freedesktop/DBus \
      --method org.freedesktop.DBus.GetConnectionUnixProcessID org.freedesktop.secrets \
      | sed -n 's/^(uint32 \([0-9]*\),)$/\1/p'
  )"
fi

# A private session bus does not activate the desktop accessibility stack by
# itself. Start the repo-pinned AT-SPI launcher, then require org.a11y.Bus to
# answer before any fixture or driver process inherits this session.
ATSPI_LAUNCHER="${CUA_AT_SPI_BUS_LAUNCHER:-$(command -v at-spi-bus-launcher || true)}"
if [[ ! -x "${ATSPI_LAUNCHER}" ]]; then
  echo "AT-SPI bus launcher is unavailable: ${ATSPI_LAUNCHER:-<not found>}" >&2
  exit 1
fi
# Inside a systemd user unit, as in any desktop terminal, the launcher prefers
# dbus-broker, whose service activation goes through the host's systemd user
# manager instead of this private bus.
ATSPI_DBUS_IMPLEMENTATION=dbus-daemon \
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

if [[ "${SESSION_KIND}" == cua-compositor ]]; then
  command -v cua-compositor >/dev/null || {
    echo "cua-compositor is required for the nested injection lane" >&2
    exit 1
  }
  cua-compositor > "${COMPOSITOR_LOG}" 2>&1 &
elif [[ "${SESSION_KIND}" == niri ]]; then
  for tool in cage niri wlr-randr; do
    command -v "${tool}" >/dev/null || {
      echo "${tool} is required for the nested niri lane; use nix develop .#cua-driver-niri-e2e" >&2
      exit 1
    }
  done
  if ! niri validate --config "${NIRI_CONFIG}" > "${COMPOSITOR_LOG}" 2>&1; then
    cat "${COMPOSITOR_LOG}" >&2
    exit 1
  fi
  mkdir -m 700 "${CAGE_RUNTIME_DIR}"
  XDG_RUNTIME_DIR="${CAGE_RUNTIME_DIR}" cage -d -- sleep infinity > "${CAGE_LOG}" 2>&1 &
  CAGE_PID=$!
  deadline=$((SECONDS + 20))
  while [[ ! -S "${CAGE_RUNTIME_DIR}/wayland-0" ]] && ((SECONDS < deadline)) \
      && kill -0 "${CAGE_PID}" 2>/dev/null; do
    sleep 0.2
  done
  # niri's winit output takes the size of cage's headless output.
  if [[ ! -S "${CAGE_RUNTIME_DIR}/wayland-0" ]] \
      || ! XDG_RUNTIME_DIR="${CAGE_RUNTIME_DIR}" WAYLAND_DISPLAY=wayland-0 \
        timeout 5 wlr-randr --output HEADLESS-1 --custom-mode 1920x1080 >> "${CAGE_LOG}" 2>&1; then
    echo "cage did not provide a 1920x1080 headless output for niri" >&2
    cat "${CAGE_LOG}" >&2
    exit 1
  fi
  # Without a GPU, niri redraws the whole output in llvmpipe for every frame
  # the behaviour recording copies, and llvmpipe's thread per CPU starves the
  # apps under test: Electron AT-SPI walks ran about 4x slower while recording
  # on the 4-vCPU hosted runner. In the shared lane, one render thread keeps
  # the recording and leaves the other CPUs to the apps. Other lanes keep
  # llvmpipe's default: a single thread on that runner makes each niri frame
  # take longer than the agent cursor's 50 ms animation step, so the cursor
  # animates in slow motion past the overlay idle test's settle time. It has
  # no effect on a hardware GPU.
  niri_render_env=()
  if [[ "${CUA_E2E_INTERNAL_LANE:-all}" == shared ]]; then
    niri_render_env=(LP_NUM_THREADS=1)
  fi
  WAYLAND_DISPLAY="${CAGE_RUNTIME_DIR}/wayland-0" env "${niri_render_env[@]}" \
    niri --config "${NIRI_CONFIG}" >> "${COMPOSITOR_LOG}" 2>&1 &
else
  sway --unsupported-gpu --config "${SWAY_CONFIG}" > "${COMPOSITOR_LOG}" 2>&1 &
fi
COMPOSITOR_PID=$!

deadline=$((SECONDS + 20))
while ((SECONDS < deadline)); do
  if ! kill -0 "${COMPOSITOR_PID}" 2>/dev/null; then
    echo "${SESSION_KIND} exited before its Wayland socket became ready" >&2
    cat "${COMPOSITOR_LOG}" >&2
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
  echo "${SESSION_KIND} did not create a Wayland socket within 20 seconds" >&2
  cat "${COMPOSITOR_LOG}" >&2
  exit 1
fi

if [[ "${SESSION_KIND}" == niri ]]; then
  # niri names its IPC socket niri.<wayland display>.<pid>.sock. Accept only
  # the socket of the niri this runner started, for the display found above.
  export NIRI_SOCKET="${XDG_RUNTIME_DIR}/niri.${WAYLAND_DISPLAY}.${COMPOSITOR_PID}.sock"
  deadline=$((SECONDS + 10))
  while [[ ! -S "${NIRI_SOCKET}" ]] && ((SECONDS < deadline)); do
    sleep 0.2
  done
  if [[ ! -S "${NIRI_SOCKET}" ]]; then
    echo "niri did not expose its IPC socket ${NIRI_SOCKET}" >&2
    cat "${COMPOSITOR_LOG}" >&2
    exit 1
  fi
  # Both sockets must be served by this niri, and niri must drive only its
  # nested winit output: a host niri would list real outputs. Every niri msg
  # has a deadline because a wedged niri accepts connections without replying.
  socket_peers="$(python3 -I - "${XDG_RUNTIME_DIR}/${WAYLAND_DISPLAY}" "${NIRI_SOCKET}" <<'PY'
import socket
import struct
import sys

for path in sys.argv[1:]:
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(path)
        credentials = connection.getsockopt(
            socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")
        )
        print(struct.unpack("3i", credentials)[0])
PY
)"
  if [[ "$(sort -u <<< "${socket_peers}")" != "${COMPOSITOR_PID}" ]]; then
    echo "niri sockets are not served by niri pid ${COMPOSITOR_PID}: ${socket_peers//$'\n'/ }" >&2
    exit 1
  fi
  niri_outputs="$(timeout 5 niri msg --json outputs)"
  if [[ "$(jq -c 'keys' <<< "${niri_outputs}")" != '["winit"]' ]] \
      || [[ "$(jq -c '.winit.logical | [.width, .height]' <<< "${niri_outputs}")" \
        != "$(jq -nc --argjson scale "${NIRI_SCALE}" '[1920, 1080] | map(. / $scale | floor)')" ]]; then
    echo "nested niri must drive only its 1920x1080 winit output: ${niri_outputs}" >&2
    exit 1
  fi
  jq -n \
    --arg source_sha "${CUA_E2E_SOURCE_SHA:-unknown}" \
    --arg session_type "${XDG_SESSION_TYPE}" \
    --arg compositor "${CUA_E2E_COMPOSITOR}" \
    --arg wayland_display "${WAYLAND_DISPLAY}" \
    --arg cage "$(cage -v 2>&1)" \
    --argjson niri "$(timeout 5 niri msg --json version)" \
    --argjson outputs "${niri_outputs}" \
    '{source_sha:$source_sha,session_type:$session_type,compositor:$compositor,wayland_display:$wayland_display,cage:$cage,niri:$niri,outputs:$outputs}' \
    > "${REPO_ROOT}/artifacts/cua-driver/linux/niri-preflight.json"
elif [[ "${SESSION_KIND}" == cua-compositor ]]; then
  deadline=$((SECONDS + 10))
  while [[ ! -S "${CUA_INJECT_SOCKET}" ]] && ((SECONDS < deadline)); do
    sleep 0.2
  done
  if [[ ! -S "${CUA_INJECT_SOCKET}" ]]; then
    echo "cua-compositor did not expose its injection socket" >&2
    cat "${COMPOSITOR_LOG}" >&2
    exit 1
  fi
else
  export SWAYSOCK="$(find "${XDG_RUNTIME_DIR}" -maxdepth 1 -type s -name 'sway-ipc.*.sock' -print -quit)"
  if [[ -z "${SWAYSOCK}" ]]; then
    echo "Sway did not expose its IPC socket" >&2
    cat "${COMPOSITOR_LOG}" >&2
    exit 1
  fi
  if [[ "${SESSION_KIND}" == sway-xwayland ]]; then
    deadline=$((SECONDS + 15))
    while ((SECONDS < deadline)); do
      xwayland_socket="$(find /tmp/.X11-unix -maxdepth 1 -type s -name 'X*' -newer "${SWAY_CONFIG}" -print -quit 2>/dev/null || true)"
      if [[ -n "${xwayland_socket}" ]]; then
        export CUA_E2E_XWAYLAND_DISPLAY=":${xwayland_socket##*/X}"
        break
      fi
      sleep 0.2
    done
    if [[ -z "${CUA_E2E_XWAYLAND_DISPLAY:-}" ]] || ! xdpyinfo -display "${CUA_E2E_XWAYLAND_DISPLAY}" >/dev/null 2>&1; then
      echo "Sway did not expose a usable XWayland display within 15 seconds" >&2
      cat "${COMPOSITOR_LOG}" >&2
      exit 1
    fi
    jq -n \
      --arg source_sha "${CUA_E2E_SOURCE_SHA:-unknown}" \
      --arg session_type "${XDG_SESSION_TYPE}" \
      --arg compositor "${CUA_E2E_COMPOSITOR}" \
      --arg wayland_display "${WAYLAND_DISPLAY}" \
      --arg xwayland_display "${CUA_E2E_XWAYLAND_DISPLAY}" \
      --argjson sway "$(swaymsg -r -t get_version)" \
      '{source_sha:$source_sha,session_type:$session_type,compositor:$compositor,wayland_display:$wayland_display,xwayland_display:$xwayland_display,sway:$sway}' \
      > "${REPO_ROOT}/artifacts/cua-driver/linux/wayland-xwayland-preflight.json"
  fi
fi

echo "Native Wayland E2E session: ${SESSION_KIND} on ${WAYLAND_DISPLAY}"
SESSION_RUNNER="${CUA_E2E_WAYLAND_RUNNER:-${SCRIPT_DIR}/run-rust-e2e.sh}"
if [[ ! -x "${SESSION_RUNNER}" ]]; then
  echo "Wayland session runner is not executable: ${SESSION_RUNNER}" >&2
  exit 1
fi
set +e
"${SESSION_RUNNER}" "$@"
status=$?
set -e
if [[ "${status}" != 0 && "${SESSION_KIND}" == sway* ]]; then
  swaymsg -t get_tree > "${REPO_ROOT}/artifacts/cua-driver/linux/sway-tree.json" 2>/dev/null || true
fi
if [[ "${status}" != 0 && "${SESSION_KIND}" == niri ]]; then
  for request in windows workspaces outputs; do
    timeout 5 niri msg --json "${request}" \
      > "${REPO_ROOT}/artifacts/cua-driver/linux/niri-${request}.json" 2>/dev/null || true
  done
fi
exit "${status}"
