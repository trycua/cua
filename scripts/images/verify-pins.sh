#!/usr/bin/env bash
# `cua images release` verify step: the published pins hold exactly the
# pushed children, each disk is the doctored disk, and the indexes link.
#
#   verify-pins.sh --pins PINS.json --evidence DIR --arches a,b [--spacesd true|false]
#
# Reads EVIDENCE/pushed-<arch>/pushed.json, EVIDENCE/disk-<arch>.sha256 and,
# when present, EVIDENCE/attested.json (verify_pins.py checks each child).
# Needs crane.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PINS="" EVIDENCE="" ARCHES="" SPACESD=true
while [ $# -gt 0 ]; do
    case "$1" in
        --pins) PINS="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --arches) ARCHES="$2"; shift 2 ;;
        --spacesd) SPACESD="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
field() { python3 -c 'import json,sys;r=json.load(open(sys.argv[1]));print(r["repo"]+"@"+r[sys.argv[2]]["digest"])' "$PINS" "$1"; }
primary="$(field primary)" disk="$(field containerdisk)"
is_rootfs="$(python3 -c 'import json,sys;print(str(json.load(open(sys.argv[1])).get("primary_is_rootfs",False)).lower())' "$PINS")"
work="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-verify.XXXXXX")"
trap 'rm -rf "$work"' EXIT
ATTESTED="$EVIDENCE/attested.json"
attested=(); [ -f "$ATTESTED" ] && attested=(--attested "$ATTESTED")
# INDEX ARCH VARIANT WANT: the child is the pushed digest and carries a pass
# verdict (or none, when no doctor lane ran for it: verify_pins.py).
child() {
    crane manifest "$1" >"$work/index.json"
    python3 "$HERE/verify_pins.py" child "$work/index.json" --arch "$2" --variant "$3" --want "$4" ${attested[@]+"${attested[@]}"}
}
IFS=, read -r -a arches <<<"$ARCHES"
for arch in "${arches[@]}"; do
    rec="$EVIDENCE/pushed-$arch/pushed.json"
    want_disk="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["containerdisk"])' "$rec")"
    child "$disk" "$arch" containerdisk "$want_disk" >/dev/null
    if [ "$is_rootfs" = true ]; then
        want_rootfs="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["rootfs"])' "$rec")"
        child "$primary" "$arch" rootfs "$want_rootfs" >/dev/null
    fi
    read -r want _ <"$EVIDENCE/disk-$arch.sha256"
    mkdir -p "$work/$arch"
    crane export --platform "linux/$arch" "$disk" - | tar -x -C "$work/$arch" disk/disk.img
    got="$(sha256sum "$work/$arch/disk/disk.img" | cut -d' ' -f1)"
    [ "$got" = "$want" ] || { echo "$disk ($arch) disk $got is not the doctored $want" >&2; exit 1; }
    rm -rf "${work:?}/$arch"
    echo "$arch: disk.img $got"
done
crane manifest "$primary" | python3 -c '
import json, sys
a = json.load(sys.stdin)["annotations"]
v = json.loads(a["ai.cua.image.variants"])
assert v["containerdisk"] == sys.argv[1], v
assert a["ai.cua.spacesd"] == sys.argv[2], a
print("primary links the disk")' "$disk" "$SPACESD"
