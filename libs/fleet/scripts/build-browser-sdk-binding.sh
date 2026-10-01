#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binding_dir="$repo_root/cyclops-cs/sdk-bindings/ts-uniffi-browser"

for command in cargo rustup npm; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "error: $command must be available on PATH" >&2
    exit 127
  fi
done

if ! rustup target list --installed | grep -qx 'wasm32-unknown-unknown'; then
  rustup target add wasm32-unknown-unknown
fi

wasm_bindgen_version="0.2.126"
if ! command -v wasm-bindgen >/dev/null 2>&1 || ! wasm-bindgen --version | grep -Fq "$wasm_bindgen_version"; then
  cargo install wasm-bindgen-cli --version "$wasm_bindgen_version" --locked
fi

npm ci --prefix "$binding_dir"
npm --prefix "$binding_dir" run build

for artifact in \
  "$binding_dir/ts/wasm-bindgen/index.js" \
  "$binding_dir/ts/wasm-bindgen/index_bg.wasm" \
  "$binding_dir/examples/dist/browser.js"; do
  if [ ! -s "$artifact" ]; then
    echo "error: expected generated browser SDK artifact is missing or empty: $artifact" >&2
    exit 1
  fi
  printf '%s\n' "$artifact"
done
