#!/bin/bash
# The developer tools image's gate, IN the guest (dev-image-gate.sh runs it):
# build and test the HelloSim fixture on the iOS Simulator, build its Rust core
# for the iOS Simulator target, drive the app, take screenshots and write
# timings. Everything lands in STAGE/results.
#
#   dev-image-gate-guest.sh STAGE_DIR
set -euo pipefail
STAGE="${1:?staging directory}"
R="$STAGE/results"
mkdir -p "$R"
export PATH="$HOME/.cargo/bin:/usr/local/bin:$PATH"
W="$HOME/gate-work"
rm -rf "$W"; mkdir -p "$W"
cp -R "$STAGE/fixture/." "$W/"
cd "$W"
t() { date +%s; }
: >"$R/timings.txt"
mark() { echo "$1 $2" >>"$R/timings.txt"; }
DEST='platform=iOS Simulator,name=Cua iPhone'

{
    echo "== tools"
    xcodebuild -version
    xcrun simctl list runtimes
    xcrun simctl list devices available | grep -i "cua iphone" || true
    xcodegen --version; pod --version; sccache --version; rustup target list --installed
    git --version; gh --version | head -1; node --version; pnpm --version
    DevToolsSecurity -status
    df -h /
} >"$R/tools.txt" 2>&1

echo "== xcodegen"
xcodegen generate >"$R/xcodegen.log" 2>&1

echo "== cold build and test (build + unit + UI tests on the Simulator)"
s=$(t)
xcodebuild test -project HelloSim.xcodeproj -scheme HelloSim -destination "$DEST" \
    -derivedDataPath "$W/dd" -resultBundlePath "$R/cold.xcresult" >"$R/xcodebuild-cold.log" 2>&1 || {
    tail -40 "$R/xcodebuild-cold.log" >&2; exit 1; }
mark cold_xcode_seconds $(( $(t) - s ))

echo "== warm build and test (nothing changed)"
s=$(t)
xcodebuild test -project HelloSim.xcodeproj -scheme HelloSim -destination "$DEST" \
    -derivedDataPath "$W/dd" -resultBundlePath "$R/warm.xcresult" >"$R/xcodebuild-warm.log" 2>&1 || {
    tail -40 "$R/xcodebuild-warm.log" >&2; exit 1; }
mark warm_xcode_seconds $(( $(t) - s ))

echo "== incremental build after an edit"
printf '\n// edited\n' >>HelloSim/Counter.swift
s=$(t)
xcodebuild build-for-testing -project HelloSim.xcodeproj -scheme HelloSim -destination "$DEST" \
    -derivedDataPath "$W/dd" >"$R/xcodebuild-incremental.log" 2>&1
mark incremental_xcode_seconds $(( $(t) - s ))

xcrun xcresulttool get test-results summary --path "$R/cold.xcresult" >"$R/cold-summary.json"
mkdir -p "$R/ui-screenshots"
xcrun xcresulttool export attachments --path "$R/cold.xcresult" --output-path "$R/ui-screenshots" >/dev/null

echo "== Rust core for the iOS Simulator (sccache)"
cd rust
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0
sccache --stop-server >/dev/null 2>&1 || true
sccache --start-server >/dev/null 2>&1
s=$(t)
cargo build --release --target aarch64-apple-ios-sim >"$R/cargo-cold.log" 2>&1
mark cold_rust_seconds $(( $(t) - s ))
cargo clean >/dev/null 2>&1
s=$(t)
cargo build --release --target aarch64-apple-ios-sim >"$R/cargo-warm.log" 2>&1
mark warm_rust_seconds $(( $(t) - s ))
sccache --show-stats >"$R/sccache.txt" 2>&1 || true
ls -la target/aarch64-apple-ios-sim/release/libhellosim_core.a >>"$R/tools.txt"
cd ..

echo "== run the app in the Simulator and take a screenshot"
udid="$(xcrun simctl list devices available | sed -n 's/.*Cua iPhone (\([0-9A-F-]*\)).*/\1/p' | head -1)"
s=$(t)
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null
mark simulator_boot_seconds $(( $(t) - s ))
app="$(find "$W/dd/Build/Products" -name HelloSim.app -maxdepth 3 | head -1)"
xcrun simctl install "$udid" "$app"
xcrun simctl launch "$udid" ai.cua.fixture.HelloSim >/dev/null
sleep 3
xcrun simctl io "$udid" screenshot "$R/simulator.png" >/dev/null
open -a Simulator || true
sleep 5
echo "done"
