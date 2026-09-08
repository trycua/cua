#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(dirname "$SCRIPT_DIR")

BUILD_ARTIFACTS=1
if [ "${1:-}" = "--no-build" ]; then
  BUILD_ARTIFACTS=0
  shift
fi

OUTPUT_DIR=${1:-"$PROJECT_DIR/dist"}

if [ "$BUILD_ARTIFACTS" -eq 1 ]; then
  "$SCRIPT_DIR/build.sh" "$OUTPUT_DIR"
fi

lipo "$OUTPUT_DIR/LumeMetalCapabilities-arm64.dylib" -verify_arch arm64
lipo "$OUTPUT_DIR/LumeMetalCapabilities-arm64e.dylib" -verify_arch arm64e
lipo "$OUTPUT_DIR/metal-capabilities" -verify_arch arm64
codesign --verify --strict "$OUTPUT_DIR/LumeMetalCapabilities-arm64.dylib"
codesign --verify --strict "$OUTPUT_DIR/LumeMetalCapabilities-arm64e.dylib"

if strings "$OUTPUT_DIR/LumeMetalCapabilities-arm64.dylib" | grep -Eq \
  'GPU_HOOK_TIME_SCALE|mach_absolute_time|clock_gettime|gettimeofday|MESH_FALLBACK|IGNORE_ARGTYPE|SYNC_COMPUTE'; then
  echo "unexpected research-only behavior found in arm64 artifact" >&2
  exit 1
fi

if strings "$OUTPUT_DIR/LumeMetalCapabilities-arm64e.dylib" | grep -Eq \
  'GPU_HOOK_TIME_SCALE|mach_absolute_time|clock_gettime|gettimeofday|MESH_FALLBACK|IGNORE_ARGTYPE|SYNC_COMPUTE'; then
  echo "unexpected research-only behavior found in arm64e artifact" >&2
  exit 1
fi

if strings "$OUTPUT_DIR/LumeMetalCapabilities-arm64.dylib" | grep -Eq \
  'LUME_METAL_FEATURE_PROFILE|featureProfile|LUME_METAL_FAMILY_MAX'; then
  echo "unsafe broad capability behavior found in arm64 artifact" >&2
  exit 1
fi

if strings "$OUTPUT_DIR/LumeMetalCapabilities-arm64e.dylib" | grep -Eq \
  'LUME_METAL_FEATURE_PROFILE|featureProfile|LUME_METAL_FAMILY_MAX'; then
  echo "unsafe broad capability behavior found in arm64e artifact" >&2
  exit 1
fi

(
  cd "$OUTPUT_DIR"
  shasum -a 256 -c SHA256SUMS
)

echo "artifact verification passed"
