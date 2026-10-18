#!/usr/bin/env bash
# Build the Windows Server 2022 cua-spacesd image from the Windows workspace
# disk (the computer-server-era 2022-disk, pinned by digest in
# cd-image-windows.yml): boot it under QEMU+KVM on a throwaway overlay, provision
# cua-spacesd through the base's own computer-server, shut the guest down
# cleanly and write the result as a new disk.img (the containerDisk payload).
#
#   build-image.sh --base REF --bin DIR --out DIR [--probe] [--stamp S]
#       [--mem 4G] [--smp 2] [--timeout 2400] [--keep-work]
#
# --bin DIR holds cua-spacesd.exe (x86_64-pc-windows-msvc, static CRT) and
# cua-spacesd-build-info.json (`cua-spacesd build-info` output); see
# .github/workflows/cd-image-windows.yml. --probe boots the base, prints
# inventory.ps1 and stops (nothing is written).
#
# Output: DIR/disk.img, DIR/manifest.json (the claims baked into the guest at
# C:\ProgramData\cua-image\manifest.json), DIR/inventory.txt, DIR/install.log,
# DIR/serial.log. Needs /dev/kvm, qemu-system-x86_64, qemu-img, OVMF, crane,
# python3, grpcurl. The guest is always stopped; the overlay is deleted unless
# --keep-work.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
BASE="" BIN="" OUT="" PROBE=0 MEM=4G SMP=2 TIMEOUT=2400 KEEP=0
STAMP="$(date -u +%Y%m%d)-$(git -C "$HERE" rev-parse --short=7 HEAD)"
while [ $# -gt 0 ]; do
    case "$1" in
        --base) BASE="$2"; shift 2 ;;
        --bin) BIN="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --probe) PROBE=1; shift ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --mem) MEM="$2"; shift 2 ;;
        --smp) SMP="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --keep-work) KEEP=1; shift ;;
        -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$BASE" ] && [ -n "$OUT" ] || { echo "--base and --out are required" >&2; exit 2; }
[ "$PROBE" = 1 ] || [ -f "$BIN/cua-spacesd.exe" ] || { echo "--bin DIR must hold cua-spacesd.exe" >&2; exit 2; }
[ -w /dev/kvm ] || { echo "needs a writable /dev/kvm" >&2; exit 2; }
tools=(qemu-system-x86_64 qemu-img crane python3 jq); [ "$PROBE" = 1 ] || tools+=(grpcurl)
for t in "${tools[@]}"; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 2; }; done
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="${CUA_WINDOWS_BUILD_WORK:-$OUT/work}"
mkdir -p "$WORK/serve"
EXEC="$HERE/guest-exec.py"
QEMU_PID="" HTTP_PID=""
cleanup() {
    [ -n "$HTTP_PID" ] && kill "$HTTP_PID" 2>/dev/null
    [ -n "$QEMU_PID" ] && kill "$QEMU_PID" 2>/dev/null
    wait 2>/dev/null || true
    [ "$KEEP" = 1 ] || rm -rf "$WORK"
}
trap cleanup EXIT
free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }

echo "==> base $BASE"
BASE_DIGEST="$(crane digest "$BASE")"
BASE_PINNED="${BASE%%[:@]*}@$BASE_DIGEST"
case "$BASE" in *@*) BASE_PINNED="$BASE" ;; esac
if [ ! -f "$WORK/base/disk/disk.img" ]; then
    mkdir -p "$WORK/base"
    crane export --platform linux/amd64 "$BASE_PINNED" - | tar -x -C "$WORK/base" disk/disk.img
fi
fmt="$(qemu-img info --output=json "$WORK/base/disk/disk.img" | jq -r .format)"
qemu-img info "$WORK/base/disk/disk.img"
rm -f "$WORK/overlay.qcow2"
qemu-img create -q -f qcow2 -F "$fmt" -b "$WORK/base/disk/disk.img" "$WORK/overlay.qcow2"
df -h "$WORK" | tail -1

code="" vars=""
for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do [ -f "$f" ] && { code="$f"; break; }; done
for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do [ -f "$f" ] && { vars="$f"; break; }; done
[ -n "$code" ] && [ -n "$vars" ] || { echo "OVMF firmware not found (apt install ovmf)" >&2; exit 2; }
cp "$vars" "$WORK/vars.fd"

# Files the guest downloads from the host (10.0.2.2 is the host's loopback
# under QEMU user networking).
cp "$HERE"/*.ps1 "$WORK/serve/"
mkdir -p "$WORK/serve/fixtures"
cp "$HERE"/fixtures/*.ps1 "$WORK/serve/fixtures/"
[ "$PROBE" = 1 ] || cp "$BIN/cua-spacesd.exe" "$WORK/serve/"
fsport="$(free_port)"
python3 -m http.server "$fsport" --bind 127.0.0.1 --directory "$WORK/serve" >"$WORK/http.log" 2>&1 &
HTTP_PID=$!
FETCH="http://10.0.2.2:$fsport"

csport="$(free_port)" envport="$(free_port)"
echo "==> booting (KVM, $MEM, -smp $SMP; computer-server -> 127.0.0.1:$csport, spacesd -> 127.0.0.1:$envport)"
qemu-system-x86_64 -machine q35 -accel kvm -cpu host -smp "$SMP" -m "$MEM" \
    -drive "if=pflash,format=raw,readonly=on,file=$code" \
    -drive "if=pflash,format=raw,file=$WORK/vars.fd" \
    -drive "file=$WORK/overlay.qcow2,if=none,id=d0,format=qcow2,cache=unsafe" -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 \
    -nic "user,model=e1000e,hostfwd=tcp:127.0.0.1:$csport-:8000,hostfwd=tcp:127.0.0.1:$envport-:3211" \
    -vga std -display none -serial "file:$OUT/serial.log" -monitor none >"$WORK/qemu.out" 2>&1 &
QEMU_PID=$!
CS="http://127.0.0.1:$csport"
t0=$SECONDS
python3 "$EXEC" --url "$CS" --wait "$TIMEOUT" || { cat "$WORK/qemu.out"; exit 2; }
echo "==> computer-server answered after $((SECONDS - t0))s"
gexec() { python3 "$EXEC" --url "$CS" --fetch-base "$FETCH" "$@"; }
gexec --timeout 300 inventory.ps1 | tee "$OUT/inventory.txt"
if [ "$PROBE" = 1 ]; then echo "probe done"; exit 0; fi

# The claims baked into the guest: the repo claims plus this build's identity.
python3 - "$REPO_ROOT/scripts/images/manifests/windows-2022.json" "$BIN/cua-spacesd-build-info.json" \
    "$BASE_PINNED" "$STAMP" "$(git -C "$HERE" rev-parse HEAD)" >"$OUT/manifest.json" <<'PY'
import json, sys
claims = json.load(open(sys.argv[1]))
info = json.load(open(sys.argv[2]))
claims.pop("_doc", None)
spacesd = claims.setdefault("spacesd", {})
spacesd.update({"present": True, "source": "build"})
for key in ("version", "protocol_revision", "git_sha", "cua_driver_version",
            "tools_sha256", "tools_count", "codecs_compiled"):
    if key in info:
        spacesd[key] = info[key]
claims["source_revision"] = sys.argv[5]
claims["build"] = {"base": sys.argv[3], "stamp": sys.argv[4]}
json.dump(claims, sys.stdout, indent=2)
print()
PY
cp "$OUT/manifest.json" "$WORK/serve/manifest.json"

echo "==> installing cua-spacesd"
gexec --timeout 900 install-spacesd.ps1 -Fetch "$FETCH" 2>&1 | tee "$OUT/install.log"
[ "${PIPESTATUS[0]}" = 0 ] || { echo "install failed" >&2; exit 1; }

echo "==> waiting for cua-spacesd on the forwarded port"
# A TCP connect to a QEMU user-net forward succeeds even when nothing in the
# guest listens, so ask cua-spacesd itself (GetCapabilities answers in
# bootstrap mode).
ok=0
for _ in $(seq 1 60); do
    if grpcurl -plaintext -max-time 10 -import-path "$REPO_ROOT/libs/cua/proto" -proto cua/env/v1/system.proto \
        -d '{}' "127.0.0.1:$envport" cua.env.v1.SystemService/GetCapabilities >"$OUT/capabilities.json" 2>/dev/null; then
        ok=1; break
    fi
    sleep 5
done
[ "$ok" = 1 ] || { echo "cua-spacesd is not reachable on :3211 after install" >&2; gexec --timeout 120 inventory.ps1 >&2 || true; exit 1; }
echo "cua-spacesd answers GetCapabilities through the forward"

echo "==> sealing and shutting down"
gexec --timeout 300 seal.ps1 2>&1 | tee -a "$OUT/install.log" || true
for _ in $(seq 1 120); do kill -0 "$QEMU_PID" 2>/dev/null || break; sleep 5; done
if kill -0 "$QEMU_PID" 2>/dev/null; then echo "the guest did not power off" >&2; exit 1; fi
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

echo "==> writing disk.img (compressed qcow2, like the base)"
rm -f "$OUT/disk.img"
qemu-img convert -c -O qcow2 -o compression_type=zlib "$WORK/overlay.qcow2" "$OUT/disk.img"
qemu-img info "$OUT/disk.img"
echo "built $OUT/disk.img from $BASE_PINNED (stamp $STAMP)"
