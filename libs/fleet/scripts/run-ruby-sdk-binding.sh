#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
bindings="$repo_root/cyclops-cs/sdk-bindings/ruby"
library="$("$repo_root"/cyclops-cs/scripts/build-sdk-bindings-native.sh)"
runtime="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-ruby-sdk.XXXXXX")"
trap 'rm -rf "$runtime"' EXIT
cp -R "$bindings/cyclops_sdk" "$runtime/cyclops_sdk"
cp "$bindings/cyclops_sdk.rb" "$runtime/cyclops_sdk.rb"
cp "$library" "$runtime/cyclops_sdk/$(basename "$library")"
library_dir="$runtime/cyclops_sdk"
target="$1"
case "$target" in /*) ;; *) target="$repo_root/$target" ;; esac
shift
case "$(uname -s)" in
  Darwin) export DYLD_LIBRARY_PATH="$library_dir${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" ;;
  Linux) export LD_LIBRARY_PATH="$library_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
  MINGW*|MSYS*|CYGWIN*) export PATH="$library_dir:$PATH" ;;
  *) echo "Unsupported Ruby SDK host: $(uname -s)" >&2; exit 1 ;;
esac
RUBYLIB="$runtime:$bindings${RUBYLIB:+:$RUBYLIB}" ruby "$target" "$@"
