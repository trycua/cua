#!/usr/bin/env bash
# Build the macOS test-harness apps (AppKit + SwiftUI + WKWebView) into .app
# bundles under ../../rust/test-apps/harness-{appkit,swiftui,wkwebview}/ —
# matching the convention used by windows.ps1 for the Windows WPF + WinUI3 +
# WebView2 apps. WKWebView is the macOS analogue of the Windows WebView2
# harness: an Apple-WebKit (NOT Chromium) host loading the shared web DOM.
#
# Usage:
#   ./macos.sh                  # build all three
#   ./macos.sh --skip swiftui   # skip one target (appkit|swiftui|wkwebview)
#   ./macos.sh --clean          # remove staged outputs first
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
STAGE_DIR="$(cd "$HARNESS_DIR/../rust/test-apps" && pwd)"

SKIP=""
CLEAN=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip) SKIP="$2"; shift 2;;
        --clean) CLEAN=1; shift;;
        -h|--help)
            sed -n '2,11p' "$0"; exit 0;;
        *) echo "unknown arg: $1" >&2; exit 1;;
    esac
done

if [[ "$CLEAN" == "1" ]]; then
    rm -rf "$STAGE_DIR/harness-appkit" "$STAGE_DIR/harness-swiftui" "$STAGE_DIR/harness-wkwebview"
    mkdir -p "$STAGE_DIR/harness-appkit" "$STAGE_DIR/harness-swiftui" "$STAGE_DIR/harness-wkwebview"
    echo "==> Cleaned stage dirs"
fi

build_app() {
    local name="$1" src_dir="$2" frameworks="$3" stage_subdir="$4"
    local bundle="$STAGE_DIR/$stage_subdir/$name.app"
    local exe_path="$bundle/Contents/MacOS/$name"
    local plist="$bundle/Contents/Info.plist"

    echo "==> Building $name"
    rm -rf "$bundle"
    mkdir -p "$bundle/Contents/MacOS"

    # shellcheck disable=SC2086  # word-splitting on $frameworks is intentional
    xcrun swiftc \
        -O \
        -target arm64-apple-macos13.0 \
        $frameworks \
        -parse-as-library \
        -o "$exe_path" \
        "$src_dir"/*.swift

    # Minimal Info.plist — bundle ID + LSUIElement=false so the app shows
    # in the Dock and is targetable by NSWorkspace/AX queries normally.
    cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>$name</string>
    <key>CFBundleIdentifier</key><string>com.trycua.harness.$(echo "$stage_subdir" | sed 's/harness-//')</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>1.0.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
EOF
    chmod +x "$exe_path"
    echo "    → $bundle"
}

if [[ "$SKIP" != "appkit" ]]; then
    build_app "CuaTestHarness.AppKit" \
        "$HARNESS_DIR/apps/macos/appkit" \
        "" \
        "harness-appkit"
fi

if [[ "$SKIP" != "swiftui" ]]; then
    build_app "CuaTestHarness.SwiftUI" \
        "$HARNESS_DIR/apps/macos/swiftui" \
        "" \
        "harness-swiftui"
fi

if [[ "$SKIP" != "wkwebview" ]]; then
    build_app "CuaTestHarness.WKWebView" \
        "$HARNESS_DIR/apps/macos/wkwebview" \
        "-framework WebKit" \
        "harness-wkwebview"
    # Bundle the SHARED web DOM (same index.html the Windows WebView2 and the
    # Electron harnesses load) into the app's Resources so the WKWebView is
    # self-contained and reuses the canonical content rather than duplicating it.
    WK_RES="$STAGE_DIR/harness-wkwebview/CuaTestHarness.WKWebView.app/Contents/Resources/web"
    mkdir -p "$WK_RES"
    cp "$HARNESS_DIR/shared/web/index.html" "$WK_RES/index.html"
    echo "    → bundled shared/web/index.html into Resources/web/"
fi

echo ""
echo "Built apps:"
find "$STAGE_DIR" -maxdepth 3 -name "*.app" -type d
