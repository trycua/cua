#!/usr/bin/env bash
# `cua images release` push step for windows-2022: the doctored disk as the
# new immutable pins 2022-disk-<stamp> and 2022-<stamp> (the primary).
#
#   release-push.sh --out DIR --stamp STAMP --evidence DIR
#
# Checks DIR/disk.img is the disk the doctor passed (EVIDENCE/disk.sha256),
# then writes EVIDENCE/pushed.json {stamp, disk_digest, primary_digest} and
# EVIDENCE/pushed-amd64/pushed.json (the attest subject: the disk child).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="" STAMP="" EVIDENCE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="$2"; shift 2 ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -f "$OUT/disk.img" ] && [ -n "$STAMP" ] && [ -n "$EVIDENCE" ] || { echo "usage: see the header" >&2; exit 2; }
read -r want _ <"$EVIDENCE/disk.sha256"
got="$(sha256sum "$OUT/disk.img" | cut -d' ' -f1)"
[ "$got" = "$want" ] || { echo "$OUT/disk.img ($got) is not the doctored disk ($want)" >&2; exit 1; }
"$HERE/push-disk.sh" --disk-img "$OUT/disk.img" --stamp "$STAMP" --manifest "$OUT/manifest.json" | tee "$EVIDENCE/push.txt"
disk="$(sed -n 's/^disk_digest=//p' "$EVIDENCE/push.txt")"
"$ROOT/scripts/images/publish-windows-primary.sh" --disk "ghcr.io/trycua/windows@$disk" --spacesd true --stamp "$STAMP" \
    | tee "$EVIDENCE/primary.txt"
primary="$(sed -n 's/^primary_digest=//p' "$EVIDENCE/primary.txt")"
child="$(sed -n 's/^child_digest=//p' "$EVIDENCE/push.txt")"
python3 -c 'import json,sys;json.dump({"stamp":sys.argv[1],"disk_digest":sys.argv[2],"primary_digest":sys.argv[3]},open(sys.argv[4],"w"))' \
    "$STAMP" "$disk" "$primary" "$EVIDENCE/pushed.json"
# The subject the attest step attaches doctor reports to (and the ledger
# records): the amd64 child both indexes (2022-disk and the 2022 primary)
# list, in the shape scripts/images/attest-doctor-reports.py reads.
mkdir -p "$EVIDENCE/pushed-amd64"
python3 -c 'import json,sys;json.dump({"arch":"amd64","repo":"ghcr.io/trycua/windows","containerdisk":sys.argv[1]},open(sys.argv[2],"w"))' \
    "$child" "$EVIDENCE/pushed-amd64/pushed.json"
cat "$EVIDENCE/pushed.json"
