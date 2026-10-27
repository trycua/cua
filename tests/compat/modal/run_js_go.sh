#!/usr/bin/env bash
# The JavaScript and Go SDKs against the Modal gateway, hermetic (mock Spaces):
#   tests/compat/modal/run_js_go.sh [js] [go]     (default: both)
# Needs node (>= 20) and go (>= 1.23); the gateway is built from this checkout.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-modal-sdks.XXXXXX")"
which=("$@"); [ ${#which[@]} -eq 0 ] && which=(js go)
(cd "$REPO/libs/cua" && cargo build --locked -q -p cua-compat --features testing --example mock_gateway)
GW="$REPO/libs/cua/target/debug/examples/mock_gateway"

mkdir -p "$WORK/www" && printf 'hello-from-port-8080' >"$WORK/www/index.html"
python3 - "$WORK/www" >"$WORK/port" <<'PY' &
import http.server, socketserver, sys, functools
h = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
h.log_message = lambda *a: None
s = socketserver.ThreadingTCPServer(("127.0.0.1", 0), h)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
PIDS=($!)
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
for _ in $(seq 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done

start_gateway() { # name, extra env...
    local name="$1"; shift
    env CUA_MODAL_BUILD_DRY_RUN=1 "$@" "$GW" --provider modal --dir "$WORK/$name" --port "8080=127.0.0.1:$(cat "$WORK/port")" \
        >"$WORK/$name.env.json" 2>"$WORK/$name.log" &
    PIDS+=($!)
    for _ in $(seq 100); do [ -s "$WORK/$name.env.json" ] && break; sleep 0.1; done
    [ -s "$WORK/$name.env.json" ] || { cat "$WORK/$name.log"; exit 1; }
}
start_gateway sandboxes
status=0

for sdk in "${which[@]}"; do
    case "$sdk" in
    js)
        cp -R "$HERE/js" "$WORK/js" && (cd "$WORK/js" && npm install --silent --no-audit --no-fund >/dev/null)
        # JS_MODAL=latest: the newest published SDK instead of the pinned one (drift lane).
        [ -z "${JS_MODAL:-}" ] || (cd "$WORK/js" && npm install --no-save --silent --no-audit --no-fund "modal@$JS_MODAL" >/dev/null)
        echo "== modal npm $(cd "$WORK/js" && node -e "console.log(JSON.parse(require('fs').readFileSync('node_modules/modal/package.json','utf8')).version)")"
        (cd "$WORK/js" && node hermetic.mjs "$WORK/sandboxes.env.json") || status=1
        # Functions: the Python client deploys an App whose Functions the JS client then calls.
        python3 -m venv "$WORK/venv" && "$WORK/venv/bin/pip" install -q "modal==$(cat "$HERE/PINNED")"
        start_gateway functions CUA_MODAL_SKIP_RUNTIME_LAYER=1 CUA_MODAL_LOCAL_RUNTIME="$WORK/venv/bin/python"
        cat >"$WORK/deploy.py" <<'PY'
import json, os, sys
os.environ.update(json.loads(open(sys.argv[1]).read()))
sys.path.insert(0, sys.argv[2])
import fn_app
fn_app.app.deploy(name="hermetic-deployed")
PY
        "$WORK/venv/bin/python" "$WORK/deploy.py" "$WORK/functions.env.json" "$HERE" >/dev/null
        (cd "$WORK/js" && node functions.mjs "$WORK/functions.env.json") || status=1
        ;;
    go)
        echo "== modal-client go $(grep -o 'modal-client/go v[0-9.]*' "$HERE/go/go.mod" | cut -d' ' -f2)"
        godir="$HERE/go"
        # GO_MODAL=latest (or a version): that module version instead of the pinned one (drift lane).
        if [ -n "${GO_MODAL:-}" ]; then
            cp -R "$HERE/go" "$WORK/go" && godir="$WORK/go"
            (cd "$godir" && go get "github.com/modal-labs/modal-client/go@$GO_MODAL" >/dev/null 2>&1 && go mod tidy >/dev/null 2>&1)
            echo "== latest: $(grep -o 'modal-client/go v[0-9.]*' "$godir/go.mod" | cut -d' ' -f2)"
        fi
        (cd "$godir" && go run . "$WORK/sandboxes.env.json") || status=1
        ;;
    esac
done
[ "$status" = 0 ] || { echo "--- gateway log"; tail -40 "$WORK/sandboxes.log"; }
exit "$status"
