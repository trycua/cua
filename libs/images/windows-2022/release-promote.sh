#!/usr/bin/env bash
# `cua images release` promote step for windows-2022: move 2022-disk and 2022
# to the pins EVIDENCE/pushed.json names (after the pushed disk passed the
# strict doctor).
#
#   release-promote.sh --evidence DIR
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
EVIDENCE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --evidence) EVIDENCE="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
rec="$EVIDENCE/pushed.json"
[ -f "$rec" ] || { echo "no $rec (run the push step)" >&2; exit 2; }
j() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$rec" "$1"; }
STAMP="$(j stamp)" DISK="$(j disk_digest)" PRIMARY="$(j primary_digest)"
r=ghcr.io/trycua/windows
[ "$(crane digest "$r:2022-disk-$STAMP")" = "$DISK" ] && [ "$(crane digest "$r:2022-$STAMP")" = "$PRIMARY" ] \
    || { echo "the pins no longer hold the pushed digests" >&2; exit 1; }
echo "before: 2022-disk $(crane digest $r:2022-disk), 2022 $(crane digest $r:2022)"
"$ROOT/scripts/images/check-tag-safety.sh" --moving --digest "$DISK" "$r:2022-disk"
crane tag "$r@$DISK" 2022-disk
[ "$(crane digest "$r:2022-disk")" = "$DISK" ]
"$ROOT/scripts/images/publish-windows-primary.sh" --disk "$r@$DISK" --spacesd true --stamp "$STAMP" --promote
[ "$(crane digest "$r:2022")" = "$PRIMARY" ]
echo "promoted: $r:2022-disk -> $DISK, $r:2022 -> $PRIMARY"
