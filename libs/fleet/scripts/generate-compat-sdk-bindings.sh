#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: generate-compat-sdk-bindings.sh [--check]" >&2
}

case "${1:-}" in
  "") check_only=false ;;
  --check) check_only=true ;;
  *) usage; exit 2 ;;
esac
[ "$#" -le 1 ] || { usage; exit 2; }

if ! command -v gofmt >/dev/null 2>&1; then
  echo "error: gofmt is required to normalize compatibility Go bindings; install Go (for example, with actions/setup-go) and ensure gofmt is on PATH" >&2
  exit 127
fi

required_go_generator_version="uniffi-bindgen 0.7.1+v0.31.0"
if ! command -v uniffi-bindgen-go >/dev/null 2>&1; then
  echo "error: uniffi-bindgen-go $required_go_generator_version is required; install it with: CARGO_PROFILE_DEV_DEBUG=0 cargo install --debug --git https://github.com/NordSecurity/uniffi-bindgen-go.git --tag 'v0.7.1+v0.31.0' --locked uniffi-bindgen-go" >&2
  exit 127
fi
actual_go_generator_version="$(uniffi-bindgen-go --version)"
if [ "$actual_go_generator_version" != "$required_go_generator_version" ]; then
  echo "error: expected uniffi-bindgen-go $required_go_generator_version, found $actual_go_generator_version" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cyclops_root="$repo_root/cyclops-cs"
normalizer="$cyclops_root/scripts/normalize-compat-sdk-bindings.py"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-compat-bindings.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

library="$("$cyclops_root/scripts/build-sdk-bindings-native.sh")"
(
  cd "$cyclops_root/sdk"
  uniffi-bindgen-go --library "$library" --out-dir "$temporary/go" --no-format
  npx --yes --package=uniffi-bindgen-react-native@0.31.0-3 ubrn generate napi bindings \
    --library --no-format --ts-dir "$temporary/node" --lib-colocated "$library"
)

arguments=(--raw-go "$temporary/go/cyclops_sdk_schema/cyclops_sdk_schema.go" --raw-node "$temporary/node/cyclops_sdk_schema.ts")
if [ "$check_only" = true ]; then
  arguments+=(--check)
fi
"$normalizer" "${arguments[@]}"
