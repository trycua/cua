#!/usr/bin/env bash
# Tests publish-doctor-report.sh against a throwaway local registry:2
# (container cua-e2e-doctor-registry-<pid>, removed on exit): the report
# lands as a referrer under the tag schema (REPO:sha256-<hex>), carries the
# status annotations, is listed by --list, and is recorded in a ledger.
# Needs docker, oras, crane and python3. Never touches a real registry.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NAME="cua-e2e-doctor-registry-$$"
WORK="$(mktemp -d)"
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$WORK"; }
trap cleanup EXIT
pass=0; fail=0
ok() { pass=$((pass + 1)); echo "  ok   $*"; }
bad() { fail=$((fail + 1)); echo "  FAIL $*"; }

docker run -d --name "$NAME" --memory=256m -p 127.0.0.1::5000 registry:2 >/dev/null
PORT="$(docker port "$NAME" 5000/tcp | head -1 | sed 's/.*://')"
REG="127.0.0.1:$PORT"
for _ in $(seq 1 50); do curl -fs "http://$REG/v2/" >/dev/null && break; sleep 0.2; done

# A tiny image to attach to.
echo hello >"$WORK/hello.txt"
tar -C "$WORK" -cf "$WORK/layer.tar" hello.txt
crane append --insecure --new_layer "$WORK/layer.tar" -t "$REG/cua-e2e/linux:test" >/dev/null
digest="$(crane digest --insecure "$REG/cua-e2e/linux:test")"
subject="$REG/cua-e2e/linux@$digest"

cat >"$WORK/report.json" <<'JSON'
{"schema_version": 1, "producer": "cua-spacesd", "spacesd": {"version": "0.1.0", "protocol_revision": 3},
 "image": {"name": "linux", "variant": "rootfs", "os": "linux"},
 "environment": {"runtime": "gvisor", "arch": "arm64"},
 "summary": {"status": "pass", "pass": 3, "warn": 0, "fail": 0, "skip": 1, "duration_ms": 10, "strict": true},
 "checks": [], "fidelity": {}}
JSON
echo '{"lane": "runsc", "claim_secrets": false}' >"$WORK/lane.json"

out="$("$HERE/publish-doctor-report.sh" --plain-http --subject "$subject" --report "$WORK/report.json" \
    --lane "$WORK/lane.json" --ledger "$WORK/ledger" --run "https://example.invalid/run/1")"
ref="$(sed -n 's/^report_ref=//p' <<<"$out")"
[ -n "$ref" ] && ok "report pushed: $ref" || bad "no report_ref in: $out"

hex="${digest#sha256:}"
if crane ls --insecure "$REG/cua-e2e/linux" | grep -qx "sha256-$hex"; then
    ok "referrers tag schema index at sha256-$hex"
else
    bad "no sha256-$hex tag: $(crane ls --insecure "$REG/cua-e2e/linux" | tr '\n' ' ')"
fi
listed="$("$HERE/publish-doctor-report.sh" --plain-http --list --subject "$subject")"
if python3 - "$listed" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
refs = d.get("referrers") or d.get("manifests") or []
ok = len(refs) == 1 and refs[0].get("annotations", {}).get("ai.cua.doctor.status") == "pass" \
     and refs[0].get("annotations", {}).get("ai.cua.doctor.lane") == "runsc"
sys.exit(0 if ok else 1)
PY
then ok "--list shows the report with ai.cua.doctor.status=pass, lane runsc"; else bad "--list: $listed"; fi

# The artifact's blob is the report itself.
manifest="$(crane manifest --insecure "$ref")"
blob="$(python3 -c 'import json,sys;m=json.loads(sys.argv[1]);print(m["layers"][0]["digest"], m["subject"]["digest"], m["artifactType"])' "$manifest")"
read -r layer subj atype <<<"$blob"
[ "$subj" = "$digest" ] && ok "artifact subject is the image manifest" || bad "subject $subj != $digest"
[ "$atype" = "application/vnd.cua.doctor.report.v1+json" ] && ok "artifactType" || bad "artifactType $atype"
if crane blob --insecure "$REG/cua-e2e/linux@$layer" | cmp -s - "$WORK/report.json"; then
    ok "blob round-trips the report"; else bad "blob differs from the report"; fi

# Ledger.
if python3 "$HERE/doctor_ledger.py" status --ledger "$WORK/ledger" --repo "$REG/cua-e2e/linux" --digest "$digest" --lanes runsc >/dev/null; then
    ok "ledger records runsc=pass"; else bad "ledger status"; fi

# A subject that was never pushed is refused.
if "$HERE/publish-doctor-report.sh" --plain-http --subject "$REG/cua-e2e/linux@sha256:$(printf '0%.0s' $(seq 1 64))" \
    --report "$WORK/report.json" >/dev/null 2>&1; then
    bad "attached to a missing subject"; else ok "missing subject refused"; fi

# doctor-attest: lane reports + push records -> referrers and attested.json.
mkdir -p "$WORK/attest/doctor-x-arm64/arm64-runsc" "$WORK/attest/doctor-x-arm64/arm64-qemu" "$WORK/attest/pushed-x-arm64"
cp "$WORK/report.json" "$WORK/attest/doctor-x-arm64/arm64-runsc/report.json"
echo '{"lane": "runsc", "arch": "arm64", "variant": "rootfs", "claim_secrets": false, "exit": 0}' \
    >"$WORK/attest/doctor-x-arm64/arm64-runsc/lane.json"
cp "$WORK/report.json" "$WORK/attest/doctor-x-arm64/arm64-qemu/report.json"
echo '{"lane": "qemu", "arch": "arm64", "variant": "containerdisk", "claim_secrets": true, "exit": 0}' \
    >"$WORK/attest/doctor-x-arm64/arm64-qemu/lane.json"
printf '{"repo": "%s", "arch": "arm64", "rootfs": "%s", "containerdisk": "%s"}\n' "$REG/cua-e2e/linux" "$digest" "$digest" \
    >"$WORK/attest/pushed-x-arm64/pushed.json"
if (cd "$WORK" && CUA_DOCTOR_PLAIN_HTTP=1 "$HERE/attest-doctor-reports.sh" attest "https://example.invalid/run/2" false >/dev/null); then
    ok "attest attached every lane report"
else
    bad "attest failed"
fi
n="$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))))' "$WORK/attest/attested.json" 2>/dev/null || echo 0)"
[ "$n" = 2 ] && ok "attested.json lists 2 lanes" || bad "attested.json has $n entries"
ann="$(python3 "$HERE/attest-doctor-reports.py" descriptor-annotations "$WORK/attest/attested.json" --variant containerdisk --format imagetools)"
case "$ann" in
    *"manifest-descriptor[linux/arm64]:ai.cua.doctor.status=pass"*"ai.cua.doctor.report=$REG/cua-e2e/linux@sha256:"*) ok "descriptor annotations: $ann" ;;
    *) bad "descriptor annotations: $ann" ;;
esac
# The form `cua images publish --descriptor-annotation` takes (every variant).
ann="$(python3 "$HERE/attest-doctor-reports.py" descriptor-annotations "$WORK/attest/attested.json")"
case "$ann" in
    *"--descriptor-annotation"*"containerdisk/arm64:ai.cua.doctor.status=pass"*"rootfs/arm64:ai.cua.doctor.report=$REG/cua-e2e/linux@sha256:"*) ok "cua descriptor annotations" ;;
    *) bad "cua descriptor annotations: $ann" ;;
esac
# A failing lane stops attest (nothing downstream may run).
python3 - "$WORK/attest/doctor-x-arm64/arm64-runsc/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); r["summary"]["status"] = "fail"; json.dump(r, open(sys.argv[1], "w"))
PY
if (cd "$WORK" && CUA_DOCTOR_PLAIN_HTTP=1 "$HERE/attest-doctor-reports.sh" attest "https://example.invalid/run/3" false >/dev/null 2>&1); then
    bad "attest passed with a failing lane"
else
    ok "a failing lane fails attest"
fi

echo "==> $pass passed, $fail failed"
[ "$fail" = 0 ]
