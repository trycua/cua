#!/usr/bin/env bash
# OSWorld VM variant: the upstream Ubuntu 22.04 qcow2 plus cua-spacesd.
#
#   customize.sh BASE.qcow2 CUA_SPACESD OUT_DIR
#
# Runs inside scripts/bench-images/guestfs (libguestfs, LIBGUESTFS_BACKEND=direct).
# Only guestfish file operations are used: no guest binary is executed, so an
# amd64 disk is customized identically on an arm64 host, with or without KVM,
# and the upstream software (apps, versions, the OSWorld server on :5000) is
# untouched. Adds:
#   - /usr/local/bin/cua-spacesd (+ cua-env-driver and cua-guestd links), the shared token
#     scripts from libs/images/linux/files, and
#     cua-spacesd.service running as `user` in the GDM Xorg session on :0;
#   - Fleet claim secrets: run-cua.mount (virtiofs cua-claim-secrets),
#     cua-env-token-sync.service and its udev rule;
#   - cua-seed-token.service: the image has no cloud-init, so the spacesd
#     token of a local SDK boot is read from its NoCloud seed (token and
#     spacesd.env only; ignored when a Fleet claim share is present);
#   - netplan DHCP on any en*/eth* NIC (KubeVirt), cloud-init network off;
#   - /dev/uinput for the `input` group, `user` in `input`;
#   - /etc/X11/xorg.conf.d/10-cua-noblank.conf: no X screen saver or DPMS;
#   - console=ttyS0 on the kernel command line;
#   - /etc/cua-image/{variant,image.json,spacesd-source}.
# BASE is never modified: the work happens on an overlay, which is then
# trimmed and flattened into OUT_DIR/disk.img (compressed qcow2).
set -euo pipefail
BASE="${1:?base qcow2}"; SPACESD="${2:?cua-spacesd binary}"; OUT="${3:?out dir}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$HERE/../../../../.." && pwd)"
DESK="$REPO/libs/images/linux/files"
WORK="$OUT/work"
mkdir -p "$WORK" "$OUT"
export LIBGUESTFS_BACKEND="${LIBGUESTFS_BACKEND:-direct}"
[ -e /dev/kvm ] || export LIBGUESTFS_BACKEND_SETTINGS="${LIBGUESTFS_BACKEND_SETTINGS:-force_tcg}"
log() { echo "[osworld-vm $(date +%T)] $*" >&2; }

file "$SPACESD" | grep -q 'x86-64' || { echo "$SPACESD is not an x86-64 binary" >&2; exit 1; }

overlay="$WORK/overlay.qcow2"
rm -f "$overlay"
qemu-img create -q -f qcow2 -F qcow2 -b "$(cd "$(dirname "$BASE")" && pwd)/$(basename "$BASE")" "$overlay"

# 1. Read what must be edited rather than replaced.
log "inspect"
guestfish --ro -a "$BASE" -i <<EOF
download /etc/group $WORK/group
download /boot/grub/grub.cfg $WORK/grub.cfg
download /etc/os-release $WORK/os-release
EOF
grep -q 'VERSION_ID="22.04"' "$WORK/os-release" || { echo "unexpected base: $(cat "$WORK/os-release")" >&2; exit 1; }
python3 - "$WORK" <<'PY'
import re, sys
w = sys.argv[1]
lines = open(f"{w}/group").read().splitlines()
out = []
for l in lines:
    f = l.split(":")
    if f[0] == "input":
        members = [m for m in f[3].split(",") if m]
        if "user" not in members:
            members.append("user")
        f[3] = ",".join(members)
        l = ":".join(f)
    out.append(l)
open(f"{w}/group.new", "w").write("\n".join(out) + "\n")
g = open(f"{w}/grub.cfg").read()
g2 = re.sub(r"(^\s*linux\s+\S+vmlinuz\S*[^\n]*?)(\s*)$",
            lambda m: m.group(1) if "console=ttyS0" in m.group(1) else m.group(1) + " console=tty0 console=ttyS0,115200n8",
            g, flags=re.M)
open(f"{w}/grub.cfg.new", "w").write(g2)
print("input group:", [l for l in out if l.startswith("input:")][0], file=sys.stderr)
print("grub linux lines patched:", g2.count("console=ttyS0,115200n8"), file=sys.stderr)
PY

printf 'vm\n' >"$WORK/variant"
printf 'local\n' >"$WORK/spacesd-source"

# 2. Apply.
log "customize overlay"
guestfish -a "$overlay" -i <<EOF
mkdir-p /opt/cua/bin
mkdir-p /etc/cua-image
mkdir-p /etc/systemd/system/multi-user.target.wants
mkdir-p /etc/systemd/system/cua-spacesd.service.d
mkdir-p /etc/systemd/system/cua-env-token-sync.service.d
mkdir-p /etc/cloud/cloud.cfg.d
mkdir-p /etc/modules-load.d
mkdir-p /etc/X11/xorg.conf.d
upload $SPACESD /usr/local/bin/cua-spacesd
chmod 0755 /usr/local/bin/cua-spacesd
ln-sf cua-spacesd /usr/local/bin/cua-env-driver
ln-sf cua-spacesd /usr/local/bin/cua-guestd
upload $DESK/start-spacesd.sh /opt/cua/bin/start-spacesd.sh
upload $DESK/ensure-env-token.sh /opt/cua/bin/ensure-env-token.sh
upload $DESK/env-token-mode.sh /opt/cua/bin/env-token-mode.sh
upload $DESK/start-token-sync.sh /opt/cua/bin/start-token-sync.sh
upload $HERE/files/cua-osworld-session-env.sh /opt/cua/bin/cua-osworld-session-env.sh
upload $HERE/files/cua-seed-token.py /opt/cua/bin/cua-seed-token.py
chmod 0755 /opt/cua/bin/cua-seed-token.py
upload $HERE/files/cua-seed-token.service /etc/systemd/system/cua-seed-token.service
ln-sf /etc/systemd/system/cua-seed-token.service /etc/systemd/system/multi-user.target.wants/cua-seed-token.service
chmod 0755 /opt/cua/bin/start-spacesd.sh
chmod 0755 /opt/cua/bin/ensure-env-token.sh
chmod 0755 /opt/cua/bin/env-token-mode.sh
chmod 0755 /opt/cua/bin/start-token-sync.sh
chmod 0755 /opt/cua/bin/cua-osworld-session-env.sh
ln-sf start-spacesd.sh /opt/cua/bin/start-env-driver.sh
ln-sf start-spacesd.sh /opt/cua/bin/start-guestd.sh
upload $DESK/systemd/cua-spacesd.service /etc/systemd/system/cua-spacesd.service
upload $DESK/systemd/cua-env-token-sync.service /etc/systemd/system/cua-env-token-sync.service
upload $DESK/systemd/run-cua.mount /etc/systemd/system/run-cua.mount
upload $DESK/udev/70-cua-claim-secrets.rules /etc/udev/rules.d/70-cua-claim-secrets.rules
upload $HERE/files/cua-spacesd-osworld.conf /etc/systemd/system/cua-spacesd.service.d/10-osworld.conf
upload $HERE/files/cua-env-token-sync-osworld.conf /etc/systemd/system/cua-env-token-sync.service.d/10-osworld.conf
ln-sf /etc/systemd/system/cua-spacesd.service /etc/systemd/system/multi-user.target.wants/cua-spacesd.service
ln-sf /etc/systemd/system/cua-env-token-sync.service /etc/systemd/system/multi-user.target.wants/cua-env-token-sync.service
ln-sf /etc/systemd/system/cua-spacesd.service /etc/systemd/system/cua-env-driver.service
ln-sf /etc/systemd/system/cua-spacesd.service /etc/systemd/system/cua-guestd.service
upload $HERE/files/99-cua-dhcp.yaml /etc/netplan/99-cua-dhcp.yaml
chmod 0600 /etc/netplan/99-cua-dhcp.yaml
upload $HERE/files/99-cua-disable-network-config.cfg /etc/cloud/cloud.cfg.d/99-disable-network-config.cfg
upload $HERE/files/99-cua-uinput.rules /etc/udev/rules.d/99-cua-uinput.rules
upload $HERE/files/uinput.conf /etc/modules-load.d/cua-uinput.conf
upload $HERE/files/10-cua-noblank.conf /etc/X11/xorg.conf.d/10-cua-noblank.conf
upload $WORK/group.new /etc/group
upload $WORK/grub.cfg.new /boot/grub/grub.cfg
upload $BENCH/bench.json /etc/cua-image/bench.json
upload $WORK/variant /etc/cua-image/variant
upload $WORK/spacesd-source /etc/cua-image/spacesd-source
ln-sf spacesd-source /etc/cua-image/env-driver-source
ln-sf spacesd-source /etc/cua-image/guestd-source
EOF
# What the image claims, for `cua-spacesd doctor` (build-qcow2.sh generates it).
[ -f "$OUT/manifest.json" ] || { echo "missing $OUT/manifest.json" >&2; exit 1; }
guestfish -a "$overlay" -i <<EOF
upload $OUT/manifest.json /etc/cua-image/manifest.json
upload $BENCH/image.json /etc/cua-image/image.json
EOF

# 3. Verify, then trim free space and flatten.
log "verify"
guestfish --ro -a "$overlay" -i <<'EOF' | tee "$WORK/verify.txt"
is-file /usr/local/bin/cua-spacesd
is-symlink /etc/systemd/system/multi-user.target.wants/cua-spacesd.service
cat /etc/systemd/system/cua-spacesd.service.d/10-osworld.conf
is-file /home/user/server/main.py
EOF
[ "$(grep -c '^true$' "$WORK/verify.txt")" = 3 ] || { echo "verification failed" >&2; exit 1; }
log "sparsify (fstrim) + flatten"
virt-sparsify --in-place "$overlay"
qemu-img convert -O qcow2 -c -p -m 8 -W "$overlay" "$OUT/disk.img"
rm -f "$overlay"
qemu-img info "$OUT/disk.img"
log "done: $OUT/disk.img"
