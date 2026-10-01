#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binding_file="$repo_root/cyclops-cs/sdk-bindings/ts-uniffi-browser/ts/fleet_sdk.ts"

for symbol in AccessTokenProvider connectWithAccessTokenProvider connectWithAccessToken connectBrowserWithAccessToken creationTimestamp listNamespaces listUserApiKeys createUserApiKey deleteUserApiKey; do
  if ! grep -Fq "$symbol" "$binding_file"; then
    echo "error: generated browser SDK binding is missing $symbol: $binding_file" >&2
    exit 1
  fi
done

if grep -Fq "executeAuthenticated" "$binding_file"; then
  echo "error: generated browser SDK binding exports executeAuthenticated: $binding_file" >&2
  exit 1
fi
