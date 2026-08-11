#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="$repo_root/.github/workflows/release.yml"
builder="$repo_root/scripts/build-release-archive.sh"

reject() {
  local needle=$1
  local file=$2
  if grep -Fq -- "$needle" "$file"; then
    printf 'unexpected release contract in %s: %s\n' "$file" "$needle" >&2
    exit 1
  fi
}

require() {
  local needle=$1
  local file=$2
  grep -Fq -- "$needle" "$file" || {
    printf 'missing release contract in %s: %s\n' "$file" "$needle" >&2
    exit 1
  }
}

[ -x "$builder" ] || {
  printf 'release builder is missing or not executable: %s\n' "$builder" >&2
  exit 1
}

require 'workflow_dispatch:' "$workflow"
require 'runner: ubuntu-22.04' "$workflow"
require 'runner: ubuntu-24.04-arm' "$workflow"
require 'runner: macos-15-intel' "$workflow"
require 'runner: macos-14' "$workflow"
require 'runner: windows-2022' "$workflow"
require 'runner: windows-11-arm' "$workflow"
require 'LLVM_MINGW_VERSION: "20260616"' "$workflow"
require 'x86_64-w64-mingw32-clang' "$workflow"
require 'aarch64-w64-mingw32-clang' "$workflow"
require 'rust_target: x86_64-pc-windows-gnu' "$workflow"
require 'rust_target: aarch64-pc-windows-gnullvm' "$workflow"
require "rustup target add \"\${{ matrix.rust_target }}\"" "$workflow"
require 'Install llvm-mingw' "$workflow"
require "cygpath -m \"\$toolchain_bin/\${{ matrix.cc }}.exe\"" "$workflow"
require "echo \"GO_BUILD_CC=\$go_build_cc\" >> \"\$GITHUB_ENV\"" "$workflow"
require "GO_BUILD_CC=\${{ matrix.cc }}" "$workflow"
reject 'GITHUB_PATH' "$workflow"
require "./scripts/build-release-archive.sh \"\$VERSION\" dist" "$workflow"
require "terraform-provider-fleets_\${VERSION}_SHA256SUMS" "$workflow"
require 'gpg --batch --armor --detach-sign' "$workflow"
require "terraform-provider-fleets_\${VERSION}_SHA256SUMS.sig" "$workflow"
require "gh release create \"v\$VERSION\"" "$workflow"

require 'CARGO_TERM_COLOR=never' "$builder"
require '--crate-type staticlib' "$builder"
require '--print native-static-libs' "$builder"
require 'windows_linker_flags' "$builder"
require "cygpath -m \"\$packaged_library\"" "$builder"
require "go_build_cc=\"\${GO_BUILD_CC:-\$(go env CC)}\"" "$builder"
require "CC=\"\$go_build_cc\"" "$builder"
require "rust_target=\"\${RUST_TARGET:-}\"" "$builder"
require 'cargo_args=(rustc --locked)' "$builder"
require "cargo_args+=(--target \"\$rust_target\")" "$builder"
require "cargo \"\${cargo_args[@]}\"" "$builder"
reject "\"\${cargo_target_args[@]}\"" "$builder"
require "CARGO_TARGET_\${rust_target_env}_LINKER" "$builder"
require "CC_\${rust_target//-/_}" "$builder"
require 'libcyclops_sdk.a' "$builder"
require 'CGO_ENABLED=1' "$builder"
require "terraform-provider-fleets_\${version}_\${goos}_\${goarch}.zip" "$builder"
