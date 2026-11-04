#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "This regression check requires a native macOS linker." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ARTIFACT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cua-swift-linker.XXXXXX")"
export CARGO_TARGET_DIR="${ARTIFACT_DIR}/target"
export CARGO_TERM_COLOR=never
export RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }-W linker-messages"
unset CARGO_ENCODED_RUSTFLAGS

printf 'Evidence: %s\n' "${ARTIFACT_DIR}"
{
  git -C "${REPO_ROOT}" rev-parse HEAD
  rustc --version
  cargo --version
  xcrun swift --version
  sw_vers
} > "${ARTIFACT_DIR}/environment.txt" 2>&1

set +e
cargo build --manifest-path "${REPO_ROOT}/libs/cua-driver/rust/Cargo.toml" \
  --release --locked -p cua-driver 2>&1 | tee "${ARTIFACT_DIR}/build.log"
BUILD_STATUSES=("${PIPESTATUS[@]}")
set -e

if [[ "${BUILD_STATUSES[0]}" != 0 || "${BUILD_STATUSES[1]}" != 0 ]]; then
  printf 'FAIL: native build or evidence capture failed (%s).\n' "${BUILD_STATUSES[*]}" >&2
  exit 1
fi

if grep -iE 'duplicate symbols?([[:space:]]|:)' "${ARTIFACT_DIR}/build.log" \
    > "${ARTIFACT_DIR}/duplicate-symbols.txt"; then
  echo "FAIL: successful native build emitted duplicate-symbol linker diagnostics." >&2
  exit 1
fi

echo "PASS: fresh native build succeeded without duplicate-symbol linker diagnostics."
