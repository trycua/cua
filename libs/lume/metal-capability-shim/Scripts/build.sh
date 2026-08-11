#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(dirname "$SCRIPT_DIR")
OUTPUT_DIR=${1:-"$PROJECT_DIR/dist"}
SOURCE="$PROJECT_DIR/Sources/LumeMetalCapabilities.m"
PROBE_SOURCE="$PROJECT_DIR/Tests/metal-capabilities.m"

mkdir -p "$OUTPUT_DIR"

build_dylib() {
  architecture=$1
  output=$2

  xcrun clang \
    -arch "$architecture" \
    -O3 \
    -Wall \
    -Wextra \
    -Werror \
    -fobjc-arc \
    -fblocks \
    -fvisibility=hidden \
    -dynamiclib \
    -install_name @rpath/LumeMetalCapabilities.dylib \
    -mmacosx-version-min=13.0 \
    -framework Foundation \
    -framework Metal \
    "$SOURCE" \
    -o "$output"

  codesign --force --sign - "$output"
}

build_dylib arm64 "$OUTPUT_DIR/LumeMetalCapabilities-arm64.dylib"
build_dylib arm64e "$OUTPUT_DIR/LumeMetalCapabilities-arm64e.dylib"

xcrun clang \
  -arch arm64 \
  -O2 \
  -Wall \
  -Wextra \
  -Werror \
  -fobjc-arc \
  -framework Foundation \
  -framework Metal \
  "$PROBE_SOURCE" \
  -o "$OUTPUT_DIR/metal-capabilities"

(
  cd "$OUTPUT_DIR"
  shasum -a 256 \
    LumeMetalCapabilities-arm64.dylib \
    LumeMetalCapabilities-arm64e.dylib \
    metal-capabilities \
    > SHA256SUMS
)

printf 'Built artifacts in %s\n' "$OUTPUT_DIR"
