#!/usr/bin/env bash
# Hermetic SDK suites: a provider's unmodified SDK against the cua-compat
# gateway over mock Spaces (no VMs; nothing runs on the host). Runs the
# pinned SDK version (<provider>/PINNED) and the latest.
#
#   tests/compat/run_hermetic.sh e2b|daytona|modal [SDK_SPEC ...]
set -euo pipefail
PROVIDER="${1:?provider (e2b, daytona or modal)}"; shift
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HERE="$ROOT/$PROVIDER"
REPO="$(cd "$ROOT/../.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-compat-hermetic.XXXXXX")"
specs=("$@")
[ ${#specs[@]} -eq 0 ] && specs=("$PROVIDER==$(cat "$HERE/PINNED")" "$PROVIDER")

(cd "$REPO/libs/cua" && cargo build --locked -q -p cua-compat --features testing --example mock_gateway)
GW="$REPO/libs/cua/target/debug/examples/mock_gateway"

# The stand-in for a guest server on port 8080.
mkdir -p "$WORK/www" && printf 'hello-from-port-8080' >"$WORK/www/index.html"
python3 - "$WORK/www" >"$WORK/port" <<'EOF' &
import http.server, socketserver, sys, functools, os
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

CUA_MODAL_BUILD_DRY_RUN=1 "$GW" --provider "$PROVIDER" --dir "$WORK/gw" --port "8080=127.0.0.1:$PORT" >"$WORK/env.json" 2>"$WORK/gw.log" &
GWPID=$!
for _ in $(seq 100); do [ -s "$WORK/env.json" ] && break; sleep 0.1; done
[ -s "$WORK/env.json" ] || { cat "$WORK/gw.log"; exit 1; }

status=0
for spec in "${specs[@]}"; do
    venv="${CUA_COMPAT_VENVS:-$WORK}/venv-${spec//[^a-zA-Z0-9]/_}"
    [ -x "$venv/bin/python" ] || python3 -m venv "$venv"
    "$venv/bin/pip" install -q --upgrade "$spec"
    echo "== $spec"
    "$venv/bin/python" "$HERE/hermetic_sdk.py" "$WORK/env.json" || status=1
    if [ "$PROVIDER" = modal ]; then
        "$venv/bin/pip" install -q python-dotenv
        for extra in sandbox_extra_sdk.py objects_sdk.py; do
            "$venv/bin/python" "$HERE/$extra" "$WORK/env.json" || status=1
        done
        # The overhead guard (the gateway's own time per call, against ClientHello).
        if [ "${PERF_CHECK:-}" = 1 ]; then
            "$venv/bin/python" "$HERE/perf/overhead.py" "$WORK/env.json" --check "$HERE/perf/baseline.json" || status=1
        fi
        # Functions: a second gateway whose containers are Modal's own
        # entrypoint, run as host processes by this SDK's python.
        FWORK="$WORK/fn-${spec//[^a-zA-Z0-9]/_}"; mkdir -p "$FWORK"
        CUA_MODAL_BUILD_DRY_RUN=1 CUA_MODAL_SKIP_RUNTIME_LAYER=1 CUA_MODAL_LOCAL_RUNTIME="$venv/bin/python" \
            "$GW" --provider modal --dir "$FWORK/gw" >"$FWORK/env.json" 2>"$FWORK/gw.log" &
        FGW=$!
        for _ in $(seq 100); do [ -s "$FWORK/env.json" ] && break; sleep 0.1; done
        "$venv/bin/python" "$HERE/functions_sdk.py" "$FWORK/env.json" || { status=1; tail -30 "$FWORK/gw.log"; }
        kill "$FGW" 2>/dev/null || true
    fi
done
[ "$status" = 0 ] || { echo "--- gateway log"; tail -50 "$WORK/gw.log"; }
exit "$status"
