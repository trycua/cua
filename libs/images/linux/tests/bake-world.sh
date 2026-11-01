#!/usr/bin/env bash
# The docker tier's bake integration test, against real Spaces: bake
# tests/compose-world (agent + MCP + grader containers) into a checkpoint
# with `cua sb bake --verify-fork`, fork it, and check in the fork that
#   - the stack came back live: every container running or healthy, none
#     restarted, the same container start times as at the bake (the
#     processes were restored, not restarted);
#   - `docker exec` into the agent works and the agent reaches the grader;
#   - the MCP server answers the official MCP client through a port forward
#     from this host, its process the one that was baked;
#   - iptables and br_netfilter work in the fork.
# Then it measures the checkpoint on disk and deletes everything it made.
#
#   bake-world.sh --cua CUA --image IMAGE [--evidence DIR] [--memory 4GB] [--cpu 2]
#
# IMAGE: a docker-tier disk (`.../linux-docker/<arch>/disk.img`) or reference.
# Needs a host that runs local `vm` Spaces with memory checkpoints (QEMU on
# KVM or HVF) and `uv` (the MCP client runs with `uv run --with mcp`).
# Writes DIR/bake.json, DIR/fork.json and DIR/result.json.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CUA=cua; IMAGE=""; EVIDENCE=""; MEMORY=4GB; CPU=2
while [ $# -gt 0 ]; do
    case "$1" in
        --cua) CUA="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        --cpu) CPU="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMAGE" ] || { echo "--image is required" >&2; exit 2; }
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
EVIDENCE="$(cd "$EVIDENCE" && pwd)"
run_id="$(date +%s)-$$"
ck="world-$run_id"
fork="world-fork-$run_id"
fwd_pid=""
cleanup() {
    [ -n "$fwd_pid" ] && kill "$fwd_pid" 2>/dev/null || true
    "$CUA" sb rm "$fork" --force >/dev/null 2>&1 || true
    "$CUA" sb rm "bake-$ck" --force >/dev/null 2>&1 || true
    "$CUA" sb checkpoints rm "$ck" >/dev/null 2>&1 || true
}
trap cleanup EXIT
ok() { echo "  ok   $*"; }
fail() { echo "  FAIL $*" >&2; exit 1; }
gx() { "$CUA" sb exec "$fork" --timeout 120 "$@"; }

echo "==> bake $HERE/compose-world -> $ck ($IMAGE)"
"$CUA" sb bake "$HERE/compose-world" --checkpoint "$ck" --image "$IMAGE" \
    --memory "$MEMORY" --cpu "$CPU" --health-timeout 900 \
    --health 'curl -sS -o /dev/null --max-time 5 http://127.0.0.1:18000/mcp' \
    --run "docker inspect --format '{{.Name}} {{.State.StartedAt}}' agent mcp verifier >/opt/cua-bake/started-at" \
    --run "docker exec mcp sh -c 'cat /proc/1/stat | cut -d\" \" -f22' >/opt/cua-bake/mcp-starttime" \
    --verify-fork --json | tee "$EVIDENCE/bake.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["verify_fork"]["live"], d' "$EVIDENCE/bake.json" \
    || fail "the --verify-fork fork was not live"
ok "baked and a --verify-fork fork came up live"

echo "==> fork $ck -> $fork"
"$CUA" sb fork "$ck" "$fork" --json | tee "$EVIDENCE/fork.json"

# The stack came back as it was baked: the same containers, started at the
# same times, none restarted.
gx "sudo docker inspect --format '{{.Name}} {{.State.StartedAt}}' agent mcp verifier" >"$EVIDENCE/started-at-fork"
gx "cat /opt/cua-bake/started-at" >"$EVIDENCE/started-at-bake"
diff -u "$EVIDENCE/started-at-bake" "$EVIDENCE/started-at-fork" || fail "containers restarted in the fork"
states="$(gx "sudo docker inspect --format '{{.Name}} {{.State.Running}} {{.RestartCount}} {{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' agent mcp verifier")"
echo "$states"
echo "$states" | awk '$2 != "true" || $3 != "0" || ($4 != "healthy" && $4 != "none") {bad=1} END {exit bad}' \
    || fail "a container is not running or not healthy"
ok "agent, mcp and verifier running, not restarted, same start times as baked"

# The MCP server process is the baked one (the kernel start time of its pid 1).
[ "$(gx "sudo docker exec mcp sh -c 'cut -d\" \" -f22 /proc/1/stat'")" = "$(gx "cat /opt/cua-bake/mcp-starttime")" ] \
    || fail "the MCP server process was restarted"
ok "the MCP server process is the baked one"

# docker exec, and the agent reaches the grader over the world network.
[ "$(gx "sudo docker exec agent python -c 'import urllib.request; print(urllib.request.urlopen(\"http://verifier:9000/\", timeout=5).status)'")" = 200 ] \
    || fail "the agent cannot reach the grader"
ok "docker exec into the agent; agent -> verifier:9000 answers"

# iptables / br_netfilter in the fork.
gx "sudo sh -c 'modprobe br_netfilter && test -e /proc/sys/net/bridge/bridge-nf-call-iptables && iptables -N CUA_FORK && iptables -A CUA_FORK -j RETURN && iptables -F CUA_FORK && iptables -X CUA_FORK'" \
    || fail "iptables / br_netfilter in the fork"
ok "br_netfilter and iptables work in the fork"

# The MCP server through a port forward from this host, with the official client.
port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
"$CUA" sb port-forward "$fork" "18000:$port" >"$EVIDENCE/port-forward.log" 2>&1 &
fwd_pid=$!
for _ in $(seq 1 60); do nc -z 127.0.0.1 "$port" 2>/dev/null && break; sleep 0.5; done
uv run --quiet --no-project --with mcp==2.3.0 python "$HERE/compose-world/mcp_check.py" "http://127.0.0.1:$port/mcp" \
    | tee "$EVIDENCE/mcp.json"
python3 - "$EVIDENCE/mcp.json" <<'PY' || fail "MCP check"
import json, sys
d = json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])
assert d["tools"] == ["uptime", "write_file"], d
assert d["write"].startswith("wrote 2 bytes"), d
PY
[ "$(gx "sudo docker exec verifier cat /workspace/probe.txt")" = ok ] || fail "the grader does not see the MCP write"
ok "MCP initialize, tools/list and tools/call through a host port forward; the grader sees the write"

# The checkpoint on disk: its entry under the QEMU state root of CUA_HOME
# (memory stream, device state, firmware vars) plus the disk layer it pins.
python3 - "${CUA_HOME:-$HOME/.cua}" "$ck" "$EVIDENCE/checkpoint-size.json" <<'PY'
import json, os, sys
home, ck, out = sys.argv[1:]
def size(p):
    st = os.stat(p)
    return st.st_size, st.st_blocks * 512
found = None
for root, dirs, files in os.walk(home):
    if os.path.basename(root) == ck and "state.json" in files:
        found = root
        break
if not found:
    json.dump({"error": f"no checkpoint entry {ck} under {home}"}, open(out, "w"))
    sys.exit(0)
apparent = allocated = 0
for r, _, fs in os.walk(found):
    for f in fs:
        a, b = size(os.path.join(r, f))
        apparent += a
        allocated += b
state = json.load(open(os.path.join(found, "state.json")))
layer = state.get("disk")
layer = layer if isinstance(layer, str) else (layer or {}).get("path")
la = lb = 0
if layer and os.path.exists(layer):
    la, lb = size(layer)
json.dump({"entry": found, "entry_apparent": apparent, "entry_allocated": allocated,
           "disk_layer": layer, "disk_layer_apparent": la, "disk_layer_allocated": lb,
           "total_allocated": allocated + lb}, open(out, "w"), indent=2)
PY
cat "$EVIDENCE/checkpoint-size.json"

python3 - "$EVIDENCE" <<'PY'
import json, sys
ev = sys.argv[1]
bake = json.load(open(f"{ev}/bake.json"))
fork = json.load(open(f"{ev}/fork.json"))
mcp = json.loads(open(f"{ev}/mcp.json").read().strip().splitlines()[-1])
out = {k: bake.get(k) for k in ("dockerd_ready_s", "create_s", "upload_s", "compose_up_s", "health_s", "checkpoint_s", "total_s")}
out.update({"verify_fork_s": bake["verify_fork"]["fork_s"], "fork_s": fork["seconds"], "mcp_server_uptime_s": mcp["uptime"]["seconds"]})
out["checkpoint_bytes"] = json.load(open(f"{ev}/checkpoint-size.json")).get("total_allocated")
json.dump(out, open(f"{ev}/result.json", "w"), indent=2)
print("RESULT", json.dumps(out))
PY
echo "==> bake integration test passed"
