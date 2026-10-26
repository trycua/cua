#!/usr/bin/env bash
# The unmodified Daytona SDKs (Python sync and async, TypeScript; pinned and
# latest) against the cua-compat gateway over mock Spaces: simulated guest
# processes, in-memory files, nothing runs on the host. Runs the `mock` cases of
# the shared suite and compares the three runners with the derived golden.
#
#   libs/cua/tests/compat/daytona/run_hermetic.sh [--latest]   (--latest: also the newest SDKs)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../../.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-daytona-hermetic.XXXXXX")"
PINNED="$(cat "$HERE/PINNED")"
specs=("$PINNED")
[ "${1:-}" = "--latest" ] && specs+=(latest)

# CARGO_WRAP: a prefix for heavy builds (the laptop throttle: build-slot.sh).
(cd "$REPO/libs/cua" && ${CARGO_WRAP:-} cargo build --locked -q -p cua-compat --features testing --example mock_gateway)
GW="$REPO/libs/cua/target/debug/examples/mock_gateway"

# The stand-in for a guest server on port 8080.
mkdir -p "$WORK/www" && printf 'hello-from-port-8080' >"$WORK/www/index.html"
python3 - "$WORK/www" >"$WORK/port" <<'EOF' &
import http.server, socketserver, sys, functools
h = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
h.log_message = lambda *a: None
s = socketserver.ThreadingTCPServer(("127.0.0.1", 0), h)
print(s.server_address[1], flush=True)
s.serve_forever()
EOF
WWW=$!
cleanup() { kill "$WWW" "${GWPID:-}" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
for _ in $(seq 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
PORT="$(cat "$WORK/port")"

"$GW" --provider daytona --dir "$WORK/gw" --port "8080=127.0.0.1:$PORT" >"$WORK/env.json" 2>"$WORK/gw.log" &
GWPID=$!
for _ in $(seq 100); do [ -s "$WORK/env.json" ] && break; sleep 0.1; done
[ -s "$WORK/env.json" ] || { cat "$WORK/gw.log"; exit 1; }

# The Python SDK's S3 client (obstore) ignores SSL_CERT_FILE: its Image
# context uploads need the gateway CA in the OS trust store. On a Linux CI
# runner with sudo that is installed here; elsewhere the case is skipped.
CA="$(find "$WORK/gw" -name ca.pem 2>/dev/null | head -1)"
if [ "$(uname -s)" = Linux ] && [ -n "$CA" ] && sudo -n true 2>/dev/null; then
    sudo cp "$CA" /usr/local/share/ca-certificates/cua-compat-hermetic.crt && sudo update-ca-certificates >/dev/null && export CUA_COMPAT_OS_TRUSTS_CA=1
fi

status=0
for spec in "${specs[@]}"; do
    if [ "$spec" = latest ]; then pyspec="daytona"; tsspec="@daytonaio/sdk@latest"; else pyspec="daytona==$spec"; tsspec="@daytonaio/sdk@$spec"; fi
    venv="${CUA_COMPAT_VENVS:-$WORK}/venv-${spec//[^a-zA-Z0-9]/_}"
    [ -x "$venv/bin/python" ] || python3 -m venv "$venv"
    "$venv/bin/pip" install -q --upgrade "$pyspec"
    tsdir="${CUA_COMPAT_VENVS:-$WORK}/ts-${spec//[^a-zA-Z0-9]/_}"
    if [ ! -d "$tsdir/node_modules" ]; then
        mkdir -p "$tsdir" && cp "$HERE/ts/package.json" "$tsdir/" && cp "$HERE/ts/package-lock.json" "$tsdir/" 2>/dev/null || true
        (cd "$tsdir" && if [ "$spec" = latest ]; then npm install --no-audit --no-fund -q "$tsspec"; else npm ci --no-audit --no-fund -q; fi)
    fi
    echo "== SDK $spec"
    runs=()
    for mode in sync async; do
        "$venv/bin/python" "$HERE/py/harness.py" --mode "$mode" --env "$WORK/env.json" --needs mock,hermetic --out "$WORK/py-$mode-$spec.json" 2>"$WORK/py-$mode-$spec.err" || { grep -E "^(FAIL|[0-9]+ passed)" -A6 "$WORK/py-$mode-$spec.err" | head -40; status=1; }
        runs+=("$WORK/py-$mode-$spec.json")
    done
    # The TypeScript runner lives next to its dependencies in $tsdir.
    # Node reads NODE_EXTRA_CA_CERTS at start-up, not from the runner.
    export NODE_EXTRA_CA_CERTS="$(python3 -c "import json;print(json.load(open('$WORK/env.json')).get('NODE_EXTRA_CA_CERTS',''))")"
    (cd "$tsdir" && cp "$HERE"/ts/*.ts "$HERE/ts/tsconfig.json" . && rm -rf cases && cp -r "$HERE/ts/cases" cases && npx tsx runner.ts --env "$WORK/env.json" --needs mock,hermetic --out "$WORK/ts-$spec.json" 2>"$WORK/ts-$spec.err") || { grep -E "^(FAIL|[0-9]+ passed)" -A6 "$WORK/ts-$spec.err" | head -40; status=1; }
    runs+=("$WORK/ts-$spec.json")
    # Property tests of the wire translation (bounded) and the overhead guard.
    "$venv/bin/pip" install -q hypothesis
    HYPOTHESIS_MAX_EXAMPLES="${HYPOTHESIS_MAX_EXAMPLES:-15}" "$venv/bin/python" "$HERE/py/props.py" --env "$WORK/env.json" || status=1
    if [ "${PERF_CHECK:-}" = 1 ]; then
        python3 "$HERE/perf/overhead.py" --env "$WORK/env.json" --check "$HERE/perf/baseline.json" >"$WORK/perf.json" || { cat "$WORK/perf.json"; status=1; }
    fi
    golden="$HERE/fixtures/daytona-$PINNED.json"
    python3 "$HERE/compare.py" "$golden" "${runs[@]}" --catalogue "$HERE/cases.json" --needs mock,hermetic ${GOLDEN_UPDATE:+--update} || status=1
done
[ "$status" = 0 ] || { echo "--- gateway log"; tail -50 "$WORK/gw.log"; }
exit "$status"
