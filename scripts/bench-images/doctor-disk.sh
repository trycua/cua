#!/usr/bin/env bash
# The strict VM doctor of one arch's staged containerDisk child (from
# libs/images/bench/ID/lock.json), run through scripts/images/image-doctor-lane.sh
# --lane qemu: KVM/HVF when usable, else TCG with stretched budgets. This is
# the path for hosted arm64 runners, which have no KVM (the release's
# verify/<arch>/containerdisk step needs accel-<arch> and is skipped there).
# cd-image-linux.yml doctors its arm64 disks the same way.
#
#   doctor-disk.sh ID ARCH EVIDENCE_DIR
#
# Writes EVIDENCE_DIR/containerdisk-lane-ARCH.json (a smoke.py-shaped record
# keyed by the staged disk index) with its -doctor.json report, which is what
# attest.sh reads, plus the lane's logs under EVIDENCE_DIR/lane-containerdisk-ARCH/.
# Exit status: the lane's (0 pass).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
ID="${1:?usage: doctor-disk.sh ID ARCH EVIDENCE_DIR}"
ARCH="${2:?usage: doctor-disk.sh ID ARCH EVIDENCE_DIR}"
EVIDENCE="${3:?usage: doctor-disk.sh ID ARCH EVIDENCE_DIR}"
LOCK="$REPO_ROOT/libs/images/bench/$ID/lock.json"
[ -f "$LOCK" ] || { echo "no $LOCK (stage the pins first)" >&2; exit 2; }

repo="$(jq -r .repository "$LOCK")"
index="$repo@$(jq -r .disk_index.digest "$LOCK")"
child_digest="$(jq -r --arg a "$ARCH" '.children[] | select(.arch == $a) | .containerdisk' "$LOCK")"
[[ "$child_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "no $ARCH containerdisk child in $LOCK" >&2; exit 2; }
child="$repo@$child_digest"

mkdir -p "$EVIDENCE"
EVIDENCE="$(cd "$EVIDENCE" && pwd)"
work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/doctor-disk.XXXXXX")"
trap 'rm -rf "$work"' EXIT
echo "== $child (from $index): exporting disk/disk.img"
crane export --platform "linux/$ARCH" "$child" - | tar -x -C "$work" disk/disk.img

out="$EVIDENCE/lane-containerdisk-$ARCH"
started=$SECONDS
rc=0
"$REPO_ROOT/scripts/images/image-doctor-lane.sh" --lane qemu --disk "$work/disk/disk.img" --arch "$ARCH" --out "$out" || rc=$?

tag="containerdisk-lane-$ARCH"
[ -s "$out/report.json" ] && cp "$out/report.json" "$EVIDENCE/$tag-doctor.json"
python3 - "$EVIDENCE/$tag.json" "$ID" "$ARCH" "$child" "$index" "$rc" "$((SECONDS - started))" "$out/report.json" <<'PY'
import json, os, sys, time
path, bid, arch, child, index, rc, secs, report = sys.argv[1:]
status = None
if os.path.exists(report):
    try:
        status = json.load(open(report))["summary"]["status"]
    except (OSError, ValueError, KeyError):
        pass
passed = rc == "0" and status == "pass"
json.dump({"id": bid, "variant": "containerdisk", "where": "qemu-lane", "arch": arch, "image": child,
           "child": child, "container_runtime": None, "pinned_ref": index,
           "doctor": "pass" if passed else "fail", "seconds": int(secs), "ok": passed,
           "checks": [{"check": "doctor", "status": "pass" if passed else "fail",
                       "detail": f"image-doctor-lane.sh --lane qemu exit {rc}, report status {status}"}],
           "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())},
          open(path, "w"), indent=2)
PY
echo "== $([ "$rc" = 0 ] && echo PASS || echo FAIL) $tag -> $EVIDENCE/$tag.json"
exit "$rc"
