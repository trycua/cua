#!/usr/bin/env bash
# Builds the disk of ghcr.io/trycua/runtime-gvisor for one arch: Colima's
# Ubuntu Docker disk (versions.env) with gVisor's runsc and sentry baked into
# /usr/local/bin, so the built-in Linux runtime's first boot downloads
# nothing. Colima registers runsc as Docker's default runtime from the
# profile cua-vmm writes (managed.rs), not from the disk.
#
#   sudo libs/images/runtime-gvisor/build.sh arm64 out/
#
# Runs on Linux as root (loop devices): a CI runner, or any Linux VM. Writes
# <out>/runtime-gvisor-<version>-<arch>.raw.gz and its .sha256. Only files are
# added to the base filesystem, so the build needs no emulation and one
# runner builds both arches.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=versions.env
. "$here/versions.env"

arch="${1:?usage: build.sh arm64|amd64 OUT_DIR}"
out="${2:?usage: build.sh arm64|amd64 OUT_DIR}"
case "$arch" in arm64 | amd64) ;; *) echo "arch must be arm64 or amd64" >&2; exit 2 ;; esac
[ "$(id -u)" = 0 ] || { echo "run as root (loop devices)" >&2; exit 2; }
for t in curl sha512sum bzip2 losetup; do command -v "$t" >/dev/null || { echo "needs $t" >&2; exit 2; }; done

base_url="BASE_URL_$arch"; base_sha="BASE_SHA512_$arch"
gv_url="GVISOR_URL_$arch"; gv_sha="GVISOR_SHA512_$arch"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d "${TMPDIR:-/var/tmp}/runtime-gvisor.XXXXXX")"
mnt="$work/root"
loop=""
cleanup() {
    mountpoint -q "$work/boot" 2>/dev/null && umount "$work/boot"
    mountpoint -q "$mnt" 2>/dev/null && umount "$mnt"
    [ -n "$loop" ] && losetup -d "$loop" 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT

fetch() { # url sha512 dest
    curl -fsSL --retry 3 -o "$3" "$1"
    echo "$2  $3" | sha512sum -c --quiet -
}

echo "==> base disk (colima-core $COLIMA_CORE_VERSION, $arch)"
fetch "${!base_url}" "${!base_sha}" "$work/base.raw.gz"
gzip -dc "$work/base.raw.gz" > "$work/disk.raw"
rm "$work/base.raw.gz"

echo "==> gVisor $GVISOR_VERSION"
fetch "${!gv_url}" "${!gv_sha}" "$work/gvisor.tar.bz2"
mkdir "$work/gvisor"
tar -xjf "$work/gvisor.tar.bz2" -C "$work/gvisor" runsc gvisor-bin/gvisor_sentry
rm "$work/gvisor.tar.bz2"

echo "==> bake"
loop="$(losetup --find --show --partscan "$work/disk.raw")"
udevadm settle 2>/dev/null || true
mkdir "$mnt"
# Partition 1 is the root filesystem (cloudimg layout: 1 root, 15 ESP, 16 /boot).
mount "${loop}p1" "$mnt"
install -d -m 0755 "$mnt/usr/local/bin/gvisor-bin" "$mnt/etc/cua-runtime"
install -m 0755 "$work/gvisor/runsc" "$mnt/usr/local/bin/"
install -m 0755 "$work/gvisor/gvisor-bin/gvisor_sentry" "$mnt/usr/local/bin/gvisor-bin/"
cat > "$mnt/etc/cua-runtime/release.json" <<EOF
{"image":"runtime-gvisor","version":"$RUNTIME_VERSION","arch":"$arch","base":"colima-core $COLIMA_CORE_VERSION","gvisor":"$GVISOR_VERSION","colima":"$COLIMA_VERSION","lima":"$LIMA_VERSION"}
EOF
# arm64: pointer authentication off, so gVisor memory checkpoints restore
# (gvisor#13542; cua_vmm::managed::AFTER_BOOT explains). The grub.d file
# keeps it across update-grub; the boot partition's grub.cfg (partition 16)
# gets it now, so the first boot already has it.
if [ "$arch" = arm64 ]; then
    echo 'GRUB_CMDLINE_LINUX_DEFAULT="$GRUB_CMDLINE_LINUX_DEFAULT arm64.nopauth"' \
        > "$mnt/etc/default/grub.d/99-cua-nopauth.cfg"
    boot="$work/boot"
    mkdir "$boot"
    mount "${loop}p16" "$boot"
    sed -i -E '/^[[:space:]]*linux[[:space:]]/ { / arm64\.nopauth/! s/$/ arm64.nopauth/ }' "$boot/grub/grub.cfg"
    grep -q 'arm64.nopauth' "$boot/grub/grub.cfg"
    umount "$boot"
fi
# amd64: Hyper-V boots this disk too (the Windows runtime, cua_vmm::managed::hyperv).
# Its disk controller is the synthetic SCSI one (a module, hv_storvsc) and the image has
# no initramfs: its kernel finds the root disk only through drivers built into it, so
# on Hyper-V it panics with "VFS: Cannot open root device". (On a generation 1 machine
# the emulated IDE disk is ignored by ata_piix on purpose.) Give it a small initramfs
# that carries the Hyper-V drivers (MODULES=list: only those, so the result does not
# depend on the machine that builds it), and let update-grub load it. The drivers are
# inert elsewhere. The arm64 disk, built without emulation, is left as it is.
if [ "$arch" = amd64 ]; then
    echo "==> initramfs with the Hyper-V drivers"
    mount "${loop}p16" "$mnt/boot"
    mount "${loop}p15" "$mnt/boot/efi"
    mount --bind /dev "$mnt/dev"
    mount -t proc proc "$mnt/proc"
    mount -t sysfs sysfs "$mnt/sys"
    cp /etc/resolv.conf "$mnt/etc/resolv.conf.cua-build"
    mv "$mnt/etc/resolv.conf" "$mnt/etc/resolv.conf.cua-orig" 2>/dev/null || true
    cp "$mnt/etc/resolv.conf.cua-build" "$mnt/etc/resolv.conf"
    chroot "$mnt" env DEBIAN_FRONTEND=noninteractive apt-get update -qq
    chroot "$mnt" env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends initramfs-tools
    sed -i 's/^MODULES=.*/MODULES=list/' "$mnt/etc/initramfs-tools/initramfs.conf"
    printf 'hv_vmbus\nhv_storvsc\n' >> "$mnt/etc/initramfs-tools/modules"
    chroot "$mnt" update-initramfs -c -k all
    # The cloud image tells grub to boot without an initrd (GRUB_FORCE_PARTUUID);
    # without that file grub loads the one just made.
    rm -f "$mnt/etc/default/grub.d/40-force-partuuid.cfg"
    chroot "$mnt" update-grub
    initrd="$(ls "$mnt"/boot/initrd.img-* | head -1)"
    contents="$(lsinitramfs "$initrd")"
    case "$contents" in *hv_storvsc*) ;; *) echo "$initrd has no hv_storvsc" >&2; exit 1 ;; esac
    grep -q '^[[:space:]]*initrd[[:space:]]' "$mnt/boot/grub/grub.cfg" || { echo "grub.cfg loads no initrd" >&2; exit 1; }
    ! grep -q 'initrdless' "$mnt/boot/grub/grub.cfg" || echo "note: grub.cfg still mentions initrdless" >&2
    chroot "$mnt" env DEBIAN_FRONTEND=noninteractive apt-get clean
    rm -rf "$mnt"/var/lib/apt/lists/*
    # The files just removed leave their blocks behind and gzip cannot shrink them:
    # fill what is free with zeros so the image compresses as it did before.
    dd if=/dev/zero of="$mnt/zero.fill" bs=1M 2>/dev/null || true
    dd if=/dev/zero of="$mnt/boot/zero.fill" bs=1M 2>/dev/null || true
    sync
    rm -f "$mnt/zero.fill" "$mnt/boot/zero.fill"
    rm -f "$mnt/etc/resolv.conf"
    mv "$mnt/etc/resolv.conf.cua-orig" "$mnt/etc/resolv.conf" 2>/dev/null || true
    rm -f "$mnt/etc/resolv.conf.cua-build"
    umount "$mnt/sys" "$mnt/proc" "$mnt/dev" "$mnt/boot/efi" "$mnt/boot"
fi
# The baked files are the verified ones.
cmp "$work/gvisor/runsc" "$mnt/usr/local/bin/runsc"
cmp "$work/gvisor/gvisor-bin/gvisor_sentry" "$mnt/usr/local/bin/gvisor-bin/gvisor_sentry"
sync
umount "$mnt"
losetup -d "$loop"
loop=""

echo "==> compress"
name="runtime-gvisor-$RUNTIME_VERSION-$arch.raw.gz"
if command -v pigz >/dev/null; then pigz -n -c "$work/disk.raw"; else gzip -n -c "$work/disk.raw"; fi > "$out/$name"
(cd "$out" && sha256sum "$name" > "$name.sha256")
cat "$out/$name.sha256"
ls -l "$out/$name"
