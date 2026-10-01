#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cyclops_root="$repo_root/cyclops-cs"
binding_dir="$cyclops_root/sdk-bindings/ts-uniffi-browser"
stamp_dir="$cyclops_root/target/browser-sdk"
stamp_file="$stamp_dir/input.sha256"
stamp_existing=false

case "${1:-}" in
  "") ;;
  --stamp-existing) stamp_existing=true ;;
  *)
    echo "usage: $0 [--stamp-existing]" >&2
    exit 2
    ;;
esac

artifacts=(
  "$binding_dir/ts/index.web.ts"
  "$binding_dir/ts/wasm-bindgen/index.js"
  "$binding_dir/ts/wasm-bindgen/index_bg.wasm"
)

inputs_hash="$({
  find \
    "$cyclops_root/sdk/src" \
    "$cyclops_root/sdk-schema/src" \
    -type f -print0
  printf '%s\0' \
    "$cyclops_root/Cargo.lock" \
    "$cyclops_root/Cargo.toml" \
    "$cyclops_root/rust-toolchain.toml" \
    "$cyclops_root/bindgen-cli/main.rs" \
    "$binding_dir/package-lock.json" \
    "$binding_dir/ubrn.config.yaml" \
    "$binding_dir/scripts/patch-ubrn-wasm-template.mjs" \
    "$cyclops_root/scripts/build-browser-sdk-binding.sh"
} | sort -z | xargs -0 sha256sum | sha256sum | cut -d' ' -f1)"

artifacts_ready=true
for artifact in "${artifacts[@]}"; do
  if [ ! -s "$artifact" ]; then
    artifacts_ready=false
    break
  fi
done

stamp_matches=false
if [ -f "$stamp_file" ] && [ "$(cat "$stamp_file")" = "$inputs_hash" ]; then
  stamp_matches=true
fi

if [ "$stamp_existing" = true ]; then
  if [ "$artifacts_ready" != true ]; then
    echo "error: cannot stamp missing browser SDK artifacts" >&2
    exit 1
  fi
  mkdir -p "$stamp_dir"
  printf '%s\n' "$inputs_hash" > "$stamp_file"
  exit 0
fi

if [ "$artifacts_ready" = true ] && [ "$stamp_matches" = true ]; then
  exit 0
fi

"$cyclops_root/scripts/build-browser-sdk-binding.sh"
mkdir -p "$stamp_dir"
printf '%s\n' "$inputs_hash" > "$stamp_file"
