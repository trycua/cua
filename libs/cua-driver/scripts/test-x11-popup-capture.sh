#!/usr/bin/env bash
# Run only in a fresh Xvfb desktop; never borrow the caller's DISPLAY.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../rust"
exec xvfb-run -a --server-args="-screen 0 1280x800x24 -nolisten tcp" bash -euo pipefail <<'DESKTOP'
wm_log=$(mktemp)
openbox >"$wm_log" 2>&1 &
wm_pid=$!
cleanup() {
  kill "$wm_pid" 2>/dev/null || true
  wait "$wm_pid" 2>/dev/null || true
  rm -f "$wm_log"
}
trap cleanup EXIT
ready=false
for _ in $(seq 1 100); do
  if xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q 'window id # 0x'; then
    ready=true
    break
  fi
  kill -0 "$wm_pid" || { cat "$wm_log"; exit 1; }
  sleep 0.1
done
if [[ "$ready" != true ]]; then
  cat "$wm_log"
  exit 1
fi
CUA_POPUP_CAPTURE_DISPOSABLE_X11=1 cargo test --locked -p platform-linux --lib \
  capture::popup::live_tests -- --ignored --nocapture --test-threads=1
DESKTOP
