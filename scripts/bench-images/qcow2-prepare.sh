#!/usr/bin/env bash
# Shared first step for qcow2 sources: an overlay of the pristine upstream
# disk with every ext2/3/4 filesystem checked and repaired (e2fsck -fy), so
# the VM and container variants derive from the same, consistent tree.
#
#   qcow2-prepare.sh BASE.qcow2 OUT.qcow2 [REPORT.txt]
#
# Runs inside scripts/bench-images/guestfs. BASE is never written. The
# OSWorld disk, for example, has one inode whose metadata checksum does not
# match (its data is intact); reading that file fails with EBADMSG, which
# breaks `tar-out` until the checksum is rewritten.
set -euo pipefail
BASE="${1:?base qcow2}"; OUT="${2:?overlay out}"; REPORT="${3:-${2%.qcow2}.fsck.txt}"
export LIBGUESTFS_BACKEND="${LIBGUESTFS_BACKEND:-direct}"
[ -e /dev/kvm ] || export LIBGUESTFS_BACKEND_SETTINGS="${LIBGUESTFS_BACKEND_SETTINGS:-force_tcg}"
abs() { echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
rm -f "$OUT"
qemu-img create -q -f qcow2 -F qcow2 -b "$(abs "$BASE")" "$OUT"
fs="$(guestfish --ro -a "$OUT" run : list-filesystems | awk -F': ' '$2 ~ /^ext[234]$/ {print $1}')"
[ -n "$fs" ] || { echo "no ext filesystem in $BASE" >&2; exit 1; }
: >"$REPORT"
for dev in $fs; do
    echo "== e2fsck -fy $dev" | tee -a "$REPORT"
    # e2fsck exits 1 when it fixed something; anything above 2 is a failure.
    guestfish -a "$OUT" run : debug sh "e2fsck -fy $dev >/tmp/fsck.log 2>&1; echo rc=\$?; cat /tmp/fsck.log" \
        | tee -a "$REPORT"
    grep "^rc=" "$REPORT" | tail -1 | grep -qE '^rc=[012]$' || { echo "e2fsck failed on $dev" >&2; exit 1; }
done
echo "prepared $OUT"
