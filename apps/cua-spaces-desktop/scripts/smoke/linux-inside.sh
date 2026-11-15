#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs inside the smoke container; see linux.sh.
set -uo pipefail
fail=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fail=1; fi; }

echo "== deb"
dpkg-deb -I "$DEB" | sed -n '/Package:/,/Description:/p'
dpkg -i "$DEB" >/dev/null
desktop=/usr/share/applications/cua-spaces.desktop
check "deb installs /opt/Cua Spaces/cua-spaces" '[ -x "/opt/Cua Spaces/cua-spaces" ]'
check "desktop entry at $desktop" '[ -f "$desktop" ]'
cat "$desktop" 2>/dev/null
check "desktop entry validates" 'desktop-file-validate "$desktop"'
check "StartupWMClass=cua-spaces" 'grep -qx "StartupWMClass=cua-spaces" "$desktop"'
check "hicolor icons installed" 'ls /usr/share/icons/hicolor/*/apps/cua-spaces.png >/dev/null 2>&1'
ls /usr/share/icons/hicolor/*/apps/cua-spaces.png 2>/dev/null
check "/usr/bin/cua-spaces link" '[ -e /usr/bin/cua-spaces ]'
file -L "/opt/Cua Spaces/cua-spaces"

echo "== AppImage window"
export APPIMAGE_EXTRACT_AND_RUN=1 ELECTRON_DISABLE_SECURITY_WARNINGS=1 NO_AT_BRIDGE=1
export CUA_SPACES_USER_DATA=/tmp/profile-window CUA_SPACES_LOG_STARTUP=1
Xvfb :99 -screen 0 1440x900x24 -nolisten tcp >/dev/null 2>&1 &
export DISPLAY=:99
sleep 1
openbox >/dev/null 2>&1 &
cp "$APPIMAGE" /tmp/app.AppImage && chmod +x /tmp/app.AppImage
# Under emulation (x64 on an arm64 host) binfmt_misc does not recognise an ELF
# carrying the AppImage magic at offset 8. Zero it in the copy; the runtime
# does not read it.
if [ "${EMULATED:-0}" = 1 ]; then
  printf '\0\0\0' | dd of=/tmp/app.AppImage bs=1 seek=8 count=3 conv=notrunc 2>/dev/null
  echo "note: AppImage magic zeroed for emulation"
fi
dbus-launch --exit-with-session /tmp/app.AppImage --no-sandbox >/out/window.log 2>&1 &
wid=""
for _ in $(seq 1 90); do
  wid=$(xdotool search --onlyvisible --class cua-spaces 2>/dev/null | head -1)
  [ -n "$wid" ] && break
  sleep 1
done
check "window is mapped" '[ -n "$wid" ]'
if [ -n "$wid" ]; then
  sleep 6
  xprop -id "$wid" WM_CLASS _NET_WM_NAME | tee /out/wm.txt
  check "WM_CLASS is cua-spaces" 'grep -q "\"cua-spaces\"" /out/wm.txt'
  import -window root /out/screen.png
  check "screenshot written" '[ -s /out/screen.png ]'
fi
cat /out/window.log
pkill -f app.AppImage; pkill -f cua-spaces; sleep 2

echo "== AppImage routes"
export CUA_SPACES_USER_DATA=/tmp/profile-routes CUA_SPACES_CAPTURE_ROUTES=/out/routes
timeout 180 dbus-launch --exit-with-session /tmp/app.AppImage --no-sandbox >/out/routes.log 2>&1
grep -o '\[cua-spaces\] capture .*' /out/routes.log | sed 's/^\[cua-spaces\] capture //' >/out/routes.json
check "route report written" '[ -s /out/routes.json ]'
check "every page reports bridge mode electron" \
  'node_ok=$(grep -o "\"bridge\":\"[a-z]*\"" /out/routes.json | sort | uniq -c); echo "$node_ok"; [ "$(echo "$node_ok" | wc -l)" = 1 ] && echo "$node_ok" | grep -q "\"bridge\":\"electron\""'
check "12 route captures" '[ "$(ls /out/routes/*.png 2>/dev/null | wc -l)" = 12 ]'
[ $fail = 0 ] && echo "SMOKE OK" || echo "SMOKE FAILED"
exit $fail
