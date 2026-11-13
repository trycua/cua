#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Runs inside the test image (see run.sh): installs the helper, starts a
# headless KWin on this D-Bus session with two windows that log their input,
# then runs the gate suite and, when a driver is mounted, the driver suite.
set -u
tests=/helper/tests; driver=/driver/cua-driver
# Ubuntu and Debian name Qt 6's qdbus qdbus6, Fedora qdbus-qt6.
qdbus="$(command -v qdbus6 || command -v qdbus-qt6)"
work="$(mktemp -d)"
export XDG_RUNTIME_DIR="$work/run" HOME="$work/home"
mkdir -m700 "$XDG_RUNTIME_DIR"; mkdir "$HOME"
# The real install: build against this image's KWin, into KWin's plugin dir.
# No driver is on PATH, so the script leaves turning the helper on to us.
if ! bash /helper/install.sh >/dev/null 2>"$work/install.err"; then
  echo "helper install failed" >&2; tail "$work/install.err" >&2
  exit 1
fi
export WAYLAND_DISPLAY=cua-kwin-test XDG_SESSION_TYPE=wayland \
  XDG_CURRENT_DESKTOP=KDE CUA_DRIVER_RS_ENABLE_WAYLAND=1
unset DISPLAY
# kwin_wayland carries file capabilities a container may not grant; a plain
# copy runs without them. Its permission checks read KDE's application cache,
# which this throwaway session never builds, so they are off.
cp /usr/bin/kwin_wayland "$work/kwin_wayland"
# KWin's Xwayland puts its socket here and does not create the directory.
mkdir -p -m1777 /tmp/.X11-unix
start_kwin() {
  KWIN_WAYLAND_NO_PERMISSION_CHECKS=1 KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 \
    "$work/kwin_wayland" --virtual --xwayland --socket "$WAYLAND_DISPLAY" --width 1280 --height 800 >"$work/kwin.log" 2>&1 &
  kwin=$!
  for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break; sleep 0.2; done
}
# The X display KWin's Xwayland serves, for the X11 test window.
xwayland_display() {
  local socket
  for _ in $(seq 50); do
    socket=$(ls /tmp/.X11-unix/ 2>/dev/null | head -1)
    [ -n "$socket" ] && { echo ":${socket#X}"; return 0; }
    sleep 0.2
  done
  echo "KWin did not start Xwayland" >&2; tail "$work/kwin.log" >&2
  exit 1
}
# helper_up <failure> [command...]: wait for the helper to answer, running the
# command between tries.
helper_up() {
  local failure="$1"; shift
  for _ in $(seq 100); do
    "$qdbus" org.cua.KWinTarget /org/cua/KWinTarget GetVersion >/dev/null 2>&1 && return 0
    "$@" >/dev/null 2>&1
    sleep 0.2
  done
  echo "$failure" >&2; tail "$work/kwin.log" >&2
  exit 1
}
start_kwin
if [ -x "$driver" ]; then
  "$driver" telemetry disable >/dev/null
  # The driver's switch loads the helper, and KWin must bring it back by
  # itself when it starts again, as it does after a login.
  "$driver" kwin-capture on >/dev/null
  helper_up "kwin-capture on did not load the helper" true
  kill "$kwin"; wait "$kwin" 2>/dev/null
  rm -f "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"*
  start_kwin
  helper_up "KWin did not start the helper again" true
  "$driver" kwin-capture status | grep -qx 'kwin helper: loaded (protocol 2), starts with KWin' ||
    { echo "kwin-capture status does not report the helper" >&2; "$driver" kwin-capture status >&2; exit 1; }
else
  helper_up "KWin did not load the helper" \
    "$qdbus" org.kde.KWin /Effects org.kde.kwin.Effects.loadEffect cua_kwin_target_helper
fi
events="$work/events.log"
GDK_BACKEND=wayland python3 "$tests/keylog.py" A "$events" >/dev/null 2>&1 &
GDK_BACKEND=wayland python3 "$tests/keylog.py" B "$events" >/dev/null 2>&1 &
# X runs under Xwayland, as Java, Wine and other X11-only apps do.
DISPLAY="$(xwayland_display)" GDK_BACKEND=x11 python3 "$tests/keylog.py" X "$events" >/dev/null 2>&1 &
sleep 2
python3 "$tests/gate_test.py" "$events"; status=$?
if [ -x "$driver" ]; then
  # KWin hands out EIS only as a D-Bus fd; the proxy serves it on a socket.
  python3 "$tests/eis_proxy.py" "$XDG_RUNTIME_DIR/eis-0" >/dev/null 2>&1 &
  export LIBEI_SOCKET="$XDG_RUNTIME_DIR/eis-0"
  for _ in $(seq 25); do [ -S "$LIBEI_SOCKET" ] && break; sleep 0.2; done
  python3 "$tests/driver_test.py" "$driver" "$events" 2>"$work/driver.err" || { status=1; tail -20 "$work/driver.err" >&2; }
  # Off must leave KWin with nothing to start.
  "$driver" kwin-capture off >/dev/null
  if grep -q cua_kwin_target_helperEnabled "$HOME/.config/kwinrc"; then
    echo "kwin-capture off left the helper set to start with KWin" >&2; status=1
  fi
fi
mapfile -t jobs < <(jobs -p)
kill "${jobs[@]}" 2>/dev/null; wait 2>/dev/null
exit $status
