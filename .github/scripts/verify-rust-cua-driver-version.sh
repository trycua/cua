#!/usr/bin/env bash
set -euo pipefail

RELEASE_VERSION="${1:?Usage: $0 <release_version> [manifest_path]}"
MANIFEST_PATH="${2:-libs/cua-driver/rust/Cargo.toml}"

MANIFEST_VERSION="$(cargo pkgid -p cua-driver --manifest-path "$MANIFEST_PATH" | awk -F'#' '{print $2}')"

if [ -z "$MANIFEST_VERSION" ]; then
  echo "::error::Unable to read crate version from $MANIFEST_PATH."
  exit 1
fi

if [ "$RELEASE_VERSION" != "$MANIFEST_VERSION" ]; then
  echo "::error::Release version mismatch: tag/version input '$RELEASE_VERSION' != Cargo manifest version '$MANIFEST_VERSION'."
  echo "This prevents publishing the $MANIFEST_PATH-built binaries with stale embedded CARGO_PKG_VERSION."
  exit 1
fi

echo "::notice::Version check passed: $RELEASE_VERSION"
