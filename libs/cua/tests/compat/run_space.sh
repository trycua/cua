#!/usr/bin/env bash
# A provider's unmodified SDK against `cua compat serve` on real Spaces,
# compared with the fixtures recorded against the real provider.
#
#   libs/cua/tests/compat/run_space.sh e2b|daytona [cua compat serve options...]
#   e.g. libs/cua/tests/compat/run_space.sh e2b --on local --kind vm --image vm:/path/disk.img
#
# Needs a running cua daemon (or none: the gateway embeds a runtime). Uses
# $CUA (default libs/cua/target/debug/cua). SDK specs: $SDKS (default: the
# pinned version and the latest). Every Space the run makes is deleted.
set -euo pipefail
PROVIDER="${1:?provider (e2b or daytona)}"; shift
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HERE="$ROOT/$PROVIDER"
REPO="$(cd "$ROOT/../../../.." && pwd)"
CUA="${CUA:-$REPO/libs/cua/target/debug/cua}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-compat-space.XXXXXX")"
OUT="${OUT:-$WORK}"
mkdir -p "$OUT"
read -r -a specs <<<"${SDKS:-$PROVIDER==$(cat "$HERE/PINNED") $PROVIDER}"

"$CUA" compat serve --provider "$PROVIDER" "$@" >"$OUT/gateway.out" 2>"$OUT/gateway.log" &
GW=$!
cleanup() {
    kill "$GW" 2>/dev/null || true
    wait "$GW" 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 3000); do grep -q "Clients export" "$OUT/gateway.out" 2>/dev/null && break; sleep 0.2; done
grep -q "Clients export" "$OUT/gateway.out" || { cat "$OUT/gateway.log"; exit 1; }
eval "$("$CUA" compat env --provider "$PROVIDER")"

# The first Space of a fresh daemon boots cold and can outlast an SDK's
# default request timeout (60 s): make it here, outside the checks.
if [ "$PROVIDER" = e2b ]; then
    warm=$(curl -s --max-time 600 -X POST "$E2B_API_URL/sandboxes" -H "X-API-KEY: $E2B_API_KEY" \
        -H 'Content-Type: application/json' -d '{"templateID":"base","timeout":120}' || true)
    id=$(printf '%s' "$warm" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("sandboxID",""))' 2>/dev/null || true)
    [ -z "$id" ] || curl -s --max-time 120 -X DELETE "$E2B_API_URL/sandboxes/$id" -H "X-API-KEY: $E2B_API_KEY" >/dev/null || true
fi

status=0
for spec in "${specs[@]}"; do
    venv="${CUA_COMPAT_VENVS:-$WORK}/venv-${spec//[^a-zA-Z0-9]/_}"
    [ -x "$venv/bin/python" ] || python3 -m venv "$venv"
    "$venv/bin/pip" install -q --upgrade "$spec"
    version=$("$venv/bin/python" -c "import importlib.metadata as m; print(m.version('$PROVIDER'))")
    echo "== $PROVIDER $version"
    run="$OUT/run-$PROVIDER-$version.json"
    "$venv/bin/python" "$HERE/conformance.py" >"$run" 2>"$run.err" || { tail -20 "$run.err"; status=1; continue; }
    # Providers without real-provider fixtures assert in the suite itself.
    [ -d "$HERE/fixtures" ] || { grep -E "^(PASS|FAIL)" "$run.err"; continue; }
    fixture="$HERE/fixtures/$PROVIDER-$version.json"
    [ -f "$fixture" ] || fixture=$(ls "$HERE"/fixtures/"$PROVIDER"-*.json | sort -V | tail -1)
    python3 "$HERE/compare.py" "$fixture" "$run" --rules "$HERE/rules-space.json" || status=1
done
[ "$status" = 0 ] || { echo "--- gateway log"; tail -40 "$OUT/gateway.log"; }
exit "$status"
