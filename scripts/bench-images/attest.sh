#!/usr/bin/env bash
# Attest a benchmark image's staged pins: attach the doctor report of every
# arch and variant to the per-arch child it checked, and with --record true
# record it on the image-doctor ledger (scripts/images/attest-doctor-reports.sh).
#
#   attest.sh ID --evidence DIR [--evidence DIR ...] --out DIR [--run URL] [--record true|false]
#       [--layout-only]
#
# Reads smoke.py evidence (<tag>.json with its <tag>-doctor.json) under each
# --evidence DIR. Only records for the exact staged digests in
# libs/images/bench/ID/lock.json count (rootfs: the index, containerdisk: the
# disk index), passing with `cua-spacesd doctor --strict`. Every arch and
# variant of the lock needs one, else nothing is attached. --out gets the
# lane layout attest-doctor-reports.sh reads (pushed-<arch>/pushed.json from
# the lock's children, <variant>-<arch>/{report,lane}.json) and attested.json.
# --layout-only stops after the layout (no registry or ledger access).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
ID="${1:?usage: attest.sh ID --evidence DIR --out DIR [--run URL] [--record true|false]}"; shift
EVIDENCE=() OUT="" RUN="local" RECORD=false LAYOUT_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --evidence) EVIDENCE+=("$2"); shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --run) RUN="$2"; shift 2 ;;
        --record) RECORD="$2"; shift 2 ;;
        --layout-only) LAYOUT_ONLY=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ ${#EVIDENCE[@]} -gt 0 ] && [ -n "$OUT" ] || { echo "--evidence and --out are required" >&2; exit 2; }
LOCK="$REPO_ROOT/libs/images/bench/$ID/lock.json"
[ -f "$LOCK" ] || { echo "no $LOCK (stage the pins first)" >&2; exit 2; }
rm -rf "$OUT"; mkdir -p "$OUT"
python3 - "$LOCK" "$OUT" "${EVIDENCE[@]}" <<'PY'
import glob, json, os, shutil, sys

lock_path, out, *dirs = sys.argv[1:]
lock = json.load(open(lock_path))
repo = lock["repository"]
want = {"rootfs": f"{repo}@{lock['index']['digest']}",
        "containerdisk": f"{repo}@{lock['disk_index']['digest']}"}
found = {}
for d in dirs:
    for path in sorted(glob.glob(os.path.join(d, "*.json"))):
        if path.endswith(("-doctor.json", "-capabilities.json")):
            continue
        try:
            rec = json.load(open(path))
        except (OSError, json.JSONDecodeError):
            continue
        if not isinstance(rec, dict):
            continue
        variant, arch = rec.get("variant"), rec.get("arch")
        if variant not in want or rec.get("pinned_ref") != want[variant]:
            continue
        if rec.get("ok") is not True or rec.get("doctor") != "pass":
            continue
        report = path[: -len(".json")] + "-doctor.json"
        if os.path.exists(report):
            found[(variant, arch)] = (report, rec)
missing = []
for child in lock["children"]:
    arch = child["arch"]
    os.makedirs(os.path.join(out, f"pushed-{arch}"), exist_ok=True)
    json.dump({"arch": arch, "repo": repo, "rootfs": child["rootfs"], "containerdisk": child["containerdisk"]},
              open(os.path.join(out, f"pushed-{arch}", "pushed.json"), "w"))
    for variant in ("rootfs", "containerdisk"):
        hit = found.get((variant, arch))
        if hit is None:
            missing.append(f"{variant}/{arch} ({want[variant]})")
            continue
        report, rec = hit
        lane_dir = os.path.join(out, f"{variant}-{arch}")
        os.makedirs(lane_dir, exist_ok=True)
        shutil.copy(report, os.path.join(lane_dir, "report.json"))
        lane = "qemu" if variant == "containerdisk" else "container"
        json.dump({"lane": lane, "image": rec["pinned_ref"], "arch": arch, "variant": variant,
                   "claim_secrets": False, "exit": 0}, open(os.path.join(lane_dir, "lane.json"), "w"))
if missing:
    sys.exit("no passing doctor evidence for the staged pins: " + ", ".join(missing))
PY
[ "$LAYOUT_ONLY" = 0 ] || { find "$OUT" -name '*.json' | sort; exit 0; }
ATTEST_DIR="$OUT" "$REPO_ROOT/scripts/images/attest-doctor-reports.sh" attest "$RUN" "$RECORD"
