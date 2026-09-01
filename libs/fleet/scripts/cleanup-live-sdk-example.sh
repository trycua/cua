#!/usr/bin/env bash
set -euo pipefail

curl_bin="${CURL_BIN:-curl}"
for variable in CUA_BASE_URL CUA_TOKEN_URL CUA_CLIENT_ID CUA_CLIENT_SECRET CYCLOPS_NAMESPACE; do
  [[ -n "${!variable:-}" ]] || { echo "$variable is required" >&2; exit 1; }
done

token="$($curl_bin --fail-with-body --silent --show-error -X POST "$CUA_TOKEN_URL" \
  -d grant_type=client_credentials \
  --data-urlencode "client_id=$CUA_CLIENT_ID" \
  --data-urlencode "client_secret=$CUA_CLIENT_SECRET" | jq -er .access_token)"
cleanup_failed=0
retry_attempts=5
retry_delay="${CLEANUP_RETRY_DELAY_SECONDS:-5}"

transient_status() {
  case "$1" in
    000|5??) return 0 ;;
    *) return 1 ;;
  esac
}

delete_url() {
  local url="$1" status attempt
  for attempt in $(seq 1 "$retry_attempts"); do
    status="$($curl_bin --silent --show-error -o /dev/null --write-out '%{http_code}' \
      -H "Authorization: Bearer $token" -X DELETE "$url")" || status=000
    if ! transient_status "$status" || (( attempt >= retry_attempts )); then
      break
    fi
    sleep "$retry_delay"
  done
  case "$status" in
    200|202|204|404) ;;
    *) echo "Failed to delete $url: HTTP $status" >&2; cleanup_failed=1 ;;
  esac
}

delete_collection() {
  local collection="$1" body_file status attempt
  body_file="$(mktemp)"
  for attempt in $(seq 1 "$retry_attempts"); do
    status="$($curl_bin --silent --show-error -o "$body_file" --write-out '%{http_code}' \
      -H "Authorization: Bearer $token" "$collection")" || status=000
    if ! transient_status "$status" || (( attempt >= retry_attempts )); then
      break
    fi
    sleep "$retry_delay"
  done
  case "$status" in
    200)
      while IFS= read -r name; do
        [[ -n "$name" ]] && delete_url "$collection/$name"
      done < <(jq -r '.items[]?.metadata.name' "$body_file")
      ;;
    403|404) ;;
    *) echo "Failed to list $collection: HTTP $status" >&2; cleanup_failed=1 ;;
  esac
  rm -f "$body_file"
}

claims="$CUA_BASE_URL/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/$CYCLOPS_NAMESPACE/osgymsandboxclaims"
pools="$CUA_BASE_URL/api/k8s/apis/cua.ai/v1/namespaces/$CYCLOPS_NAMESPACE/osgymworkspacepools"
delete_collection "$claims"
delete_collection "$pools"
delete_url "$CUA_BASE_URL/api/namespaces/$CYCLOPS_NAMESPACE"
exit "$cleanup_failed"
