#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(dirname "$SCRIPT_DIR")
REPOSITORY_ROOT=$(git -C "$PROJECT_DIR" rev-parse --show-toplevel)
SOURCE_REVISION=1629eb71e78dd5682cf39a241ff68517438ea629
SOURCE_ARCHIVE=LumeMetalCapabilities-source-1629eb71.tar.gz

if [ "$#" -ne 2 ]; then
  echo "usage: $0 ARTIFACT_DIR RELEASE_DIR" >&2
  exit 64
fi

ARTIFACT_DIR=$1
RELEASE_DIR=$2

for artifact in \
  LumeMetalCapabilities-arm64.dylib \
  LumeMetalCapabilities-arm64e.dylib \
  metal-capabilities; do
  if [ ! -f "$ARTIFACT_DIR/$artifact" ]; then
    echo "missing artifact: $ARTIFACT_DIR/$artifact" >&2
    exit 1
  fi
done

mkdir -p "$RELEASE_DIR"
for output in \
  LumeMetalCapabilities-arm64.dylib \
  LumeMetalCapabilities-arm64e.dylib \
  metal-capabilities \
  "$SOURCE_ARCHIVE" \
  PROVENANCE.md \
  SHA256SUMS; do
  if [ -e "$RELEASE_DIR/$output" ]; then
    echo "refusing to overwrite release output: $RELEASE_DIR/$output" >&2
    exit 1
  fi
done

cp "$ARTIFACT_DIR/LumeMetalCapabilities-arm64.dylib" "$RELEASE_DIR/"
cp "$ARTIFACT_DIR/LumeMetalCapabilities-arm64e.dylib" "$RELEASE_DIR/"
cp "$ARTIFACT_DIR/metal-capabilities" "$RELEASE_DIR/"

git -C "$REPOSITORY_ROOT" archive \
  --format=tar.gz \
  --prefix=cua-1629eb71/ \
  --output="$RELEASE_DIR/$SOURCE_ARCHIVE" \
  "$SOURCE_REVISION" \
  libs/lume/metal-capability-shim

cp "$PROJECT_DIR/Release/SHA256SUMS" "$RELEASE_DIR/SHA256SUMS"
cp "$PROJECT_DIR/Release/PROVENANCE.md" "$RELEASE_DIR/PROVENANCE.md"
"$SCRIPT_DIR/verify.sh" --no-build "$RELEASE_DIR"

echo "Packaged verified release assets in $RELEASE_DIR"
