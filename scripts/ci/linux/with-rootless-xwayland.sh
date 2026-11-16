#!/usr/bin/env bash
# Run a command against a rootless Xwayland inside a headless sway, the X
# server that Hyprland, KWin, WSLg and gamescope give X11 clients. Its root
# window holds no pixels, so a root GetImage fails with BadMatch.
# Usage: with-rootless-xwayland.sh <command> [args...]
set -euo pipefail

RUNTIME_DIR="$(mktemp -d)"
chmod 700 "${RUNTIME_DIR}"
SWAY_CONFIG="${RUNTIME_DIR}/sway.config"
SWAY_PID=""

cleanup() {
  [[ -n "${SWAY_PID}" ]] && kill "${SWAY_PID}" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "${RUNTIME_DIR}"
}
trap cleanup EXIT

unset DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET SWAYSOCK I3SOCK HYPRLAND_INSTANCE_SIGNATURE NIRI_SOCKET
export XDG_RUNTIME_DIR="${RUNTIME_DIR}"
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1
# sway's own Xwayland is rootless and has sway as its window manager, as the
# desktops above run theirs. A rootless Xwayland without one leaves windows
# unpainted.
printf 'output HEADLESS-1 resolution 1280x720\nxwayland force\n' > "${SWAY_CONFIG}"

sway -c "${SWAY_CONFIG}" >"${RUNTIME_DIR}/sway.log" 2>&1 &
SWAY_PID=$!
deadline=$((SECONDS + 15))
until compgen -G "${RUNTIME_DIR}/sway-ipc.*.sock" >/dev/null; do
  if ((SECONDS >= deadline)) || ! kill -0 "${SWAY_PID}" 2>/dev/null; then
    echo "headless sway did not start" >&2
    cat "${RUNTIME_DIR}/sway.log" >&2
    exit 1
  fi
  sleep 0.1
done
SWAYSOCK="$(compgen -G "${RUNTIME_DIR}/sway-ipc.*.sock" | head -n 1)"
export SWAYSOCK
# sway hands its clients the Xwayland DISPLAY.
swaymsg -q exec "echo \"\$DISPLAY\" > '${RUNTIME_DIR}/display'"
until [[ -s "${RUNTIME_DIR}/display" ]] && xdpyinfo -display "$(cat "${RUNTIME_DIR}/display")" >/dev/null 2>&1; do
  if ((SECONDS >= deadline)); then
    echo "sway's Xwayland did not start" >&2
    cat "${RUNTIME_DIR}/sway.log" >&2
    exit 1
  fi
  sleep 0.1
done
DISPLAY="$(cat "${RUNTIME_DIR}/display")"
export DISPLAY
# The command sees an X11 session through Xwayland, as on those desktops.
unset SWAYSOCK

"$@"
