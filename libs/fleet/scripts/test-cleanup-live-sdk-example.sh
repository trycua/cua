#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-live-cleanup-test.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

cat > "$temporary/curl" <<'FAKE_CURL'
#!/usr/bin/env bash
set -euo pipefail
log="${CLEANUP_TEST_LOG:?}"
output_file=""
method="GET"
url=""
while (($#)); do
  case "$1" in
    -o|--output) output_file="$2"; shift 2 ;;
    -X) method="$2"; shift 2 ;;
    --write-out) shift 2 ;;
    -H|-d|--data-urlencode) shift 2 ;;
    --fail-with-body|--silent|--show-error) shift ;;
    http*) url="$1"; shift ;;
    *) shift ;;
  esac
done
printf '%s %s\n' "$method" "$url" >> "$log"
if [[ "$url" == *'/protocol/openid-connect/token' ]]; then
  printf '{"access_token":"test-token"}\n'
  exit 0
fi
if [[ "$method" == DELETE ]]; then
  if [[ "$url" == */api/namespaces/* && -n "${CLEANUP_NAMESPACE_DELETE_FAILURES:-}" ]]; then
    counter_file="$log.namespace-delete-attempts"
    attempts=$(( $(cat "$counter_file" 2>/dev/null || echo 0) + 1 ))
    printf '%s' "$attempts" > "$counter_file"
    if (( attempts <= CLEANUP_NAMESPACE_DELETE_FAILURES )); then
      printf '502'
      exit 0
    fi
  fi
  printf '204'
  exit 0
fi
case "$url" in
  */osgymsandboxclaims|*/osgymworkspacepools)
    if [[ "${CLEANUP_LIST_STATUS:-200}" != 200 ]]; then
      printf '%s' "$CLEANUP_LIST_STATUS"
      exit 0
    fi
    if [[ "$url" == */osgymsandboxclaims ]]; then
      printf '{"items":[{"metadata":{"name":"claim-a"}}]}' > "$output_file"
    else
      printf '{"items":[{"metadata":{"name":"pool-a"}}]}' > "$output_file"
    fi
    ;;
  *)
    printf '{"items":[]}' > "$output_file"
    ;;
esac
printf '200'
FAKE_CURL
chmod +x "$temporary/curl"

export CLEANUP_TEST_LOG="$temporary/requests.log"
export CURL_BIN="$temporary/curl"
export CUA_BASE_URL="https://run.example"
export CUA_TOKEN_URL="https://auth.example/protocol/openid-connect/token"
export CUA_CLIENT_ID="client-id"
export CUA_CLIENT_SECRET="client-secret"
export CYCLOPS_NAMESPACE="sdk-example-python-1-1"

"$repo_root/cyclops-cs/scripts/cleanup-live-sdk-example.sh"

cat > "$temporary/expected.log" <<'EXPECTED'
POST https://auth.example/protocol/openid-connect/token
GET https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/sdk-example-python-1-1/osgymsandboxclaims
DELETE https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/sdk-example-python-1-1/osgymsandboxclaims/claim-a
GET https://run.example/api/k8s/apis/cua.ai/v1/namespaces/sdk-example-python-1-1/osgymworkspacepools
DELETE https://run.example/api/k8s/apis/cua.ai/v1/namespaces/sdk-example-python-1-1/osgymworkspacepools/pool-a
DELETE https://run.example/api/namespaces/sdk-example-python-1-1
EXPECTED

diff -u "$temporary/expected.log" "$CLEANUP_TEST_LOG"
printf 'live SDK cleanup regression checks passed.\n'

: > "$CLEANUP_TEST_LOG"
export CLEANUP_LIST_STATUS=403
"$repo_root/cyclops-cs/scripts/cleanup-live-sdk-example.sh"
unset CLEANUP_LIST_STATUS
cat > "$temporary/forbidden-expected.log" <<'FORBIDDEN_EXPECTED'
POST https://auth.example/protocol/openid-connect/token
GET https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/sdk-example-python-1-1/osgymsandboxclaims
GET https://run.example/api/k8s/apis/cua.ai/v1/namespaces/sdk-example-python-1-1/osgymworkspacepools
DELETE https://run.example/api/namespaces/sdk-example-python-1-1
FORBIDDEN_EXPECTED
diff -u "$temporary/forbidden-expected.log" "$CLEANUP_TEST_LOG"

: > "$CLEANUP_TEST_LOG"
rm -f "$CLEANUP_TEST_LOG.namespace-delete-attempts"
export CLEANUP_RETRY_DELAY_SECONDS=0
export CLEANUP_NAMESPACE_DELETE_FAILURES=2
"$repo_root/cyclops-cs/scripts/cleanup-live-sdk-example.sh"
cat > "$temporary/retry-expected.log" <<'RETRY_EXPECTED'
POST https://auth.example/protocol/openid-connect/token
GET https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/sdk-example-python-1-1/osgymsandboxclaims
DELETE https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/sdk-example-python-1-1/osgymsandboxclaims/claim-a
GET https://run.example/api/k8s/apis/cua.ai/v1/namespaces/sdk-example-python-1-1/osgymworkspacepools
DELETE https://run.example/api/k8s/apis/cua.ai/v1/namespaces/sdk-example-python-1-1/osgymworkspacepools/pool-a
DELETE https://run.example/api/namespaces/sdk-example-python-1-1
DELETE https://run.example/api/namespaces/sdk-example-python-1-1
DELETE https://run.example/api/namespaces/sdk-example-python-1-1
RETRY_EXPECTED
diff -u "$temporary/retry-expected.log" "$CLEANUP_TEST_LOG"

: > "$CLEANUP_TEST_LOG"
rm -f "$CLEANUP_TEST_LOG.namespace-delete-attempts"
export CLEANUP_NAMESPACE_DELETE_FAILURES=10
if "$repo_root/cyclops-cs/scripts/cleanup-live-sdk-example.sh" 2> "$temporary/persistent-stderr.log"; then
  echo "expected cleanup to fail when the namespace delete keeps returning 502" >&2
  exit 1
fi
grep -q 'Failed to delete https://run.example/api/namespaces/sdk-example-python-1-1: HTTP 502' "$temporary/persistent-stderr.log"
[[ "$(grep -c '^DELETE https://run.example/api/namespaces/sdk-example-python-1-1$' "$CLEANUP_TEST_LOG")" == 5 ]]
unset CLEANUP_NAMESPACE_DELETE_FAILURES CLEANUP_RETRY_DELAY_SECONDS
printf 'live SDK cleanup retry checks passed.\n'
