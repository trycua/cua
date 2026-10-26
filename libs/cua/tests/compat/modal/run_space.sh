#!/usr/bin/env bash
# The Modal Python SDK against `cua compat serve --provider modal` on real
# Spaces: the conformance cases compared with the golden derived from the
# SDK's sources, and the Function suite with containers that are Spaces.
#
#   libs/cua/tests/compat/modal/run_space.sh [cua compat serve options...]
#   e.g. libs/cua/tests/compat/modal/run_space.sh --on local --kind vm --image vm:/path/disk.img
#
# Needs a running cua daemon (or none: the gateway embeds a runtime). Uses
# $CUA (default libs/cua/target/debug/cua). SDK specs: $SDKS (default: the
# pinned version and the latest). Every Space and checkpoint the run makes
# is deleted.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$ROOT/../../../../.." && pwd)"
CUA="${CUA:-$REPO/libs/cua/target/debug/cua}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-modal-space.XXXXXX")"
OUT="${OUT:-$WORK}"
mkdir -p "$OUT"
read -r -a specs <<<"${SDKS:-modal==$(cat "$ROOT/PINNED") modal}"

"$CUA" compat serve --provider modal "$@" >"$OUT/gateway.out" 2>"$OUT/gateway.log" &
GW=$!
cleanup() {
    kill "$GW" 2>/dev/null || true
    wait "$GW" 2>/dev/null || true
    # Leftovers: every Space and checkpoint the gateway made.
    for n in $("$CUA" sandbox list --json 2>/dev/null | python3 -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: d=[]
items = d if isinstance(d,list) else d.get("sandboxes",[])
print(" ".join(x.get("name","") for x in items if x.get("name","").startswith("modal-")))'); do
        "$CUA" sandbox delete -f "$n" >/dev/null 2>&1 || true
    done
    for c in $("$CUA" sandbox checkpoints --json 2>/dev/null | python3 -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: d=[]
d = d if isinstance(d,list) else d.get("checkpoints",[])
print(" ".join(x["name"] for x in d if x["name"].startswith("modal-")))'); do
        "$CUA" sandbox checkpoints rm "$c" >/dev/null 2>&1 || true
    done
}
trap cleanup EXIT
for _ in $(seq 3000); do grep -q "Clients export" "$OUT/gateway.out" 2>/dev/null && break; sleep 0.2; done
grep -q "Clients export" "$OUT/gateway.out" || { cat "$OUT/gateway.log"; exit 1; }
eval "$("$CUA" compat env --provider modal)"

status=0
for spec in "${specs[@]}"; do
    venv="${CUA_COMPAT_VENVS:-$WORK}/venv-${spec//[^a-zA-Z0-9]/_}"
    [ -x "$venv/bin/python" ] || python3 -m venv "$venv"
    "$venv/bin/pip" install -q --upgrade "$spec"
    version=$("$venv/bin/python" -c "import importlib.metadata as m; print(m.version('modal'))")
    echo "== modal $version"
    run="$OUT/run-modal-$version.json"
    "$venv/bin/python" "$ROOT/conformance.py" >"$run" 2>"$run.err" || { tail -20 "$run.err"; status=1; continue; }
    grep -E "^(PASS|FAIL)" "$run.err" || true
    fixture="$ROOT/fixtures/modal-$version-derived.json"
    [ -f "$fixture" ] || fixture=$(ls "$ROOT"/fixtures/modal-*-derived.json | sort -V | tail -1)
    python3 "$ROOT/compare.py" "$fixture" "$run" || status=1
    # Functions with containers that are Spaces.
    FUNCTIONS_LANE=space "$venv/bin/python" "$ROOT/functions_sdk.py" >"$OUT/functions-$version.out" 2>"$OUT/functions-$version.err" || {
        grep -E "^(PASS|FAIL|[0-9]+ passed)" "$OUT/functions-$version.out" | tail -20; status=1; continue; }
    grep -E "^[0-9]+ passed" "$OUT/functions-$version.out"
done
[ "$status" = 0 ] || { echo "--- gateway log"; tail -40 "$OUT/gateway.log"; }
exit "$status"
