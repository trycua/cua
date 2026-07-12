#!/usr/bin/env bash
# Build the macOS test fixture apps (AppKit + SwiftUI + WKWebView + Electron + Tauri)
# into app bundles under libs/cua-driver/rust/test-apps/harness-* —
# matching the convention used by windows.ps1 for the Windows WPF + WinUI3 +
# WebView2 apps. WKWebView is the Apple-WebKit host; Electron is the shared
# Chromium harness used across Windows, Linux, and macOS. Tauri is the shared
# native-webview harness used across Windows, Linux, and macOS.
#
# Usage:
#   ./macos.sh                  # build all macOS-runnable harnesses
#   ./macos.sh --skip swiftui   # skip one target (appkit|swiftui|wkwebview|electron|tauri)
#   ./macos.sh --only wkwebview # build just one target
#   ./macos.sh --clean          # archive staged outputs first
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
STAGE_DIR="$(cd "$HARNESS_DIR/../../rust/test-apps" && pwd)"

SKIP=""
ONLY=""
CLEAN=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip) SKIP="$2"; shift 2;;
        --only) ONLY="$2"; shift 2;;
        --clean) CLEAN=1; shift;;
        -h|--help)
            sed -n '2,11p' "$0"; exit 0;;
        *) echo "unknown arg: $1" >&2; exit 1;;
    esac
done

archive_existing() {
    local target="$1"
    [[ -e "$target" ]] || return 0
    local archive_root="${TMPDIR:-/tmp}/cua-driver-fixture-build-archive"
    local stamp
    stamp="$(date +%Y%m%d-%H%M%S)-$$"
    mkdir -p "$archive_root"
    mv "$target" "$archive_root/$(basename "$target").$stamp"
}

if [[ "$CLEAN" == "1" ]]; then
    archive_existing "$STAGE_DIR/harness-appkit"
    archive_existing "$STAGE_DIR/harness-swiftui"
    archive_existing "$STAGE_DIR/harness-wkwebview"
    archive_existing "$STAGE_DIR/harness-electron"
    archive_existing "$STAGE_DIR/harness-tauri"
    mkdir -p "$STAGE_DIR/harness-appkit" "$STAGE_DIR/harness-swiftui" "$STAGE_DIR/harness-wkwebview" "$STAGE_DIR/harness-electron" "$STAGE_DIR/harness-tauri"
    echo "==> Cleaned stage dirs"
fi

build_app() {
    local name="$1" src_dir="$2" frameworks="$3" stage_subdir="$4"
    local bundle="$STAGE_DIR/$stage_subdir/$name.app"
    local exe_path="$bundle/Contents/MacOS/$name"
    local plist="$bundle/Contents/Info.plist"

    echo "==> Building $name"
    archive_existing "$bundle"
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

if [[ -z "$ONLY" || "$ONLY" == "appkit" ]] && [[ "$SKIP" != "appkit" ]]; then
    build_app "CuaTestHarness.AppKit" \
        "$HARNESS_DIR/apps/macos/appkit" \
        "" \
        "harness-appkit"
fi

if [[ -z "$ONLY" || "$ONLY" == "swiftui" ]] && [[ "$SKIP" != "swiftui" ]]; then
    build_app "CuaTestHarness.SwiftUI" \
        "$HARNESS_DIR/apps/macos/swiftui" \
        "" \
        "harness-swiftui"
fi

if [[ -z "$ONLY" || "$ONLY" == "wkwebview" ]] && [[ "$SKIP" != "wkwebview" ]]; then
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

if [[ -z "$ONLY" || "$ONLY" == "electron" ]] && [[ "$SKIP" != "electron" ]]; then
    "$HARNESS_DIR/apps/cross-platform/electron/build.sh"
fi

if [[ -z "$ONLY" || "$ONLY" == "tauri" ]] && [[ "$SKIP" != "tauri" ]]; then
    "$HARNESS_DIR/apps/cross-platform/tauri/build.sh"
fi

echo ""
echo "Built apps:"
find "$STAGE_DIR" -maxdepth 3 -name "*.app" -type d
