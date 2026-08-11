#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="$repo_root/.github/workflows/release.yml"
builder="$repo_root/scripts/build-release-archive.sh"

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
require "./scripts/build-release-archive.sh \"\$VERSION\" dist" "$workflow"
require "terraform-provider-fleets_\${VERSION}_SHA256SUMS" "$workflow"
require 'gpg --batch --armor --detach-sign' "$workflow"
require "terraform-provider-fleets_\${VERSION}_SHA256SUMS.sig" "$workflow"
require "gh release create \"v\$VERSION\"" "$workflow"

require 'CARGO_TERM_COLOR=never' "$builder"
require 'cargo rustc' "$builder"
require '--crate-type staticlib' "$builder"
require '--print native-static-libs' "$builder"
require 'CGO_ENABLED=1' "$builder"
require "terraform-provider-fleets_\${version}_\${goos}_\${goarch}.zip" "$builder"
