#!/usr/bin/env bash
# Eval-parity lane (Linux images with both variants): the rootfs under docker
# (gVisor by default) and the containerDisk under QEMU, side by side, then
# `cua doctor parity`: the doctor must pass on both, every fidelity key must
# be identical or an expected difference (the manifest's
# parity.expected_diff), and each task (builtin:form, builtin:file) must get
# the same evaluator score on both in oracle, null and partial modes.
#
#   image-parity-lane.sh --rootfs REF --disk DISK.img --cua CUA_BIN --out DIR
#       [--arch amd64|arm64] [--runtime runsc|runc] [--tasks LIST]
#
# One container (4g) and one VM (4G) at a time; both are removed on exit.
# Writes DIR/parity.json, parity.md and the VM serial log.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
ROOTFS="" DISK="" CUA="" OUT="$PWD/doctor-out/parity" ARCH="$(host_arch)" RUNTIME=runsc TASKS="builtin:form,builtin:file"
while [ $# -gt 0 ]; do
    case "$1" in
        --rootfs) ROOTFS="$2"; shift 2 ;;
        --disk) DISK="$2"; shift 2 ;;
        --cua) CUA="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --arch) ARCH="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --tasks) TASKS="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$ROOTFS" ] && [ -f "$DISK" ] && [ -x "$CUA" ] || { echo "--rootfs, --disk and --cua are required" >&2; exit 2; }
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
NAME="cua-e2e-parity-$$"
TOKEN_A="cua-e2e-parity-a-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
TOKEN_B="cua-e2e-parity-b-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
WORK="$(mktemp -d)"
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$WORK"; }
trap cleanup EXIT

echo "==> A: $ROOTFS under --runtime=$RUNTIME"
docker run -d --name "$NAME" --runtime="$RUNTIME" --shm-size=512m --memory=4g --memory-swap=4g \
    -e CUA_ENV_TOKEN="$TOKEN_A" -p 127.0.0.1::3211 "$ROOTFS" >/dev/null
for _ in $(seq 1 120); do
    [ "$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null)" = healthy ] && break
    sleep 1
done
PORT_A="$(docker port "$NAME" 3211/tcp | head -1 | sed 's/.*://')"

cat >"$WORK/run.sh" <<EOF
set -uo pipefail
for _ in \$(seq 1 90); do curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:\$QEMU_FWD_3211/health" | grep -q 204 && break; sleep 2; done
"$CUA" --embedded --state-dir "$WORK/state" doctor parity "http://127.0.0.1:$PORT_A" "http://127.0.0.1:\$QEMU_FWD_3211" \
    --token "$TOKEN_A" --token-b "$TOKEN_B" --tasks "$TASKS" --out "$OUT/parity.json" | tee "$OUT/parity.md"
exit \${PIPESTATUS[0]}
EOF
echo "==> B: $DISK under QEMU"
set +e
"$REPO/libs/images/common/boot-qemu.sh" "$DISK" --arch "$ARCH" --mem 4G --smp 4 --fwd 3211 \
    --env-token "$TOKEN_B" --timeout 1200 --log "$OUT/serial.log" --run "bash $WORK/run.sh" | tee "$OUT/boot.txt"
rc=${PIPESTATUS[0]}
set -e
status="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["status"])' "$OUT/parity.json" 2>/dev/null || echo missing)"
echo "==> parity $status (exit $rc), report in $OUT"
[ "$status" = pass ]
