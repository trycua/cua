#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Runs inside the test image (see run.sh): builds the helper, starts a headless
# KWin on this D-Bus session with two windows that log their input, then runs
# the gate suite and, when a driver is mounted, the driver suite.
set -u
tests=/helper/tests; driver=/driver/cua-driver
work="$(mktemp -d)"
export XDG_RUNTIME_DIR="$work/run" HOME="$work/home"
mkdir -m700 "$XDG_RUNTIME_DIR"; mkdir "$HOME"
if ! cmake -S /helper -B "$work/build" -DCMAKE_BUILD_TYPE=Release >/dev/null ||
  ! cmake --build "$work/build" -j"$(nproc)" >/dev/null; then
  echo "helper build failed" >&2
  exit 1
fi
export QT_PLUGIN_PATH="$work/build/bin" WAYLAND_DISPLAY=cua-kwin-test XDG_SESSION_TYPE=wayland \
  XDG_CURRENT_DESKTOP=KDE CUA_DRIVER_RS_ENABLE_WAYLAND=1
unset DISPLAY
if [ -x "$driver" ]; then
  "$driver" telemetry disable >/dev/null
  "$driver" kwin-capture on >/dev/null
fi
# kwin_wayland carries file capabilities a container may not grant; a plain
# copy runs without them. Its permission checks read KDE's application cache,
# which this throwaway session never builds, so they are off.
cp /usr/bin/kwin_wayland "$work/kwin_wayland"
KWIN_WAYLAND_NO_PERMISSION_CHECKS=1 KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 \
  "$work/kwin_wayland" --virtual --socket "$WAYLAND_DISPLAY" --width 1280 --height 800 >"$work/kwin.log" 2>&1 &
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break; sleep 0.2; done
loaded=
for _ in $(seq 100); do
  qdbus6 org.cua.KWinTarget /org/cua/KWinTarget GetVersion >/dev/null 2>&1 && { loaded=1; break; }
  qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadEffect cua_kwin_target_helper >/dev/null 2>&1
  sleep 0.2
done
[ -n "$loaded" ] || { echo "KWin did not load the helper" >&2; tail "$work/kwin.log" >&2; exit 1; }
events="$work/events.log"
GDK_BACKEND=wayland python3 "$tests/keylog.py" A "$events" >/dev/null 2>&1 &
GDK_BACKEND=wayland python3 "$tests/keylog.py" B "$events" >/dev/null 2>&1 &
sleep 2
python3 "$tests/gate_test.py" "$events"; status=$?
if [ -x "$driver" ]; then
  # KWin hands out EIS only as a D-Bus fd; the proxy serves it on a socket.
  python3 "$tests/eis_proxy.py" "$XDG_RUNTIME_DIR/eis-0" >/dev/null 2>&1 &
  export LIBEI_SOCKET="$XDG_RUNTIME_DIR/eis-0"
  for _ in $(seq 25); do [ -S "$LIBEI_SOCKET" ] && break; sleep 0.2; done
  python3 "$tests/driver_test.py" "$driver" "$events" 2>"$work/driver.err" || { status=1; tail -20 "$work/driver.err" >&2; }
fi
mapfile -t jobs < <(jobs -p)
kill "${jobs[@]}" 2>/dev/null; wait 2>/dev/null
exit $status
