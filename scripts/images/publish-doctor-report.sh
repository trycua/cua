#!/usr/bin/env bash
# Attach a doctor report to the image manifest it describes, and record it.
#
#   publish-doctor-report.sh --subject REPO@sha256:<digest> --report report.json
#       [--lane lane.json] [--ledger DIR] [--run URL] [--plain-http]
#   publish-doctor-report.sh --list --subject REPO@sha256:<digest> [--plain-http]
#
# The report is pushed as an OCI artifact (artifactType
# application/vnd.cua.doctor.report.v1+json) whose `subject` is the image
# manifest. ghcr has no OCI 1.1 referrers API, so oras maintains the
# referrers *tag schema* index at REPO:sha256-<hex>; registries with the API
# (ECR) get the same artifact through it. Annotations on the artifact:
# ai.cua.doctor.status, ai.cua.doctor.lane, ai.cua.doctor.spacesd,
# org.opencontainers.image.created.
#
# Prints `report_ref=<repo>@<artifact digest>` (also to $GITHUB_OUTPUT).
# With --ledger, records the verdict in the image-doctor ledger
# (scripts/images/doctor_ledger.py record).
#
# Needs oras and crane, and registry credentials for pushes (CI only; tests
# use a throwaway local registry:2 with --plain-http).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARTIFACT_TYPE="application/vnd.cua.doctor.report.v1+json"
SUBJECT="" REPORT="" LANE="" LEDGER="" RUN="" PLAIN=() LIST=0
while [ $# -gt 0 ]; do
    case "$1" in
        --subject) SUBJECT="$2"; shift 2 ;;
        --report) REPORT="$2"; shift 2 ;;
        --lane) LANE="$2"; shift 2 ;;
        --ledger) LEDGER="$2"; shift 2 ;;
        --run) RUN="$2"; shift 2 ;;
        --plain-http) PLAIN=(--plain-http); shift ;;
        --list) LIST=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
# Throwaway local registries (tests): plain HTTP.
[ "${CUA_DOCTOR_PLAIN_HTTP:-0}" = 1 ] && PLAIN=(--plain-http)
case "$SUBJECT" in
    *@sha256:*) ;;
    *) echo "--subject must be digest-pinned (REPO@sha256:...)" >&2; exit 2 ;;
esac
repo="${SUBJECT%@*}"
digest="${SUBJECT#*@}"
crane_insecure=(); [ ${#PLAIN[@]} -gt 0 ] && crane_insecure=(--insecure)

if [ "$LIST" = 1 ]; then
    oras discover "${PLAIN[@]+"${PLAIN[@]}"}" --distribution-spec v1.1-referrers-tag \
        --artifact-type "$ARTIFACT_TYPE" --format json "$SUBJECT"
    exit 0
fi
[ -s "$REPORT" ] || { echo "--report must name a non-empty report.json" >&2; exit 2; }

# The subject must exist; never attach to something that was not pushed.
crane digest "${crane_insecure[@]+"${crane_insecure[@]}"}" "$SUBJECT" >/dev/null

read -r status lane spacesd < <(python3 - "$REPORT" "$LANE" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
if r.get("schema_version") != 1:
    sys.exit("report schema_version must be 1")
lane = ""
if sys.argv[2]:
    info = json.load(open(sys.argv[2]))
    lane = info.get("lane", "") + ("-claim-secrets" if info.get("claim_secrets") else "")
lane = lane or r.get("environment", {}).get("runtime", "unknown")
# Reports from images built before the rename say "guestd".
print(r["summary"]["status"], lane, (r.get("spacesd") or r.get("guestd") or {}).get("version", "none"))
PY
)
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$REPORT" "$work/report.json"
created="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
out="$(cd "$work" && oras attach "${PLAIN[@]+"${PLAIN[@]}"}" --distribution-spec v1.1-referrers-tag \
    --artifact-type "$ARTIFACT_TYPE" \
    --annotation "ai.cua.doctor.status=$status" \
    --annotation "ai.cua.doctor.lane=$lane" \
    --annotation "ai.cua.doctor.spacesd=$spacesd" \
    --annotation "org.opencontainers.image.created=$created" \
    --format json "$SUBJECT" "report.json:$ARTIFACT_TYPE")"
artifact="$(python3 -c 'import json,sys;print(json.loads(sys.stdin.read())["digest"])' <<<"$out")"
report_ref="$repo@$artifact"
echo "report_ref=$report_ref"
echo "status=$status"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
    { echo "report_ref=$report_ref"; echo "status=$status"; } >>"$GITHUB_OUTPUT"
fi
if [ -n "$LEDGER" ]; then
    lane_args=(); [ -n "$LANE" ] && lane_args=(--lane "$LANE")
    python3 "$HERE/doctor_ledger.py" record --ledger "$LEDGER" --repo "$repo" --digest "$digest" \
        --report "$REPORT" "${lane_args[@]+"${lane_args[@]}"}" --report-ref "$report_ref" --run "$RUN"
fi
