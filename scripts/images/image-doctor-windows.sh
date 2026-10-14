#!/usr/bin/env bash
# Windows lane of the image doctor (VM only): boot the Windows containerDisk
# under QEMU+KVM on a throwaway overlay and run `cua-spacesd doctor` in the
# guest's interactive session. Images without cua-spacesd (the computer-server
# era copies) get the doctor shim instead, with the claims in
# scripts/images/manifests/windows-2022.json.
#
#   image-doctor-windows.sh (--image REF | --disk FILE) --out DIR [--strict]
#       [--cua CUA_BIN] [--manifest FILE] [--mem 4G] [--smp 2] [--timeout 1800]
#
# With cua-spacesd, the lane first checks the bootstrap path the SDK uses for
# Windows (the guest starts uninitialized; the first Init, sent from the host
# with grpcurl over the forwarded port, installs a fresh token, which
# cua-spacesd persists to C:\ProgramData\cua\spacesd\token), then runs
# `cua-spacesd doctor --effects virtual --expect-runtime qemu` in the guest
# through the image's computer-server (:8000, kept for older clients).
#
# Needs /dev/kvm (GitHub-hosted ubuntu-24.04 runners expose it), qemu-system-x86_64,
# qemu-img, OVMF, crane (with --image), grpcurl, python3. Writes DIR/report.json,
# report.txt, lane.json, serial.log (and report.xml). Always stops the VM.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMAGE="" DISK="" CUA="" OUT="$PWD/doctor-out/windows" STRICT=() MANIFEST="$HERE/manifests/windows-2022.json"
MEM=4G SMP=2 TIMEOUT=1800
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --disk) DISK="$2"; shift 2 ;;
        --cua) CUA="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --strict) STRICT=(--strict); shift ;;
        --manifest) MANIFEST="$2"; shift 2 ;;
        --mem) MEM="$2"; shift 2 ;;
        --smp) SMP="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMAGE" ] || [ -n "$DISK" ] || { echo "--image or --disk is required" >&2; exit 2; }
[ -w /dev/kvm ] || { echo "the Windows lane needs a writable /dev/kvm" >&2; exit 2; }
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cua-e2e-windoctor.XXXXXX")"
QEMU_PID=""
cleanup() { [ -n "$QEMU_PID" ] && kill "$QEMU_PID" 2>/dev/null; wait 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT
lane_json() { printf '{"lane": "windows", "arch": "amd64", "variant": "containerdisk", "claim_secrets": false, "mode": "%s", "exit": %s}\n' "$1" "$2" >"$OUT/lane.json"; }

if [ -z "$DISK" ]; then
    echo "==> extracting the disk of $IMAGE"
    crane export --platform linux/amd64 "$IMAGE" - | tar -x -C "$WORK" disk/disk.img
    DISK="$WORK/disk/disk.img"
fi
fmt="$(qemu-img info --output=json "$DISK" | python3 -c 'import json,sys;print(json.load(sys.stdin)["format"])')"
qemu-img create -q -f qcow2 -F "$fmt" -b "$(cd "$(dirname "$DISK")" && pwd)/$(basename "$DISK")" "$WORK/overlay.qcow2"
code="" vars=""
for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do [ -f "$f" ] && { code="$f"; break; }; done
for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do [ -f "$f" ] && { vars="$f"; break; }; done
[ -n "$code" ] && [ -n "$vars" ] || { echo "OVMF firmware not found (apt install ovmf)" >&2; exit 2; }
cp "$vars" "$WORK/vars.fd"
free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
csport="$(free_port)" envport="$(free_port)"
echo "==> booting (KVM, $MEM, -smp $SMP; computer-server -> 127.0.0.1:$csport, spacesd -> 127.0.0.1:$envport)"
qemu-system-x86_64 -machine q35 -accel kvm -cpu host -smp "$SMP" -m "$MEM" \
    -drive "if=pflash,format=raw,readonly=on,file=$code" \
    -drive "if=pflash,format=raw,file=$WORK/vars.fd" \
    -drive "file=$WORK/overlay.qcow2,if=none,id=d0,format=qcow2" -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 \
    -nic "user,model=e1000e,hostfwd=tcp:127.0.0.1:$csport-:8000,hostfwd=tcp:127.0.0.1:$envport-:3211" \
    -vga std -display none -serial "file:$OUT/serial.log" -monitor none >"$WORK/qemu.out" 2>&1 &
QEMU_PID=$!
EXEC="$ROOT/libs/images/windows-2022/guest-exec.py"
CS="http://127.0.0.1:$csport"
python3 "$EXEC" --url "$CS" --wait "$TIMEOUT" || { cat "$WORK/qemu.out"; exit 2; }

# A TCP connect to a QEMU user-net forward succeeds even when nothing in the
# guest listens, so probe with a real RPC: GetCapabilities answers in
# bootstrap mode. The logon task can take a few minutes after first boot.
PROTO=(-import-path "$ROOT/libs/cua/proto" -proto cua/env/v1/system.proto)
grpc() { grpcurl -plaintext -max-time 20 "${PROTO[@]}" "$@"; }
spacesd=0 caps=""
if command -v grpcurl >/dev/null; then
    for _ in $(seq 1 60); do
        caps="$(grpc -d '{}' "127.0.0.1:$envport" cua.env.v1.SystemService/GetCapabilities 2>/dev/null)" && { spacesd=1; break; }
        sleep 5
    done
fi

if [ "$spacesd" = 0 ]; then
    echo "==> no cua-spacesd on :3211; doctor shim through computer-server"
    [ -x "$CUA" ] || { echo "--cua is required for the shim" >&2; exit 2; }
    set +e
    "$CUA" doctor --no-host --shim "computer-server=$CS" --expect-manifest "$MANIFEST" \
        --effects virtual ${STRICT[@]+"${STRICT[@]}"} --out "$OUT/report.json" --junit "$OUT/report.xml" | tee "$OUT/report.txt"
    rc=${PIPESTATUS[0]}
    set -e
    lane_json shim "$rc"
    exit "$rc"
fi

echo "==> bootstrap: GetCapabilities, then Init with a fresh token"
echo "$caps" | python3 -c 'import json,sys;c=json.load(sys.stdin);print("version", c.get("version"), "initialized", c.get("initialized", False))'
initialized="$(echo "$caps" | python3 -c 'import json,sys;print(str(json.load(sys.stdin).get("initialized", False)).lower())')"
[ "$initialized" = false ] || { echo "cua-spacesd is already initialized at first boot (want bootstrap mode)" >&2; lane_json spacesd 1; exit 1; }
token="$(python3 -c 'import secrets;print(secrets.token_hex(16))')"
grpc -d "{\"token\":\"$token\"}" "127.0.0.1:$envport" cua.env.v1.SystemService/Init >/dev/null
# Unauthenticated calls are refused now; the token is accepted.
if grpc -d '{"token":"x"}' "127.0.0.1:$envport" cua.env.v1.SystemService/Init >/dev/null 2>&1; then
    echo "a second, unauthenticated Init succeeded" >&2; lane_json spacesd 1; exit 1
fi
grpc -H "authorization: Bearer $token" -d '{}' "127.0.0.1:$envport" cua.env.v1.SystemService/GetCapabilities >/dev/null
unset token

echo "==> cua-spacesd doctor in the guest"
set +e
strict='$false'; [ ${#STRICT[@]} -gt 0 ] && strict='$true'
sed "s/__STRICT__/$strict/" "$ROOT/libs/images/windows-2022/run-doctor.ps1" >"$WORK/run-doctor.ps1"
python3 "$EXEC" --url "$CS" --timeout 900 "$WORK/run-doctor.ps1" >"$WORK/doctor.out" 2>"$WORK/doctor.err"
rc=$?
set -e
python3 - "$WORK/doctor.out" "$OUT" <<'PY'
import base64, pathlib, re, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
out = pathlib.Path(sys.argv[2])
def block(name):
    m = re.search(rf"-----BEGIN {name}-----\s*(.*?)\s*-----END {name}-----", text, re.S)
    return base64.b64decode(m.group(1)) if m else None
for name, file in (("REPORT", "report.json"), ("JUNIT", "report.xml")):
    data = block(name)
    if data is not None:
        (out / file).write_bytes(data)
human = re.split(r"-----BEGIN ", text)[0]
(out / "report.txt").write_text(human)
print(human)
PY
cat "$WORK/doctor.err" >&2 || true
[ -f "$OUT/report.json" ] || { echo "the guest doctor wrote no report (exit $rc)" >&2; [ "$rc" = 0 ] && rc=2; }
lane_json spacesd "$rc"
exit "$rc"
