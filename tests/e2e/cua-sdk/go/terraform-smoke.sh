#!/usr/bin/env bash
# configure-pool-with-terraform, smoke only (plan §3.5): the provider in the
# read-only libs/fleet mirror is unchanged; we build its native Fleet SDK,
# run its unit tests, and (when TF_ACC=1 and KUBEBUILDER_ASSETS are set) its
# existing envtest acceptance test. Never edits libs/fleet.
#
# Records one line into $CUA_E2E_RESULTS/go.jsonl.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
PROVIDER="$REPO/libs/fleet/terraform-provider-fleets"
RUN="${CUA_E2E_RUN:-$(od -An -tx1 -N3 /dev/urandom | tr -d ' \n')}"
TARGET="${CUA_E2E_FLEET_SDK_TARGET:-$HERE/.target}"
t0=$SECONDS
lane="hermetic"; [ "${TF_ACC:-}" = 1 ] && lane="fleet"

record() {  # status reason
    [ -n "${CUA_E2E_RESULTS:-}" ] || return 0
    mkdir -p "$CUA_E2E_RESULTS"
    python3 - "$1" "$2" "$((SECONDS - t0))" "$lane" "$RUN" >>"$CUA_E2E_RESULTS/go.jsonl" <<'PY'
import json, sys
status, reason, secs, lane, run = sys.argv[1:6]
print(json.dumps({"scenario": "configure-pool-with-terraform", "lang": "go", "lane": lane,
                  "test": "provider unit tests" + (" + acceptance" if lane == "fleet" else ""),
                  "status": status, "secs": float(secs), "reason": reason, "run": run}))
PY
}

command -v go >/dev/null || { echo "skip: go not installed"; record skip "go is not installed"; exit 0; }
echo "==> building cyclops-sdk (native) from libs/fleet into $TARGET"
if ! CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --locked --manifest-path "$REPO/libs/fleet/Cargo.toml" \
        -p cyclops-sdk --target-dir "$TARGET" >&2; then
    record fail "cyclops-sdk native build failed"; exit 1
fi
lib="$TARGET/debug"
export CGO_CFLAGS="-I$REPO/libs/fleet/sdk-bindings/go-uniffi/fleet_sdk -I$REPO/libs/fleet/sdk-bindings/go-uniffi/cyclops_sdk_schema"
export CGO_LDFLAGS="-L$lib -lcyclops_sdk -Wl,-rpath,$lib"
export LD_LIBRARY_PATH="$lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
cd "$PROVIDER"
echo "==> go test ./internal/provider (unit)"
if ! timeout 900 go test ./internal/provider/; then record fail "provider unit tests failed"; exit 1; fi
if [ "${TF_ACC:-}" = 1 ]; then
    if [ -z "${KUBEBUILDER_ASSETS:-}" ]; then
        KUBEBUILDER_ASSETS="$(go run sigs.k8s.io/controller-runtime/tools/setup-envtest@release-0.19 use 1.31.0 -p path)" || {
            record fail "setup-envtest failed"; exit 1; }
        export KUBEBUILDER_ASSETS
    fi
    echo "==> acceptance (envtest)"
    if ! timeout 1800 go test -tags=acceptance ./internal/provider -run TestAccPoolLifecycle -v; then
        record fail "acceptance test failed"; exit 1
    fi
fi
record pass ""
