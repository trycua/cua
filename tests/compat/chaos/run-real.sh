#!/bin/bash
# Seeded chaos against a compat gateway on REAL Spaces, with a leak check
# after every seed (cua-chaos diffs a before/after inventory of the isolated
# CUA_HOME: Spaces, sandboxes, VM and helper processes, ports, containers,
# kernel objects, per-Space files and the gateway's records).
#
#   CUA=... CHAOS=... CHAOS_WORK=dir run-real.sh e2b 0..40 [-- gateway serve args]
#
# Everything lives under $CHAOS_WORK (an isolated CUA_HOME, pid files, logs);
# nothing touches ~/.cua. Exit 1: a seed leaked (its report and the replay
# command are in the output and in $CHAOS_WORK/run/seed-<n>.json).
set -euo pipefail
provider=${1:?provider}; seeds=${2:?seeds a..b}; shift 2
[ "${1:-}" = -- ] && shift
serve=("$@")
: "${CUA:?}" "${CHAOS:?}"
here=$(cd "$(dirname "$0")" && pwd)
export CHAOS_ROOT=${CHAOS_WORK:-$HOME/chaos}
mkdir -p "$CHAOS_ROOT/run"
export CUA_HOME=$CHAOS_ROOT/home DO_NOT_TRACK=1 CUA_TELEMETRY=0
mkdir -p "$CUA_HOME"
export RUST_LOG=${CHAOS_RUST_LOG:-cua_compat=debug,info}
PORT=${CHAOS_GW_PORT:-3100}
echo "$PORT" >"$CHAOS_ROOT/gateway.port"

export PY=${CHAOS_PYTHON:-$CHAOS_ROOT/venv/bin/python}
[ -n "${CHAOS_PYTHON:-}" ] || [ -x "$CHAOS_ROOT/venv/bin/python" ] || python3 -m venv "$CHAOS_ROOT/venv"
case "$provider" in e2b) pkg=e2b;; daytona) pkg=daytona;; modal) pkg="modal==$(cat "$here/../modal/PINNED" 2>/dev/null || echo 1.6.2)";; *) echo "no driver for $provider" >&2; exit 2;; esac
[ -n "${CHAOS_PYTHON:-}" ] || "$CHAOS_ROOT/venv/bin/pip" install -q "$pkg" >/dev/null

cat >"$CHAOS_ROOT/daemon.start" <<S
# Idempotent: a second start while it runs is a no-op (never a second daemon).
if [ -s "$CHAOS_ROOT/daemon.pid" ] && kill -0 "\$(cat "$CHAOS_ROOT/daemon.pid")" 2>/dev/null; then exit 0; fi
nohup "$CUA" daemon start --foreground >>"$CHAOS_ROOT/daemon.log" 2>&1 &
echo \$! >"$CHAOS_ROOT/daemon.pid"
for i in \$(seq 100); do timeout 3 "$CUA" daemon status >/dev/null 2>&1 && exit 0; sleep 0.3; done
exit 1
S
if [ -n "${CHAOS_NO_GATEWAY:-}" ]; then
  # The E2B shim talks to the daemon directly: there is no gateway to start or kill.
  echo "exit 0" >"$CHAOS_ROOT/gateway.start"
else
cat >"$CHAOS_ROOT/gateway.start" <<S
if [ -s "$CHAOS_ROOT/gateway.pid" ] && kill -0 "\$(cat "$CHAOS_ROOT/gateway.pid")" 2>/dev/null; then exit 0; fi
nohup "$CUA" compat serve --provider $provider --listen 127.0.0.1:$PORT --host-port $((PORT+1)) ${serve[*]} >>"$CHAOS_ROOT/gateway.log" 2>&1 &
echo \$! >"$CHAOS_ROOT/gateway.pid"
# No readiness probe: during a partition it would be dropped too. Clients retry.
sleep 1
S
fi
# In-Space faults reach a running sandbox Space of this home (container kind:
# `docker exec`, so the fault happens inside the Space and its limits apply).
cat >"$CHAOS_ROOT/space.exec" <<S
# Runs \$1 inside one random running sandbox Space of this home (any kind:
# container or vm), through the Cua CLI, so the Space's own limits apply.
n=\$(timeout 30 "$CUA" sandbox ls --json 2>/dev/null | python3 -c 'import sys,json,random
try:
    xs=[s["name"] for s in json.load(sys.stdin) if s.get("state")=="running" and s["name"].startswith(("e2b-","daytona-"))]
except Exception:
    xs=[]
print(random.choice(xs) if xs else "")')
[ -n "\$n" ] || exit 0
timeout 120 "$CUA" sandbox exec "\$n" "\$1" || true
S
cat >"$CHAOS_ROOT/spacesd.kill" <<S
# Inside the Space's own PID namespace; signals the one process named, by PID.
bash "$CHAOS_ROOT/space.exec" 'for p in /proc/[0-9]*; do case "\$(tr "\\0" " " <"\$p/cmdline" 2>/dev/null)" in *cua-spacesd*) kill -KILL "\${p#/proc/}";; esac; done'
S
bash "$CHAOS_ROOT/daemon.start"
bash "$CHAOS_ROOT/gateway.start"
trap 'for f in gateway daemon; do p=$(cat "$CHAOS_ROOT/$f.pid" 2>/dev/null); case "$p" in ""|*[!0-9]*|0|1) ;; *) kill -KILL "$p" 2>/dev/null || true;; esac; done; "$CUA" daemon stop >/dev/null 2>&1 || true' EXIT

# Extra setup (a host to register, a warm image), run with the isolated CUA_HOME.
[ -z "${CHAOS_PRE:-}" ] || bash -c "$CHAOS_PRE"
# Pull the image once so the first seeds do not time out on a cold cache.
"$CUA" sandbox create linux --name chaos-prewarm "${serve[@]}" >/dev/null 2>&1 && "$CUA" sandbox rm -f chaos-prewarm >/dev/null 2>&1 || true

# The client's environment (key, API URL, domain, CA bundle).
[ -n "${CHAOS_NO_GATEWAY:-}" ] || eval "$("$CUA" compat env --provider "$provider" --listen 127.0.0.1:$PORT --host-port $((PORT+1)))"
export PATH=$CHAOS_ROOT/venv/bin:$PATH
# With the client environment: warm what the first create builds (a Modal image, an E2B template).
[ -z "${CHAOS_WARM:-}" ] || bash -c "$CHAOS_WARM"

# The Spaces a warm pool keeps on purpose (the daemon's, not a leak).
cat >"$CHAOS_ROOT/wanted.json" <<'W'
{"keep": {"container": ["cua-pool-"], "space": ["cua-pool-"], "file:sandboxes": [".pool/cua-pool-", ".ephemeral/cua-pool-", "cua-pool-", ".checkpoints/e2b-golden-"], "file:vmm": ["qemu/_golden", "qemu/_layers", "qemu/e2b-golden-", "qemu/cua-pool-"], "process:qemu": ["/usr/bin/qemu-system-x86_64 -name e2b-golden-", "/usr/bin/qemu-system-x86_64 -name cua-pool-"], "port": ["qemu-system#"]}}
W
"$CHAOS" run --wanted-cmd "cat $CHAOS_ROOT/wanted.json" --profile "${CHAOS_PROFILE:-host}" --seeds "$seeds" --home "$CUA_HOME" \
  --gateway-dir "$CUA_HOME/compat/$provider" --scope "$provider-real" \
  --dir "$CHAOS_ROOT/run" --faults "${CHAOS_FAULTS:-$here/faults-$provider.json}" ${CHAOS_SOURCES:+--sources "$CHAOS_SOURCES"} \
  --cleanup-cmd "$PY $here/cleanup_$provider.py; ${CHAOS_EXTRA_CLEANUP:-true}" --settle-s "${CHAOS_SETTLE_S:-150}" \
  --driver "$PY" "$here/driver_$provider.py"
