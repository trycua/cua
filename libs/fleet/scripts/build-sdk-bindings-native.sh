#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace="$repo_root/cyclops-cs"
target_dir="${CYCLOPS_SDK_NATIVE_TARGET_DIR:-$workspace/target/sdk-bindings-native}"

cargo build --locked --manifest-path "$workspace/Cargo.toml" --package cyclops-sdk --target-dir "$target_dir" >&2
case "$(uname -s)" in
  Darwin) library="$target_dir/debug/libcyclops_sdk.dylib" ;;
  Linux) library="$target_dir/debug/libcyclops_sdk.so" ;;
  *) echo "unsupported host for Cyclops SDK bindings: $(uname -s)" >&2; exit 1 ;;
esac
[ -f "$library" ] || { echo "native Cyclops SDK library was not built: $library" >&2; exit 1; }
printf '%s\n' "$library"
