#!/usr/bin/env bash
# `cua images release libs/images/macos --tier <tier>` steps (release.json).
# Each tier builds on the gated VM of the tier below, <prefix>-<tier below>,
# which an earlier run with the same --var prefix left on this Mac:
#
#   cua images release libs/images/macos --tier slim --var prefix=P [--var base_vm=cua-base-...]
#   cua images release libs/images/macos --tier full --var prefix=P
#   ... then with --publish (pins) and, once every tier is pinned, --promote.
#
#   release-tier.sh build   --tier T --prefix P --out DIR [--base-vm VM] [--stamp S]
#   release-tier.sh gate    --out DIR --evidence DIR
#   release-tier.sh push    --tier T --prefix P --stamp S [--pin TAG] --evidence DIR
#   release-tier.sh publish --tier T --stamp S [--pin TAG] --evidence DIR
#   release-tier.sh verify  --tier T --evidence DIR
#   release-tier.sh promote --tier T --evidence DIR
#
# build    build.sh --tier T --name P-T --keep (its strict doctor and ax probe
#          run inside; the VM is left stopped and sanitized). With --stamp,
#          a P-T that tiers.sh (or an earlier build) already built from the
#          stamp's revision and gated into DIR is reused, not rebuilt: a
#          `--resume --publish` after tiers.sh never deletes the gated VM.
# gate     the build's doctor verdict (strict, exit 0, ax probe pass) copied
#          to EVIDENCE/doctor/arm64-lume, where attest-doctor-reports.sh
#          finds it.
# push     push.sh P-T <pin>-raw -> EVIDENCE/raw.json.
# publish  annotate.sh <pin>-raw <pin> (tier, content key) -> EVIDENCE/pushed.json
#          ({arch, repo, lume: pin digest, ...}, the attest push record).
# verify   the pin still holds that digest, carries the cua annotations, has
#          the raw push's config and layers, its disk chunks cover the whole
#          disk (tools/disk-coverage.py), and has a doctor report attached.
# promote  the tier's floating tags (26-slim | 26 | 26-xcode, 26-xcode-X.Y)
#          move to the pin, through check-tag-safety.sh --moving.
#
# Pins: 26-slim-<stamp>, 26-<stamp>, 26-xcode-<XCODE_VERSION>-<stamp>; --pin
# (release.json's rendered tag) must name the same one.
# VM prefixes must start with cua-e2e- or cua-ci- (the only VMs this deletes).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
REPOSITORY=ghcr.io/trycua/macos
cmd="${1:?build|gate|push|publish|verify|promote}"; shift
TIER="" PREFIX="" OUT="" EVIDENCE="" STAMP="" BASE_VM="" PIN=""
while [ $# -gt 0 ]; do
    case "$1" in
        --tier) TIER="$2"; shift 2 ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --pin) PIN="$2"; shift 2 ;;
        --base-vm) BASE_VM="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
# shellcheck source=versions.env
. "$HERE/versions.env"
pin() {
    [[ "$STAMP" =~ ^[0-9]{8}-[0-9a-f]{7}$ ]] || { echo "--stamp is <yyyymmdd>-<sha7>" >&2; exit 2; }
    local p
    case "$TIER" in
        slim) p="26-slim-$STAMP" ;;
        full) p="26-$STAMP" ;;
        xcode) p="26-xcode-$XCODE_VERSION-$STAMP" ;;
        *) echo "--tier is slim, full or xcode" >&2; exit 2 ;;
    esac
    [ -z "$PIN" ] || [ "$PIN" = "$p" ] ||
        { echo "--pin $PIN is not the $TIER pin $p (release.json and versions.env disagree)" >&2; exit 2; }
    echo "$p"
}
vm_name() {
    [[ "$PREFIX" =~ ^cua-(e2e|ci)-[A-Za-z0-9._-]+$ ]] ||
        { echo "--prefix must start with cua-e2e- or cua-ci- (got '$PREFIX')" >&2; exit 2; }
    echo "$PREFIX-$1"
}
exists() { lume get "$1" --format json >/dev/null 2>&1; }
jget() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2"; }

case "$cmd" in
build)
    vm="$(vm_name "$TIER")"
    case "$TIER" in
        slim) base=(); [ -z "$BASE_VM" ] || base=(--base-vm "$BASE_VM") ;;
        full) base=(--base-vm "$(vm_name slim)") ;;
        xcode) base=(--base-vm "$(vm_name full)") ;;
        *) echo "--tier is slim, full or xcode" >&2; exit 2 ;;
    esac
    if [ "$TIER" != slim ] && ! exists "${base[1]}"; then
        echo "${base[1]} is missing: release the tier below first with the same --var prefix" >&2; exit 2
    fi
    if [ -n "$STAMP" ] && exists "$vm" && python3 - "$OUT" "$vm" "${STAMP#*-}" <<'PY'
import json, sys
out, vm, sha7 = sys.argv[1:]
try:
    size = json.load(open(f"{out}/size.json"))
    lane = json.load(open(f"{out}/doctor/lane.json"))
    info = json.load(open(f"{out}/doctor/build-info.json"))
    report = json.load(open(f"{out}/doctor/report.json"))
except (OSError, ValueError):
    sys.exit(1)
ok = (size.get("vm") == vm and lane.get("exit") == 0 and lane.get("ax_probe") == "pass"
      and report.get("summary", {}).get("status") == "pass"
      and str(info.get("git_sha", "")).startswith(sha7))
sys.exit(0 if ok else 1)
PY
    then
        echo "reusing $vm: built from ${STAMP#*-} and gated (its report is in $OUT/doctor)" >&2
        exit 0
    fi
    # A rerun rebuilds from the base (a failed build left the VM behind).
    if exists "$vm"; then echo "deleting the earlier $vm" >&2; lume delete "$vm" --force >/dev/null; fi
    mkdir -p "$OUT"
    "$HERE/build.sh" --tier "$TIER" --name "$vm" --keep --out "$OUT" ${base[@]+"${base[@]}"}
    lume get "$vm" --format json | python3 -c '
import json, sys
d = json.load(sys.stdin); d = d[0] if isinstance(d, list) else d
print(json.dumps({"vm": d["name"], "disk_used_bytes": d["diskSize"]["allocated"], "disk_total_bytes": d["diskSize"]["total"]}))
' >"$OUT/size.json"
    ;;
gate)
    d="$OUT/doctor"
    python3 - "$d" <<'PY'
import json, sys
d = sys.argv[1]
lane = json.load(open(f"{d}/lane.json"))
report = json.load(open(f"{d}/report.json"))
bad = []
if lane.get("exit") != 0: bad.append(f"doctor exit {lane.get('exit')}")
if lane.get("ax_probe") != "pass": bad.append(f"ax probe {lane.get('ax_probe')}")
if report.get("summary", {}).get("status") != "pass": bad.append(f"report {report.get('summary', {}).get('status')}")
if report.get("summary", {}).get("strict") is not True: bad.append("report is not strict")
if bad: sys.exit("the build's doctor gate did not pass: " + ", ".join(bad))
c = report["summary"]
print(f"doctor --strict: {c['pass']} passed, {c['warn']} warnings, {c['fail']} failed, {c['skip']} skipped; ax probe pass")
PY
    mkdir -p "$EVIDENCE/doctor/arm64-lume"
    cp "$d"/* "$EVIDENCE/doctor/arm64-lume/"
    ;;
push)
    p="$(pin)"; vm="$(vm_name "$TIER")"
    digest="$("$HERE/push.sh" "$vm" "$p-raw" | tail -1)"
    python3 -c 'import json,sys;json.dump({"tag":sys.argv[1],"digest":sys.argv[2],"vm":sys.argv[3]},open(sys.argv[4],"w"))' \
        "$p-raw" "$digest" "$vm" "$EVIDENCE/raw.json"
    cat "$EVIDENCE/raw.json"
    ;;
publish)
    p="$(pin)"
    [ "$(crane digest "$REPOSITORY:$p-raw")" = "$(jget "$EVIDENCE/raw.json" digest)" ] ||
        { echo "$p-raw no longer holds the pushed digest" >&2; exit 1; }
    key="$(python3 "$HERE/tools/content-key.py" "$EVIDENCE/doctor/arm64-lume" --image-json "$HERE/image.json" --tier "$TIER")"
    echo "$key" >"$EVIDENCE/content-key"
    digest="$("$HERE/annotate.sh" "$p-raw" "$p" --tier "$TIER" --content-key "$key")"
    python3 -c 'import json,sys;json.dump({"arch":"arm64","repo":sys.argv[1],"lume":sys.argv[2],"tag":sys.argv[3],"tier":sys.argv[4],"content_key":sys.argv[5]},open(sys.argv[6],"w"))' \
        "$REPOSITORY" "$digest" "$p" "$TIER" "$key" "$EVIDENCE/pushed.json"
    cat "$EVIDENCE/pushed.json"
    ;;
verify)
    rec="$EVIDENCE/pushed.json"
    tag="$(jget "$rec" tag)" digest="$(jget "$rec" lume)"
    [ "$(crane digest "$REPOSITORY:$tag")" = "$digest" ] || { echo "$tag no longer holds $digest" >&2; exit 1; }
    crane manifest "$REPOSITORY@$digest" >"$EVIDENCE/pin-manifest.json"
    crane manifest "$REPOSITORY@$(jget "$EVIDENCE/raw.json" digest)" >"$EVIDENCE/raw-manifest.json"
    python3 "$HERE/tools/disk-coverage.py" "$EVIDENCE/pin-manifest.json"
    "$ROOT/scripts/images/publish-doctor-report.sh" --list --subject "$REPOSITORY@$digest" >"$EVIDENCE/referrers.json"
    python3 - "$EVIDENCE" "$TIER" <<'PY'
import json, sys
e, tier = sys.argv[1], sys.argv[2]
pin = json.load(open(f"{e}/pin-manifest.json"))
raw = json.load(open(f"{e}/raw-manifest.json"))
rec = json.load(open(f"{e}/pushed.json"))
a = pin.get("annotations", {})
want = {"ai.cua.image.os": "macos", "ai.cua.spacesd": "true", "ai.cua.image.variant": "lume",
        "ai.cua.image.tier": tier, "ai.cua.image.content-key": rec["content_key"]}
bad = [f"{k}={a.get(k)!r} (want {v!r})" for k, v in want.items() if a.get(k) != v]
if [pin.get("config"), pin.get("layers")] != [raw.get("config"), raw.get("layers")]:
    bad.append("the pin's config/layers differ from the raw push")
refs = json.load(open(f"{e}/referrers.json"))
reports = [m for m in refs.get("manifests", refs.get("referrers", []))
           if m.get("annotations", {}).get("ai.cua.doctor.status") == "pass"]
if not reports:
    bad.append("no passing doctor report attached")
if bad:
    sys.exit("verify: " + "; ".join(bad))
print(f"verified {rec['repo']}:{rec['tag']} {rec['lume']} ({len(pin.get('layers', []))} layers, {len(reports)} doctor report(s))")
PY
    ;;
promote)
    rec="$EVIDENCE/pushed.json"
    tag="$(jget "$rec" tag)" digest="$(jget "$rec" lume)"
    [ "$(jget "$rec" tier)" = "$TIER" ] || { echo "$rec is not the $TIER tier" >&2; exit 1; }
    [ "$(crane digest "$REPOSITORY:$tag")" = "$digest" ] || { echo "$tag no longer holds $digest" >&2; exit 1; }
    case "$TIER" in
        slim) moving=(26-slim) ;;
        full) moving=(26) ;;
        xcode) moving=(26-xcode "26-xcode-$XCODE_VERSION") ;;
    esac
    for t in "${moving[@]}"; do
        before="$(crane digest "$REPOSITORY:$t" 2>/dev/null || echo none)"
        "$ROOT/scripts/images/check-tag-safety.sh" --moving --digest "$digest" "$REPOSITORY:$t"
        crane tag "$REPOSITORY@$digest" "$t"
        [ "$(crane digest "$REPOSITORY:$t")" = "$digest" ] || { echo "$t did not move" >&2; exit 1; }
        echo "promoted $REPOSITORY:$t $before -> $digest"
    done
    ;;
*) echo "unknown command $cmd" >&2; exit 2 ;;
esac
