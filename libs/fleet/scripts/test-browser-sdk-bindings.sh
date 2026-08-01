#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binding_file="$repo_root/cyclops-cs/sdk-bindings/ts-uniffi-browser/ts/cyclops_sdk.ts"

for symbol in AccessTokenProvider connectWithAccessTokenProvider connectWithAccessToken connectBrowserWithAccessToken; do
  if ! grep -Fq "$symbol" "$binding_file"; then
    echo "error: generated browser SDK binding is missing $symbol: $binding_file" >&2
    exit 1
  fi
done
