#!/usr/bin/env bash
# The shared Daytona suite against `cua compat serve` on real Spaces on THIS
# machine (a KVM runner, or a Mac with the Cua runtimes): Python sync, Python
# async and TypeScript, every lane (mock + space), compared with the golden.
#
#   libs/cua/tests/compat/daytona/run_space.sh [cua compat serve options...]
#   e.g. run_space.sh --on local --kind vm --image vm:/path/disk.img
#
# $CUA: the cua binary (default libs/cua/target/debug/cua). Every Space made is deleted.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../../.." && pwd)"
CUA="${CUA:-$REPO/libs/cua/target/debug/cua}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-daytona-space.XXXXXX")"
OUT="${OUT:-$WORK}"; mkdir -p "$OUT"
PINNED="$(cat "$HERE/PINNED")"
# Real Spaces: the preview, image-build and snapshot lanes need the real guest.
export COMPAT_REAL_GUEST=1
# Image builds and `image.create_from_image` ship cua-spacesd into the guest:
# the Linux binary must sit beside cua.
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) SD="cua-spacesd-linux-x86_64" ;;
    Linux-aarch64|Linux-arm64) SD="cua-spacesd-linux-aarch64" ;;
    *) SD="" ;;
esac
if [ -n "$SD" ] && [ ! -e "$(dirname "$CUA")/$SD" ]; then
    echo "setup: $SD is missing beside $CUA; image builds will fail. Build it (cargo build -p cua-spacesd from libs/cua-spacesd) and copy it next to cua." >&2
fi
"$CUA" compat serve --provider daytona "$@" >"$OUT/gateway.out" 2>"$OUT/gateway.log" &
GW=$!
cleanup() { kill "$GW" 2>/dev/null || true; wait "$GW" 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 3000); do grep -q "Clients export" "$OUT/gateway.out" 2>/dev/null && break; sleep 0.2; done
grep -q "Clients export" "$OUT/gateway.out" || { cat "$OUT/gateway.log"; exit 1; }
eval "$("$CUA" compat env --provider daytona)"
python3 - >"$WORK/env.json" <<'PY'
import json, os
print(json.dumps({k: os.environ[k] for k in ("DAYTONA_API_URL", "DAYTONA_API_KEY", "SSL_CERT_FILE", "NODE_EXTRA_CA_CERTS") if k in os.environ}))
PY
# the first Space of a fresh daemon boots cold: make it outside the checks
warm=$(curl -s --max-time 900 -X POST "$DAYTONA_API_URL/sandbox" -H "Authorization: Bearer $DAYTONA_API_KEY" -H 'Content-Type: application/json' -d '{}' || true)
wid=$(printf '%s' "$warm" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)
[ -z "$wid" ] || curl -s --max-time 300 -X DELETE "$DAYTONA_API_URL/sandbox/$wid" -H "Authorization: Bearer $DAYTONA_API_KEY" >/dev/null || true

venv="${CUA_COMPAT_VENVS:-$WORK}/venv-$PINNED"
[ -x "$venv/bin/python" ] || python3 -m venv "$venv"
"$venv/bin/pip" install -q "daytona==$PINNED" hypothesis
runs=(); status=0
for mode in sync async; do
    "$venv/bin/python" "$HERE/py/harness.py" --mode "$mode" --env "$WORK/env.json" --out "$OUT/py-$mode.json" 2>"$OUT/py-$mode.err" || { grep -E "^FAIL" -A8 "$OUT/py-$mode.err" | head -60; status=1; }
    runs+=("$OUT/py-$mode.json")
done
tsdir="${CUA_COMPAT_VENVS:-$WORK}/ts-$PINNED"
if [ ! -d "$tsdir/node_modules" ]; then mkdir -p "$tsdir" && cp "$HERE/ts/package.json" "$HERE/ts/package-lock.json" "$tsdir/" && (cd "$tsdir" && npm ci --no-audit --no-fund -q); fi
(cd "$tsdir" && cp "$HERE"/ts/*.ts "$HERE/ts/tsconfig.json" . && rm -rf cases && cp -r "$HERE/ts/cases" cases && npx tsx runner.ts --env "$WORK/env.json" --out "$OUT/ts.json" 2>"$OUT/ts.err") || { grep -E "^FAIL" -A8 "$OUT/ts.err" | head -60; status=1; }
runs+=("$OUT/ts.json")
python3 "$HERE/compare.py" "$HERE/fixtures/daytona-$PINNED.json" "${runs[@]}" --catalogue "$HERE/cases.json" || status=1
[ "$status" = 0 ] || { echo "--- gateway log"; tail -40 "$OUT/gateway.log"; }
exit "$status"
