#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binding_dir="$repo_root/cyclops-cs/sdk-bindings/ts-uniffi-browser"

for variable in CUA_CLIENT_ID CUA_CLIENT_SECRET CUA_TOKEN_URL CUA_BASE_URL CYCLOPS_NAMESPACE CUA_IMAGE CUA_IMAGE_PULL_SECRET; do
  if [ -z "${!variable:-}" ]; then
    echo "error: missing required environment variable: $variable" >&2
    exit 1
  fi
done

token_json="$(curl --fail --silent --show-error \
  --request POST \
  --url "$CUA_TOKEN_URL" \
  --header 'accept: application/json' \
  --header 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=client_credentials' \
  --data-urlencode "client_id=$CUA_CLIENT_ID" \
  --data-urlencode "client_secret=$CUA_CLIENT_SECRET")"
CYCLOPS_ACCESS_TOKEN="$(jq -er '.access_token | select(type == "string" and length > 0)' <<<"$token_json")"
export CYCLOPS_ACCESS_TOKEN

if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
  echo "::add-mask::$CYCLOPS_ACCESS_TOKEN"
fi

"$repo_root/cyclops-cs/scripts/build-browser-sdk-binding.sh"
(
  cd "$binding_dir"
  ./node_modules/.bin/playwright test \
    --project=chromium \
    tests/browser-artifact.spec.ts \
    tests/browser-lifecycle.spec.ts \
    tests/browser-live.spec.ts
)
