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
