#!/usr/bin/env bash
# Build + run the one-grant embedding demo on a freshly reset TCC state.
# MANUAL — needs a real Mac with a user at the keyboard; TCC prompts cannot
# be granted from CI. See ../../Skills/cua-driver/EMBEDDING.md for the pass
# criteria.
#
#   ./demo.sh                      # ad-hoc signature (fine for local demos)
#   SIGN_IDENTITY="Developer ID Application: …" ./demo.sh
set -euo pipefail
cd "$(dirname "$0")"

APP=ExampleAgentHarness.app
BUNDLE_ID=${BUNDLE_ID:-com.trycua.example-agent-harness}
SIGN_IDENTITY=${SIGN_IDENTITY:--}   # "-" = ad-hoc
LOG=/tmp/cua-embedded-demo.log

# Rebuilding re-signs the bundle; a new (ad-hoc) signature can orphan an
# existing TCC grant row, so keep the same build across the grant + re-run
# flow. `rm -rf ExampleAgentHarness.app` to force a rebuild.
if [ -d "$APP" ]; then
    echo "1/4 reusing existing ${APP} (delete it to force a rebuild)"
else
echo "1/4 building ${APP}…"
mkdir -p "$APP/Contents/MacOS"
swiftc -O ExampleAgentHarness.swift -o "$APP/Contents/MacOS/ExampleAgentHarness" \
  -framework ApplicationServices
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>ExampleAgentHarness</string>
    <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
    <key>CFBundleName</key><string>ExampleAgentHarness</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict>
</plist>
PLIST
# A stable code-signing identity is what keys the TCC grant rows to this
# app; re-signing later orphans existing grants (see EMBEDDING.md).
codesign --force --sign "$SIGN_IDENTITY" "$APP"
fi

# Re-runs after granting must skip the reset or they wipe the grant again.
if [ -z "${SKIP_RESET:-}" ]; then
    echo "2/4 resetting TCC state for ${BUNDLE_ID} (expect re-prompts)…"
    tccutil reset Accessibility "$BUNDLE_ID" || true
    tccutil reset ScreenCapture "$BUNDLE_ID" || true
else
    echo "2/4 SKIP_RESET set — keeping existing grants"
fi

echo "3/4 launching the host — grant BOTH prompts, then re-run with"
echo "    SKIP_RESET=1 ./demo.sh   (TCC answers are cached per process)."
echo "    (open(1) is correct HERE: the HOST must be its own responsible"
echo "     process; it then spawns cua-driver directly as its child.)"
# `open` does not forward shell env; route a custom driver path through
# launchd so the app can see it.
if [ -n "${CUA_DRIVER_PATH:-}" ]; then
    launchctl setenv CUA_DRIVER_PATH "$CUA_DRIVER_PATH"
    trap 'launchctl unsetenv CUA_DRIVER_PATH' EXIT
fi
rm -f "$LOG"
open "$APP"

echo "4/4 following $LOG — expect attribution 'host', a screenshot,"
echo "    Finder window state, a visible agent-cursor glide, and"
echo "    'DEMO COMPLETE: PASS'. Then verify System Settings → Privacy &"
echo "    Security lists ONLY ExampleAgentHarness (no CuaDriver)."
# Portable wait (stock macOS has no timeout(1)).
for _ in $(seq 1 120); do
    grep -q "DEMO COMPLETE\|grant the prompts" "$LOG" 2>/dev/null && break
    sleep 1
done
cat "$LOG" 2>/dev/null

grep -q "DEMO COMPLETE: PASS" "$LOG" && echo "✅ one-grant demo passed" || {
    echo "❌ demo failed — debug attribution with:"
    echo "  log stream --debug --predicate 'subsystem == \"com.apple.TCC\" AND eventMessage BEGINSWITH \"AttributionChain\"'"
    exit 1
}
