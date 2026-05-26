#!/usr/bin/env bash
set -euo pipefail

if [[ -d "$HOME/.cargo/bin" ]]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS only" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
EXAMPLE_DIR="$ROOT/crates/cua-driver-embedded/examples/macos-app-smoke"
OUT_DIR="${TMPDIR:-/tmp}/cua-embedded-app-check"
APP="$OUT_DIR/CuaEmbeddedAppCheck.app"
RESULT="$OUT_DIR/open.out"

cd "$ROOT"
cargo build --release -p cua-driver-embedded

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"
cp "$EXAMPLE_DIR/Info.plist" "$APP/Contents/Info.plist"
cp "$ROOT/target/release/libcua_driver_embedded.dylib" "$APP/Contents/Frameworks/"
install_name_tool -id @rpath/libcua_driver_embedded.dylib \
  "$APP/Contents/Frameworks/libcua_driver_embedded.dylib"

# The current macOS platform backend links a small Swift ScreenCaptureKit
# bridge. Most Swift runtime libraries are in /usr/lib/swift on recent macOS,
# but libswift_Concurrency is still resolved via @rpath in this build.
SWIFT_RUNTIME="/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx"
if otool -L "$APP/Contents/Frameworks/libcua_driver_embedded.dylib" | grep -q '@rpath/libswift_Concurrency.dylib'; then
  cp "$SWIFT_RUNTIME/libswift_Concurrency.dylib" "$APP/Contents/Frameworks/"
fi

clang \
  -framework CoreFoundation \
  -I "$ROOT/crates/cua-driver-embedded/include" \
  "$EXAMPLE_DIR/CuaEmbeddedAppCheck.c" \
  "$APP/Contents/Frameworks/libcua_driver_embedded.dylib" \
  -Wl,-rpath,@executable_path/../Frameworks \
  -o "$APP/Contents/MacOS/CuaEmbeddedAppCheck"

codesign --force --deep --sign - "$APP" >/dev/null
codesign --verify --deep --strict "$APP"

rm -f "$RESULT"
open -n -W "$APP" --args "$RESULT"
cat "$RESULT"
