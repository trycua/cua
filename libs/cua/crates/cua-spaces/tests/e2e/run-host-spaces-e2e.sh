#!/usr/bin/env bash
# "Spin up Spaces on my spare machine", end to end, on this machine:
#
#   fake OIDC issuer + cua-relay (account mode, devices enforced: no grace)
#   host:   `cua host setup --profile spare` (a real cua-spacesd joined to
#           the relay with its desktop services off, the process runner)
#           plus the host's cua daemon, which the driver starts on the
#           first request and which creates Docker Linux Spaces
#   client: an enrolled device's cua daemon, driven through `cua mcp`
#           (what a coding agent does) and the `cua spaces` CLI
#
# Asserts: create (count 2, the host named by words of its name), list
# (grouped under the host), a command inside a provided Space, the host's
# own shell refused (its desktop is not shared), delete, the host's audit,
# and the relay refusing a device that is not enrolled.
#
# macOS VMs cannot run in CI and are never created here: the provided
# runtime is Docker (Linux containers). Every Space, daemon, relay and temp
# home is removed on exit; nothing touches ~/.cua, the Keychain or a real
# `cua host` setup.
#
#   run-host-spaces-e2e.sh [--skip-build] [--keep] [--image REF]
#
# Needs Docker, and the Linux cua-spacesd from
# libs/cua-spacesd/scripts/ci/build-linux-e2e-bins.sh (run unless
# --skip-build) for the Spaces' own driver. Evidence goes to $EVIDENCE.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
REPO="$(cd "$WORKSPACE/../.." && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
BASE_IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
KEEP=0
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --image) BASE_IMAGE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --skip-build) BUILD=0; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
TARGET_VOLUME="${CUA_ENV_TARGET_VOLUME:-cua-e2e-host-target}"
BIN_IMAGE="${CUA_ENV_TEST_IMAGE:-cua-spacesd-linuxtest}"
T="${CARGO_TARGET_DIR:-$REPO/target}"
RUN="hs$$"
# Short paths: the daemons' Unix sockets live in these homes, so the default
# is under $HOME rather than the long macOS TMPDIR. Override with CUA_E2E_WORK.
WORK="${CUA_E2E_WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/cua/e2e}/$RUN"
EVIDENCE="${EVIDENCE:-$WORK/evidence}"
IMAGE="cua-e2e-local/linux-host-spaces:$RUN"
H="$WORK/h"
C="$WORK/c"
X="$WORK/x"
mkdir -p "$H" "$C" "$X" "$EVIDENCE" "$WORK/home"
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
RELAY_PORT="$(free_port)"
ISSUER_PORT="$(free_port)"
HOST_PORT="$(free_port)"
RELAY="http://127.0.0.1:$RELAY_PORT"
ISSUER="http://127.0.0.1:$ISSUER_PORT"
CUA="$T/debug/cua"
PIDS=()
# The host's daemon runs Docker Spaces; its temp HOME hides the engine's
# default socket, so name it (the current docker context's).
DOCKER_HOST="${DOCKER_HOST:-$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null || true)}"
export DOCKER_HOST

# Every cua process runs in a temp home with the file credential store.
as() {
    local home="$1"
    shift
    env HOME="$WORK/home" CUA_HOME="$home" CUA_CREDENTIAL_STORE=file \
        CUA_TELEMETRY_ENABLED=false CUA_RELAY_URL="$RELAY" CUA_DAEMON_AUTOSTART=0 "$@"
}

cleanup() {
    status=$?
    set +e
    as "$H" "$CUA" host status --json >"$EVIDENCE/host-status.json" 2>&1
    cp "$H/host/spaces-audit.jsonl" "$EVIDENCE/" 2>/dev/null
    cp "$H/host/driver.log" "$EVIDENCE/host-driver.log" 2>/dev/null
    # Spaces the host still provides (a failed run), by their local name.
    python3 - "$EVIDENCE/host-status.json" <<'EOF' | while read -r s; do as "$H" "$CUA" spaces delete "$s" --force >/dev/null 2>&1; done
import json, sys
try:
    for p in json.load(open(sys.argv[1])).get("providedSpaces", []):
        print(p["localSpace"])
except Exception:
    pass
EOF
    for home in "$C" "$H"; do as "$home" "$CUA" daemon stop >/dev/null 2>&1; done
    as "$H" "$CUA" host remove --force >/dev/null 2>&1
    for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
    docker ps -aq --filter "name=$RUN" | xargs -r docker rm -f >/dev/null 2>&1
    docker rmi -f "$IMAGE" >/dev/null 2>&1
    if [ "$status" != 0 ]; then
        echo "---- evidence in $EVIDENCE"
        tail -n 40 "$EVIDENCE/host-driver.log" 2>/dev/null
    fi
    if [ "$KEEP" != 1 ]; then
        cp -R "$EVIDENCE" "${TMPDIR:-/tmp}/cua-host-spaces-e2e-$RUN" 2>/dev/null
        rm -rf "$WORK"
        [ "$status" != 0 ] && echo "---- evidence copied to ${TMPDIR:-/tmp}/cua-host-spaces-e2e-$RUN"
    fi
    exit "$status"
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "needs Docker" >&2; exit 1; }
if [ "$BUILD" = 1 ]; then
    echo "==> building cua, cua-spacesd, cua-relay and the fake issuer for this machine"
    (cd "$WORKSPACE" && cargo build --locked -p cua-cli)
    (cd "$REPO/libs/cua-spacesd" && cargo build --locked -p cua-spacesd -p cua-relay \
        && cargo build --locked -p cua-relay --example fake_oidc)
    echo "==> building the Linux cua-spacesd the Spaces run"
    CUA_ENV_TARGET_VOLUME="$TARGET_VOLUME" "$REPO/libs/cua-spacesd/scripts/ci/build-linux-e2e-bins.sh"
fi

echo "==> the Space image: $BASE_IMAGE with the freshly built cua-spacesd"
docker run --rm -v "$TARGET_VOLUME:/target:ro" "$BIN_IMAGE" cat /target/e2e/cua-spacesd >"$WORK/cua-spacesd-linux"
cat >"$WORK/Dockerfile" <<EOF
FROM $BASE_IMAGE
COPY cua-spacesd-linux /opt/cua-e2e/cua-spacesd
ENV CUA_SPACESD_BIN=/opt/cua-e2e/cua-spacesd
EOF
chmod 755 "$WORK/cua-spacesd-linux"
docker build -q -t "$IMAGE" -f "$WORK/Dockerfile" "$WORK" >/dev/null

echo "==> fake OIDC issuer and cua-relay (devices enforced, no grace)"
"$T/debug/examples/fake_oidc" --listen "127.0.0.1:$ISSUER_PORT" --issuer "$ISSUER" \
    --sub ada --email ada@example.com --ttl 3600 >"$EVIDENCE/issuer.log" 2>&1 &
PIDS+=($!)
disown
"$T/debug/cua-relay" --listen "127.0.0.1:$RELAY_PORT" --oidc-issuer "$ISSUER" \
    --public-url "$RELAY" --device-grace-days 0 >"$EVIDENCE/relay.log" 2>&1 &
PIDS+=($!)
disown
for _ in $(seq 1 150); do
    curl -fsS -o /dev/null "$RELAY/healthz" 2>/dev/null && curl -fsS -o /dev/null "$ISSUER/healthz" 2>/dev/null && break
    sleep 0.2
done
curl -fsS -o /dev/null "$RELAY/healthz"

# A session as `cua auth login` leaves it (`fresh`: a sign-in just now,
# which bootstraps a device enrollment).
sign_in() {
    local home="$1" fresh="${2:-}"
    python3 - "$ISSUER" "$home/credentials.json" "$fresh" <<'EOF'
import json, sys, time, urllib.request, os
issuer, path, fresh = sys.argv[1], sys.argv[2], sys.argv[3]
q = "sub=ada&email=ada@example.com&ttl=3600" + ("&auth_time=now" if fresh else "")
tok = json.load(urllib.request.urlopen(f"{issuer}/mint?{q}"))
exp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 3500))
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, json.dumps({"access_token": tok["access_token"], "refresh_token": None,
                         "expires_at": exp, "token_type": "Bearer"}).encode())
os.close(fd)
EOF
}

echo "==> host: cua host setup --profile spare"
sign_in "$H"
as "$H" env CUA_ENV_LISTEN="127.0.0.1:$HOST_PORT" CUA_ENV_QUIC_PORT=0 CUA_IMAGE_LINUX="$IMAGE" \
    CUA_DAEMON_AUTOSTART=1 "$CUA" host setup --relay "$RELAY" --profile spare \
    --name "Mac mini (spare)" --runner process --driver-bin "$T/debug/cua-spacesd" --json \
    >"$EVIDENCE/host-setup.json"
# The host keeps no account session, only its machine token.
rm -f "$H/credentials.json"
python3 - "$EVIDENCE/host-setup.json" "$H/host/config.json" <<'EOF'
import json, sys
s = json.load(open(sys.argv[1]))
assert s["configured"] and s["mode"] == "relay", s
assert s["shareDesktop"] is False and s["provideSpaces"] is True, s
args = json.load(open(sys.argv[2].replace("config.json", "service.json")))["args"]
for flag in ("--no-desktop", "--no-driver", "--no-mcp"):
    assert flag in args, (flag, args)
print("host: desktop not shared, provides Spaces, driver without desktop services")
EOF
HOST_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["machineId"])' "$EVIDENCE/host-setup.json")"

echo "==> client: an enrolled device and its cua daemon"
sign_in "$C" fresh
as "$C" "$CUA" devices enroll --json >"$EVIDENCE/client-enroll.json"
as "$C" "$CUA" daemon start >"$EVIDENCE/client-daemon.log" 2>&1
for _ in $(seq 1 100); do
    as "$C" "$CUA" spaces ls --json >"$EVIDENCE/ls-before.json" 2>/dev/null \
        && grep -q "\"relay:$HOST_ID\"" "$EVIDENCE/ls-before.json" && break
    sleep 0.3
done
grep -q "\"relay:$HOST_ID\"" "$EVIDENCE/ls-before.json" || { echo "the host never showed up" >&2; exit 1; }

echo "==> an agent: \"spin up 2 linux spaces on my spare mac mini\" (cua mcp)"
as "$C" python3 - "$CUA" "$HOST_ID" "$RUN" "$EVIDENCE" <<'EOF'
import json, subprocess, sys
cua, host_id, run, evidence = sys.argv[1:5]
p = subprocess.Popen([cua, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=open(f"{evidence}/mcp.stderr", "w"), text=True, bufsize=1)
log = open(f"{evidence}/mcp-transcript.jsonl", "w")
n = 0
def rpc(method, params=None, notify=False):
    global n
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if not notify:
        n += 1
        msg["id"] = n
    p.stdin.write(json.dumps(msg) + "\n")
    log.write(json.dumps(msg) + "\n")
    if notify:
        return None
    while True:
        line = p.stdout.readline()
        if not line:
            raise SystemExit("cua mcp exited")
        r = json.loads(line)
        log.write(line)
        if r.get("id") == n:
            return r
def call(name, args):
    r = rpc("tools/call", {"name": name, "arguments": args})["result"]
    text = "".join(c.get("text", "") for c in r.get("content", []))
    return r.get("isError", False), text
rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "host-spaces-e2e", "version": "1"}})
rpc("notifications/initialized", notify=True)
tools = {t["name"]: t for t in rpc("tools/list")["result"]["tools"]}
desc = tools["create_space"]["description"]
assert "host:spare mac mini" in desc and "ambiguous_host" in desc, desc

err, text = call("create_space", {"on": "host:spare mac mini", "image": "linux",
                                  "count": 2, "name": run, "timeout": 600})
assert not err, text
made = json.loads(text)
assert isinstance(made, list) and len(made) == 2, made
for s in made:
    assert s["id"].startswith("relay:space-"), s
    assert s["host"] == host_id, s
    assert s["host_name"] == "Mac mini (spare)", s
ids = [s["id"] for s in made]
print("created", ids)

err, text = call("list_spaces", {})
assert not err, text
listed = {s["id"]: s for s in json.loads(text)}
for i in ids:
    assert listed[i]["host"] == host_id, listed[i]

err, text = call("space_bash", {"space": ids[0], "command": "echo from-a-provided-space; hostname"})
assert not err and "from-a-provided-space" in text, text
print("a command ran inside", ids[0])

# The host's own desktop and shell are not shared.
err, text = call("space_bash", {"space": f"relay:{host_id}", "command": "id"})
assert err and "does not share its desktop" in text, text
print("the host's own shell is refused:", text.strip().splitlines()[0][:120])

err, text = call("delete_space", {"space": ids[0]})
assert not err and "Deleted" in text, text
json.dump(ids, open(f"{evidence}/ids.json", "w"))
p.stdin.close()
p.wait(timeout=30)
EOF
SECOND="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[1])' "$EVIDENCE/ids.json")"

echo "==> the CLI lists it under its host, then deletes it"
as "$C" "$CUA" spaces ls | tee "$EVIDENCE/ls.txt"
python3 - "$EVIDENCE/ls.txt" "$HOST_ID" "$SECOND" <<'EOF'
import sys
lines = open(sys.argv[1]).read().splitlines()
host = next(i for i, l in enumerate(lines) if l.startswith(f"relay:{sys.argv[2]}"))
child = next(i for i, l in enumerate(lines) if l.strip().startswith(sys.argv[3]))
assert child == host + 1 and lines[child].startswith("  "), lines
print("grouped under the host")
EOF
as "$C" "$CUA" spaces delete "$SECOND" --force --json | tee "$EVIDENCE/delete.json"

echo "==> the host's audit"
as "$H" "$CUA" host status --json >"$EVIDENCE/host-status-after.json"
python3 - "$EVIDENCE/host-status-after.json" <<'EOF'
import json, sys
s = json.load(open(sys.argv[1]))
assert s["providedSpaces"] == [], s["providedSpaces"]
assert s.get("spacesAuditError") is None, s.get("spacesAuditError")
acts = [(e["action"], e["who"]) for e in reversed(s["spacesAudit"])]
creates = [w for a, w in acts if a == "create"]
deletes = [w for a, w in acts if a == "delete"]
assert len(creates) == 2 and len(deletes) == 2, acts
assert all("ada" in w for w in creates + deletes), acts
print("audit:", acts)
EOF

echo "==> a device that is not enrolled is refused"
sign_in "$X"
set +e
as "$X" "$CUA" spaces create linux --on "spare mac mini" --json >"$EVIDENCE/unenrolled.json" 2>"$EVIDENCE/unenrolled.err"
code=$?
set -e
[ "$code" != 0 ] || { echo "an unenrolled device created a Space" >&2; exit 1; }
grep -qi "not enrolled" "$EVIDENCE/unenrolled.err" "$EVIDENCE/unenrolled.json" \
    || { cat "$EVIDENCE/unenrolled.err" >&2; echo "expected a not-enrolled refusal" >&2; exit 1; }
echo "refused: $(grep -hi "not enrolled" "$EVIDENCE/unenrolled.err" "$EVIDENCE/unenrolled.json" | head -n 1 | cut -c1-200)"
docker ps -a --format '{{.Names}}' | grep -q "$RUN" && { echo "containers left behind" >&2; exit 1; }
echo "==> host spaces e2e: PASS (evidence: $EVIDENCE)"
