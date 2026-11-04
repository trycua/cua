#!/usr/bin/env bash
# Build a bootable GPT disk (qcow2) from a VM rootfs tar. Runs inside the
# disk-builder image, unprivileged: every filesystem is created as a plain file
# (mkfs.ext4 -d / mkfs.vfat + mtools) and spliced into the disk at its partition
# offset, and GRUB is written with grub-mkstandalone (UEFI) and
# grub-mkimage + a hand-placed boot.img/core.img (BIOS, amd64).
#
# Layout (identical on both arches):
#   p1  BIOS boot   1 MiB    bios_grub (amd64 BIOS core.img; unused on arm64)
#   p2  ESP        64 MiB    FAT32 "UEFI", EFI/BOOT/BOOT{X64,AA64}.EFI
#   p3  root       rest      ext4 "cloudimg-rootfs" (cloud-init growpart grows it)
# amd64 boots under SeaBIOS (KubeVirt's default `bios`) and UEFI; arm64 boots
# UEFI only (KubeVirt/QEMU virt on arm64 is UEFI-only anyway).
#
# Usage: make-disk <rootfs.tar> <out.qcow2>
# Env:   DISK_SIZE (virtual size, default 20G), QCOW2_COMPRESS (1 = zlib -c, default 1)
set -euo pipefail
IN="${1:?rootfs tar}"
OUT="${2:?output qcow2}"
DISK_SIZE="${DISK_SIZE:-20G}"
COMPRESS="${QCOW2_COMPRESS:-1}"
ARCH="$(dpkg --print-architecture)"
W="${WORK:-/tmp/make-disk}"
rm -rf "$W"; mkdir -p "$W/root" "$W/efi"
log() { echo "[make-disk $(date +%T)] $*"; }

log "extracting $IN ($ARCH)"
tar --numeric-owner --xattrs --xattrs-include='*' -xpf "$IN" -C "$W/root"
R="$W/root"
test -e "$R/boot/vmlinuz" && test -e "$R/boot/initrd.img" \
    || { echo "rootfs has no /boot/vmlinuz + /boot/initrd.img; build it from common/vm/Dockerfile" >&2; exit 1; }

# Build-time bind mounts leak into `docker export`; fix them up for a real boot.
rm -f "$R/.dockerenv"
: >"$R/etc/machine-id"
echo localhost >"$R/etc/hostname"
rm -f "$R/etc/resolv.conf"; ln -s ../run/systemd/resolve/stub-resolv.conf "$R/etc/resolv.conf"
printf '127.0.0.1 localhost\n::1 localhost ip6-localhost ip6-loopback\n' >"$R/etc/hosts"
mkdir -p "$R/boot/efi" "$R/boot/grub"

case "$ARCH" in
    amd64) CONSOLE="console=tty1 console=ttyS0,115200n8"; EFI_NAME=BOOTX64.EFI; EFI_FMT=x86_64-efi ;;
    arm64) CONSOLE="console=tty1 console=ttyAMA0,115200n8"; EFI_NAME=BOOTAA64.EFI; EFI_FMT=arm64-efi ;;
    *) echo "unsupported arch $ARCH" >&2; exit 1 ;;
esac

cat >"$R/boot/grub/grub.cfg" <<CFG
# Written by libs/images/common/disk-builder/make-disk.sh
set timeout=1
set default=0
insmod part_gpt
insmod ext2
insmod gzio
$( [ "$ARCH" = amd64 ] && printf 'serial --unit=0 --speed=115200\nterminal_input console serial\nterminal_output console serial\n' )
search --no-floppy --label cloudimg-rootfs --set=root
menuentry "Ubuntu (cua image)" {
    linux /boot/vmlinuz root=LABEL=cloudimg-rootfs ro $CONSOLE ${KERNEL_EXTRA_ARGS:-}
    initrd /boot/initrd.img
}
CFG

# Stage-1 config embedded in every GRUB core: find the root fs by label and hand
# over to its grub.cfg, so kernel upgrades inside the guest keep working.
cat >"$W/early.cfg" <<'CFG'
insmod part_gpt
insmod ext2
search --no-floppy --label cloudimg-rootfs --set=root
set prefix=($root)/boot/grub
configfile ($root)/boot/grub/grub.cfg
CFG

log "building UEFI GRUB ($EFI_FMT)"
grub-mkstandalone -O "$EFI_FMT" -o "$W/$EFI_NAME" \
    --modules="part_gpt ext2 search search_label gzio linux normal configfile" \
    "boot/grub/grub.cfg=$W/early.cfg"

if [ "$ARCH" = amd64 ]; then
    log "building BIOS GRUB core"
    mkdir -p "$W/i386-pc"
    cp /usr/lib/grub/i386-pc/boot.img "$W/i386-pc/"
    grub-mkimage -O i386-pc -d /usr/lib/grub/i386-pc -o "$W/i386-pc/core.img" \
        -c "$W/early.cfg" -p /boot/grub \
        biosdisk part_gpt ext2 search search_label configfile normal linux gzio serial terminal echo test
    # Modules on the root fs too, so `insmod` from grub.cfg resolves at runtime.
    cp -r /usr/lib/grub/i386-pc "$R/boot/grub/"
fi

# Sizes in MiB.
to_mib() { numfmt --from=iec "$1" | awk '{printf "%d", $1/1048576}'; }
TOTAL=$(to_mib "$DISK_SIZE")
BIOS_START=1; BIOS_SIZE=1
ESP_START=2; ESP_SIZE=64
ROOT_START=$((ESP_START + ESP_SIZE)); ROOT_SIZE=$((TOTAL - ROOT_START - 1))
USED=$(du -sm --apparent-size "$R" | cut -f1)
[ "$ROOT_SIZE" -gt $((USED * 12 / 10 + 256)) ] \
    || { echo "DISK_SIZE $DISK_SIZE too small for ${USED} MiB rootfs" >&2; exit 1; }

log "ESP ${ESP_SIZE}MiB"
mkfs.vfat -F 32 -n UEFI -C "$W/esp.img" $((ESP_SIZE * 1024)) >/dev/null
mmd -i "$W/esp.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/esp.img" "$W/$EFI_NAME" "::/EFI/BOOT/$EFI_NAME"

log "root ext4 ${ROOT_SIZE}MiB from ${USED}MiB of files"
mkfs.ext4 -q -F -L cloudimg-rootfs -U "$(uuidgen)" -E root_owner=0:0 \
    -d "$R" "$W/root.img" "${ROOT_SIZE}M"

log "assembling ${TOTAL}MiB GPT disk"
truncate -s "${TOTAL}M" "$W/disk.raw"
sfdisk --quiet "$W/disk.raw" <<SF
label: gpt
start=${BIOS_START}MiB, size=${BIOS_SIZE}MiB, type=21686148-6449-6E6F-744E-656564454649, name=bios
start=${ESP_START}MiB, size=${ESP_SIZE}MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=esp
start=${ROOT_START}MiB, size=${ROOT_SIZE}MiB, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=root
SF
dd if="$W/esp.img" of="$W/disk.raw" bs=1M seek="$ESP_START" conv=notrunc,sparse status=none
dd if="$W/root.img" of="$W/disk.raw" bs=1M seek="$ROOT_START" conv=notrunc,sparse status=none
rm -f "$W/root.img" "$W/esp.img"

if [ "$ARCH" = amd64 ]; then
    # What grub-bios-setup does for an embedded core, done by hand because it
    # insists on resolving the root device of its -d directory (an overlayfs
    # here) even with --skip-fs-probe:
    #   MBR   boot.img code (bytes 0..439; the protective-MBR partition table and
    #         disk signature stay), kernel sector at 0x5c = core.img's LBA,
    #         drive check at 0x66 NOP'd (hard disk, not floppy).
    #   p1    core.img; its first sector (diskboot) ends in a blocklist
    #         {u64 start, u16 len, u16 segment} whose start must be LBA+1.
    log "installing BIOS boot code (core.img at LBA $((BIOS_START * 2048)))"
    le() { local v="$1" n="$2" i out=""; for ((i = 0; i < n; i++)); do out+="$(printf '\\x%02x' $(((v >> (8 * i)) & 255)))"; done; printf "$out"; }
    core_lba=$((BIOS_START * 2048))
    core="$W/i386-pc/core.img"
    core_sectors=$(( ($(stat -c %s "$core") + 511) / 512 ))
    [ "$core_sectors" -le $((BIOS_SIZE * 2048)) ] || { echo "core.img ($core_sectors sectors) exceeds the BIOS boot partition" >&2; exit 1; }
    dd if="$W/i386-pc/boot.img" of="$W/disk.raw" bs=1 count=440 conv=notrunc status=none
    le "$core_lba" 8 | dd of="$W/disk.raw" bs=1 seek=$((0x5c)) conv=notrunc status=none
    printf '\x90\x90' | dd of="$W/disk.raw" bs=1 seek=$((0x66)) conv=notrunc status=none
    le $((core_lba + 1)) 8 | dd of="$core" bs=1 seek=$((512 - 12)) conv=notrunc status=none
    le $((core_sectors - 1)) 2 | dd of="$core" bs=1 seek=$((512 - 4)) conv=notrunc status=none
    dd if="$core" of="$W/disk.raw" bs=512 seek="$core_lba" conv=notrunc status=none
fi

log "converting to qcow2 (compress=$COMPRESS)"
args=(-O qcow2 -m 8 -W)
[ "$COMPRESS" = 1 ] && args+=(-c)
qemu-img convert "${args[@]}" "$W/disk.raw" "$OUT.part"
mv "$OUT.part" "$OUT"
rm -rf "$W"
qemu-img info "$OUT"
