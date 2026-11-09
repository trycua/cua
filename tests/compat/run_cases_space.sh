#!/usr/bin/env bash
# The shared cases against `cua compat serve` on real Spaces (a Linux vm by
# default), compared with the recordings from the real E2B.
#
#   tests/compat/run_cases_space.sh [run_cases.sh options] -- [cua compat serve options]
#
# Needs a running cua daemon (or none: the gateway embeds a runtime). Uses
# $CUA (default libs/cua/target/debug/cua). Every Space the run makes is
# deleted when the gateway exits.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
CUA="${CUA:-$REPO/libs/cua/target/debug/cua}"
OPTS=(); SERVE=()
while [ $# -gt 0 ]; do [ "$1" = -- ] && { shift; SERVE=("$@"); break; }; OPTS+=("$1"); shift; done
[ ${#SERVE[@]} -eq 0 ] && SERVE=(--on local --kind vm --cpus 2 --memory-mb 2048)
WORK="${CASES_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/cua-cases-space.XXXXXX")}"
mkdir -p "$WORK"
# Ports: $CUA_COMPAT_LISTEN / $CUA_COMPAT_HOST_PORT (a shared machine may have another gateway on the defaults).
EXTRA_SERVE=()
[ -n "${CUA_COMPAT_LISTEN:-}" ] && EXTRA_SERVE+=(--listen "$CUA_COMPAT_LISTEN")
[ -n "${CUA_COMPAT_HOST_PORT:-}" ] && EXTRA_SERVE+=(--host-port "$CUA_COMPAT_HOST_PORT")
"$CUA" compat serve --provider e2b ${EXTRA_SERVE[@]+"${EXTRA_SERVE[@]}"} "${SERVE[@]}" >"$WORK/gateway.out" 2>"$WORK/gateway.log" &
GW=$!
cleanup() { kill "$GW" 2>/dev/null || true; wait "$GW" 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 3000); do grep -q "Clients export" "$WORK/gateway.out" 2>/dev/null && break; sleep 0.2; done
grep -q "Clients export" "$WORK/gateway.out" || { cat "$WORK/gateway.log"; exit 1; }
eval "$("$CUA" compat env --provider e2b ${EXTRA_SERVE[@]+"${EXTRA_SERVE[@]}"})"
# The first Space of a fresh daemon boots cold: make it here, outside the checks.
warm=$(curl -s --max-time 600 -X POST "$E2B_API_URL/sandboxes" -H "X-API-KEY: $E2B_API_KEY" \
    -H 'Content-Type: application/json' -d '{"templateID":"base","timeout":120}' || true)
id=$(printf '%s' "$warm" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("sandboxID",""))' 2>/dev/null || true)
[ -z "$id" ] || curl -s --max-time 120 -X DELETE "$E2B_API_URL/sandboxes/$id" -H "X-API-KEY: $E2B_API_KEY" >/dev/null || true
export NODE_EXTRA_CA_CERTS="${SSL_CERT_FILE:-}"
status=0
CASES_WORK="$WORK" "$ROOT/run_cases.sh" ${OPTS[@]+"${OPTS[@]}"} || status=$?
# Anything else to check against this gateway ($CASES_AFTER, run with the
# gateway's environment), for example the code-interpreter check.
if [ -n "${CASES_AFTER:-}" ]; then bash -c "$CASES_AFTER" || status=1; fi
[ "$status" = 0 ] || { echo "--- gateway log"; tail -40 "$WORK/gateway.log"; }
echo "outputs in $WORK"
exit $status
