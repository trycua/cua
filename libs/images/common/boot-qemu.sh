#!/usr/bin/env bash
# Boot a libs/images disk.img (the containerDisk payload) under local QEMU and
# smoke it: serial console reaches a login prompt, optional TCP probes through
# user-mode port forwards, optional RFB frame grab, optional SSH.
#
# The base disk is never written: the VM runs on a throwaway qcow2 overlay
# (the same copy-on-write shape KubeVirt uses for containerDisks).
#
#   boot-qemu.sh <disk.img> [--arch arm64|amd64] [--mem 4G] [--smp 4]
#       [--fwd 3211,22] [--probe 3211] [--rfb OUT.png (VNC images only)]
#       [--ssh] [--ssh-cmd 'CMD'] [--env-token TOKEN]
#       [--claim-secrets DIR] [--run CMD]
#       [--timeout 300] [--log serial.log] [--keep-running]
#
# Acceleration (BOOT_QEMU_ACCEL overrides): hvf on macOS for the host arch, kvm on Linux when /dev/kvm is
# usable, else TCG (slow; an amd64 guest on Apple Silicon is TCG).
# Firmware: arm64 -> edk2-aarch64-code.fd (UEFI); amd64 -> SeaBIOS (QEMU's
# default, = KubeVirt `bios`), or --uefi for edk2-x86_64-code.fd.
# The guest always gets a NoCloud seed (cidata ISO), like Fleet's cloud-init:
# without a datasource cloud-init disables itself and never generates the SSH
# host keys, so sshd stays down. --ssh adds a throwaway key to the `cua` user
# (then logs in and lists the cua-* units); --env-token writes
# /etc/cua/env-token and restarts cua-spacesd if it raced cloud-init.
# --claim-secrets DIR simulates Fleet's per-claim Secret share on KubeVirt
# (virtiofs tag cua-claim-secrets -> /run/cua). QEMU on macOS has no
# virtiofsd, so DIR is shared over virtio-9p with the same tag, read-only,
# and the seed's bootcmd installs a run-cua.mount drop-in switching Type= to
# 9p. On Linux with virtiofsd installed (/usr/libexec/virtiofsd, or
# CUA_VIRTIOFSD) the share is real virtio-fs instead, exactly the device
# KubeVirt attaches (CUA_BOOT_VIRTIOFS=0 forces 9p). Everything above the
# mount (token-sync, driver) is the real path.
# --run CMD runs on the host after the probes with QEMU_FWD_<guest port>=<host
# port> exported (and QEMU_SSH_KEY with --ssh); its exit status joins the
# result.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=ssh-retry.sh
. "$HERE/ssh-retry.sh"
DISK="${1:?disk.img}"; shift
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
ARCH="$(host_arch)"; MEM=4G; SMP=4; FWD="3211,22"; PROBE=""; RFB_OUT=""
CLAIM_DIR=""; RUN_CMD=""
SSH=0; SSH_CMD='uname -a; systemctl is-system-running || true; systemctl --no-pager --plain list-units "cua-*"'; ENV_TOKEN=""; TIMEOUT=300; LOG=""; KEEP=0; UEFI=0
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --mem) MEM="$2"; shift 2 ;;
        --smp) SMP="$2"; shift 2 ;;
        --fwd) FWD="$2"; shift 2 ;;
        --probe) PROBE="$2"; shift 2 ;;
        --rfb) RFB_OUT="$2"; shift 2 ;;
        --ssh) SSH=1; shift ;;
        --ssh-cmd) SSH=1; SSH_CMD="$2"; shift 2 ;;
        --env-token) ENV_TOKEN="$2"; shift 2 ;;
        --claim-secrets) CLAIM_DIR="$(cd "$2" && pwd)"; shift 2 ;;
        --run) RUN_CMD="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --log) LOG="$2"; shift 2 ;;
        --uefi) UEFI=1; shift ;;
        --keep-running) KEEP=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-boot.XXXXXX")"
LOG="${LOG:-$WORK/serial.log}"
: >"$LOG"
QEMU_PID=""
VIRTIOFSD_PID=""
cleanup() {
    if [ "$KEEP" != 1 ] && [ -n "$QEMU_PID" ]; then kill "$QEMU_PID" 2>/dev/null || true; wait "$QEMU_PID" 2>/dev/null || true; fi
    if [ "$KEEP" != 1 ] && [ -n "$VIRTIOFSD_PID" ]; then kill "$VIRTIOFSD_PID" 2>/dev/null || true; fi
    [ "$KEEP" = 1 ] || rm -rf "$WORK"
}
trap cleanup EXIT

# First existing firmware file among candidate names (Homebrew ships edk2-*.fd,
# Debian/Ubuntu ship AAVMF/OVMF).
qemu_share() {
    local d f
    for f in "$@"; do
        for d in "$(dirname "$(command -v qemu-img)")/../share/qemu" /usr/share/qemu /usr/share/AAVMF /usr/share/OVMF; do
            [ -e "$d/$f" ] && { echo "$d/$f"; return; }
        done
    done
    echo "firmware not found: $*" >&2; return 1
}

qemu-img create -q -f qcow2 -F qcow2 -b "$(cd "$(dirname "$DISK")" && pwd)/$(basename "$DISK")" "$WORK/overlay.qcow2"

accel=tcg
if [ "$ARCH" = "$(host_arch)" ]; then
    if [ "$(uname -s)" = Darwin ]; then accel=hvf; elif [ -w /dev/kvm ]; then accel=kvm; fi
fi
# BOOT_QEMU_ACCEL=tcg forces emulation (reproduces hosted arm64 runners, no KVM).
accel="${BOOT_QEMU_ACCEL:-$accel}"
cpu=max; [ "$accel" != tcg ] && cpu=host

# Host ports: pick free ones and report them.
free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
hostfwd=""; FWD_MAP=""
for p in ${FWD//,/ }; do
    hp="$(free_port)"; FWD_MAP="$FWD_MAP $p:$hp"
    hostfwd="$hostfwd,hostfwd=tcp:127.0.0.1:$hp-:$p"
done
hostport() { local e; for e in $FWD_MAP; do [ "${e%%:*}" = "$1" ] && { echo "${e#*:}"; return; }; done; return 1; }

VIRTIOFSD_PLANNED=""
if [ -n "$CLAIM_DIR" ] && [ "$(uname -s)" = Linux ] && [ "${CUA_BOOT_VIRTIOFS:-1}" != 0 ]; then
    for v in "${CUA_VIRTIOFSD:-}" /usr/libexec/virtiofsd /usr/lib/qemu/virtiofsd "$(command -v virtiofsd || true)"; do
        [ -n "$v" ] && [ -x "$v" ] && { VIRTIOFSD_PLANNED="$v"; break; }
    done
fi
seed=()
SSH_KEY="$WORK/id_ed25519"
if [ "$SSH" = 1 ]; then ssh-keygen -q -t ed25519 -N "" -f "$SSH_KEY"; fi
{
    mkdir -p "$WORK/seed"
    printf 'instance-id: cua-boot-%s\nlocal-hostname: cua-vm\n' "$$" >"$WORK/seed/meta-data"
    {
        echo "#cloud-config"
        if [ "$SSH" = 1 ]; then
            echo "users:"
            echo "  - name: cua"
            echo "    ssh_authorized_keys: [\"$(cat "$SSH_KEY.pub")\"]"
        fi
        if [ -n "$ENV_TOKEN" ]; then
            echo "write_files:"
            echo "  - {path: /etc/cua/env-token, permissions: '0640', owner: 'root:root', content: '$ENV_TOKEN'}"
            echo "runcmd:"
            echo "  - [systemctl, try-restart, cua-spacesd.service]"
        fi
        if [ -n "$CLAIM_DIR" ] && [ -z "$VIRTIOFSD_PLANNED" ]; then
            # 9p only (no virtiofsd): runs in cloud-init.service, before
            # cua-spacesd (After=cloud-init).
            echo "bootcmd:"
            echo "  - [sh, -c, \"mkdir -p /etc/systemd/system/run-cua.mount.d && printf '[Mount]\\\\nType=9p\\\\nOptions=ro,trans=virtio,version=9p2000.L,nosuid,nodev,noexec\\\\n' >/etc/systemd/system/run-cua.mount.d/10-local-9p.conf && systemctl daemon-reload && systemctl restart --no-block run-cua.mount cua-env-token-sync.service\"]"
        fi
    } >"$WORK/seed/user-data"
    if command -v hdiutil >/dev/null; then
        hdiutil makehybrid -quiet -iso -joliet -default-volume-name cidata -o "$WORK/seed.iso" "$WORK/seed"
    elif command -v cloud-localds >/dev/null; then
        cloud-localds "$WORK/seed.iso" "$WORK/seed/user-data" "$WORK/seed/meta-data"
    else
        genisoimage -quiet -output "$WORK/seed.iso" -volid cidata -joliet -rock "$WORK/seed"
    fi
    seed=(-drive "file=$WORK/seed.iso,format=raw,if=virtio,readonly=on")
}
VIRTIOFSD="$VIRTIOFSD_PLANNED"
if [ -n "$CLAIM_DIR" ] && [ -n "$VIRTIOFSD" ]; then
    "$VIRTIOFSD" --socket-path="$WORK/virtiofs.sock" --shared-dir="$CLAIM_DIR" --sandbox=none \
        --cache=auto >"$WORK/virtiofsd.log" 2>&1 &
    VIRTIOFSD_PID=$!
    for _ in $(seq 1 50); do [ -S "$WORK/virtiofs.sock" ] && break; sleep 0.1; done
    [ -S "$WORK/virtiofs.sock" ] || { echo "virtiofsd did not start:"; cat "$WORK/virtiofsd.log"; exit 1; }
    # vhost-user-fs needs guest RAM the daemon can map.
    seed+=(-object "memory-backend-memfd,id=mem,size=$MEM,share=on" -numa "node,memdev=mem"
           -chardev "socket,id=claimfs,path=$WORK/virtiofs.sock"
           -device "vhost-user-fs-pci,chardev=claimfs,tag=cua-claim-secrets")
    echo "    claim secrets: virtio-fs ($VIRTIOFSD)"
elif [ -n "$CLAIM_DIR" ]; then
    seed+=(-virtfs "local,path=$CLAIM_DIR,mount_tag=cua-claim-secrets,security_model=none,readonly=on,id=claimsecrets")
    echo "    claim secrets: virtio-9p"
fi

case "$ARCH" in
    arm64)
        code="$(qemu_share edk2-aarch64-code.fd AAVMF_CODE.fd)"; vars="$(qemu_share edk2-arm-vars.fd AAVMF_VARS.fd)"
        cp "$vars" "$WORK/vars.fd"
        machine=(qemu-system-aarch64 -machine virt -accel "$accel" -cpu "$cpu"
            -drive "if=pflash,format=raw,readonly=on,file=$code" -drive "if=pflash,format=raw,file=$WORK/vars.fd") ;;
    amd64)
        machine=(qemu-system-x86_64 -machine q35 -accel "$accel" -cpu "$cpu")
        if [ "$UEFI" = 1 ]; then
            code="$(qemu_share edk2-x86_64-code.fd OVMF_CODE_4M.fd OVMF_CODE.fd)"
            machine+=(-drive "if=pflash,format=raw,readonly=on,file=$code")
        fi ;;
    *) echo "unknown arch $ARCH" >&2; exit 2 ;;
esac

echo "==> booting $DISK ($ARCH, accel=$accel, firmware=$([ "$ARCH" = arm64 ] || [ "$UEFI" = 1 ] && echo uefi || echo bios))"
for e in $FWD_MAP; do echo "    guest :${e%%:*} -> 127.0.0.1:${e#*:}"; done
echo "    serial log: $LOG"
t0=$SECONDS
"${machine[@]}" -smp "$SMP" -m "$MEM" \
    -drive "file=$WORK/overlay.qcow2,format=qcow2,if=virtio" ${seed[@]+"${seed[@]}"} \
    -nic "user,model=virtio-net-pci$hostfwd" \
    -device virtio-rng-pci \
    -display none -serial "file:$LOG" -monitor none \
    >"$WORK/qemu.out" 2>&1 &
QEMU_PID=$!

rc=0
login=0
while [ $((SECONDS - t0)) -lt "$TIMEOUT" ]; do
    kill -0 "$QEMU_PID" 2>/dev/null || { echo "QEMU exited:"; cat "$WORK/qemu.out"; exit 1; }
    if grep -q "login:" "$LOG"; then login=1; break; fi
    sleep 2
done
if [ "$login" = 1 ]; then
    echo "  ok   serial login prompt after $((SECONDS - t0))s: $(grep -a 'login:' "$LOG" | tail -1 | tr -d '\r')"
else
    echo "  FAIL no login prompt within ${TIMEOUT}s; last serial lines:"; tail -20 "$LOG"; exit 1
fi

for p in ${PROBE//,/ }; do
    hp="$(hostport "$p")" || { echo "port $p not forwarded" >&2; exit 2; }
    up=0
    for _ in $(seq 1 90); do
        if python3 - "$hp" <<'PY' 2>/dev/null
import socket, sys
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=2)
s.settimeout(2)
try:
    data = s.recv(1)  # slirp accepts even when the guest port is closed; a real server talks or holds the line
except socket.timeout:
    data = b"?"
sys.exit(0 if data else 1)
PY
        then up=1; break; fi
        sleep 2
    done
    [ "$up" = 1 ] && echo "  ok   guest :$p reachable after $((SECONDS - t0))s" || { echo "  FAIL guest :$p not reachable"; rc=1; }
done

if [ -n "$RFB_OUT" ]; then
    if python3 "$HERE/tools/rfb_snapshot.py" "127.0.0.1:$(hostport 5901)" "$RFB_OUT"; then
        echo "  ok   RFB frame -> $RFB_OUT"
    else
        echo "  FAIL RFB frame"; rc=1
    fi
fi

if [ "$SSH" = 1 ] && hostport 22 >/dev/null; then
    # sshd comes up after the login prompt and :3211; under TCG (no KVM,
    # e.g. hosted arm64 runners) the banner exchange alone can take longer
    # than a short ConnectTimeout. Retry connection failures until the
    # --timeout budget is spent, with at least SSH_MIN_BUDGET left for it.
    # KVM/HVF keep the short timeouts, so a healthy guest stays fast.
    if [ "$accel" = tcg ]; then connect=45 pause=5 floor=300; else connect=10 pause=2 floor=60; fi
    floor="${SSH_MIN_BUDGET:-$floor}"
    deadline=$((t0 + TIMEOUT)); [ $((deadline - SECONDS)) -ge "$floor" ] || deadline=$((SECONDS + floor))
    if ssh_retry "$deadline" "$connect" "$pause" -- -i "$SSH_KEY" -p "$(hostport 22)" \
        -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
        cua@127.0.0.1 "$SSH_CMD"; then
        echo "  ok   ssh as cua after $((SECONDS - t0))s"
    else
        echo "  FAIL ssh after $((SECONDS - t0))s"; rc=1
    fi
fi

if [ -n "$RUN_CMD" ]; then
    for e in $FWD_MAP; do export "QEMU_FWD_${e%%:*}=${e#*:}"; done
    [ "$SSH" = 1 ] && export QEMU_SSH_KEY="$SSH_KEY"
    if bash -c "$RUN_CMD"; then echo "  ok   --run"; else echo "  FAIL --run"; rc=1; fi
fi

echo "==> boot smoke rc=$rc in $((SECONDS - t0))s"
if [ "$KEEP" = 1 ]; then echo "QEMU left running (pid $QEMU_PID, work dir $WORK)"; fi
exit "$rc"
