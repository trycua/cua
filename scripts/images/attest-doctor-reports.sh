#!/usr/bin/env bash
# doctor-attest for the image workflows (CI only: pushes to the registry and
# to the image-doctor-ledger branch).
#
#   attest-doctor-reports.sh attest RUN_URL RECORD
#       In ./attest (downloaded doctor-* and pushed-* artifacts): attach each
#       lane report to the pushed per-arch manifest it checked, write
#       attest/attested.json, and when RECORD=true add ledger entries.
#   attest-doctor-reports.sh canonical ROOTFS_REF DISK_REF RUN_URL
#       Attach the same reports to the canonical indexes' children (referrers
#       are per repository) and record them in the ledger.
#
# Fails when any lane report failed: nothing downstream (canonical tags)
# may run on a failing image.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
LEDGER_BRANCH="${CUA_DOCTOR_LEDGER_BRANCH:-image-doctor-ledger}"
ATTEST="${ATTEST_DIR:-$PWD/attest}"

ledger_checkout() {
    local dir="$ATTEST/ledger"
    ledger_remove "$dir"
    if git -C "$REPO_ROOT" fetch -q origin "$LEDGER_BRANCH" 2>/dev/null; then
        # Detached (ledger_push pushes HEAD:<branch>): a local branch would
        # stay checked out in this worktree and fail the next attest of a
        # local release that runs one tier after another.
        git -C "$REPO_ROOT" worktree add -q --detach "$dir" FETCH_HEAD
    else
        git -C "$REPO_ROOT" worktree add -q --detach "$dir"
        git -C "$dir" checkout -q --orphan "$LEDGER_BRANCH"
        # An orphan checkout keeps the whole tree staged: empty the index
        # and the worktree, or the first ledger commit carries the repo.
        git -C "$dir" read-tree --empty
        git -C "$dir" clean -fdxq
        if [ -n "$(git -C "$dir" ls-files | head -1)" ] || [ -n "$(ls -A "$dir" | grep -vx .git)" ]; then
            echo "error: the new ledger worktree is not empty" >&2
            return 1
        fi
        printf '# image-doctor ledger\n\nWritten by the image workflows only (scripts/images/doctor_ledger.py).\n' >"$dir/README.md"
    fi
    echo "$dir"
}

# Drop a ledger worktree (and its registration in the repository).
ledger_remove() {
    git -C "$REPO_ROOT" worktree remove --force "$1" >/dev/null 2>&1 || true
    rm -rf "$1"
    git -C "$REPO_ROOT" worktree prune
}

ledger_push() {
    # Push the ledger commit; when another job pushed first, rebuild the
    # commit on top of theirs by replaying this job's entries lane by lane
    # (doctor_ledger.py merge: one file per image digest, so jobs never
    # really conflict) instead of a git rebase. Bounded retries; every step
    # is checked explicitly so `set -e` never exits mid-retry.
    local dir="$1" message="$2" attempt mine remote
    git -C "$dir" add -A
    if git -C "$dir" diff --cached --quiet; then return 0; fi
    git -C "$dir" -c user.name="cua-image-doctor" -c user.email="noreply@github.com" commit -qm "$message"
    mine="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-ledger.XXXXXX")"
    # Only what this job's commit changed (a stale copy of another job's
    # entry must never overwrite its newer lanes).
    (cd "$dir" && git show --pretty=format: --name-only -z HEAD |
        xargs -0 -I{} sh -c '[ -f "$2" ] || exit 0; mkdir -p "$1/$(dirname "$2")" && cp "$2" "$1/$2"' _ "$mine" {})
    for attempt in 1 2 3 4 5 6; do
        if git -C "$dir" push -q origin "HEAD:$LEDGER_BRANCH"; then rm -rf "$mine"; return 0; fi
        echo "ledger push raced (attempt $attempt); replaying this job's entries on the pushed ledger" >&2
        sleep $((attempt * 3))
        if ! git -C "$dir" fetch -q origin "$LEDGER_BRANCH"; then continue; fi
        remote="$(git -C "$dir" rev-parse FETCH_HEAD)" || continue
        git -C "$dir" reset -q --hard "$remote" || continue
        python3 "$HERE/doctor_ledger.py" merge --from "$mine" --into "$dir" >/dev/null || continue
        git -C "$dir" add -A
        if git -C "$dir" diff --cached --quiet; then rm -rf "$mine"; return 0; fi
        git -C "$dir" -c user.name="cua-image-doctor" -c user.email="noreply@github.com" commit -qm "$message" || continue
    done
    rm -rf "$mine"
    echo "could not push the ledger" >&2
    return 1
}

cmd="${1:?attest|canonical}"; shift
case "$cmd" in
attest)
    run="${1:-}"; record="${2:-false}"
    ledger_args=()
    if [ "$record" = true ]; then ledger="$(ledger_checkout)"; ledger_args=(--ledger "$ledger"); fi
    entries="[]"
    failed=0
    while IFS=$'\t' read -r arch lane variant report lanejson subject; do
        out="$("$HERE/publish-doctor-report.sh" --subject "$subject" --report "$report" --lane "$lanejson" \
            ${ledger_args[@]+"${ledger_args[@]}"} --run "$run")"
        ref="$(sed -n 's/^report_ref=//p' <<<"$out")"
        status="$(sed -n 's/^status=//p' <<<"$out")"
        [ "$status" = pass ] || failed=1
        entries="$(python3 -c 'import json,sys;e=json.loads(sys.argv[1]);e.append(dict(zip(["arch","lane","variant","report","subject","report_ref","status"],sys.argv[2:])));print(json.dumps(e))' \
            "$entries" "$arch" "$lane" "$variant" "$report" "$subject" "$ref" "$status")"
        echo "attached $lane ($arch, $variant): $status -> $ref"
    done < <(python3 "$HERE/attest-doctor-reports.py" lanes "$ATTEST")
    printf '%s\n' "$entries" >"$ATTEST/attested.json"
    [ "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))))' "$ATTEST/attested.json")" -gt 0 ] \
        || { echo "no doctor reports to attest" >&2; exit 1; }
    if [ "$record" = true ]; then ledger_push "$ledger" "doctor: $(basename "$run")"; ledger_remove "$ledger"; fi
    [ "$failed" = 0 ] || { echo "a doctor lane failed; refusing to continue" >&2; exit 1; }
    ;;
canonical)
    rootfs_ref="$1"; disk_ref="$2"; run="$3"
    ledger="$(ledger_checkout)"
    for pair in "rootfs:$rootfs_ref" "containerdisk:$disk_ref"; do
        variant="${pair%%:*}"; index="${pair#*:}"
        repo="${index%:*}"
        # Children by architecture.
        while IFS=$'\t' read -r arch child; do
            while IFS=$'\t' read -r e_arch lane e_variant report lanejson _subject; do
                [ "$e_arch" = "$arch" ] && [ "$e_variant" = "$variant" ] || continue
                "$HERE/publish-doctor-report.sh" --subject "$repo@$child" --report "$report" --lane "$lanejson" \
                    --ledger "$ledger" --run "$run" >/dev/null
                echo "attached $lane to $repo@$child ($arch $variant)"
            done < <(python3 "$HERE/attest-doctor-reports.py" lanes "$ATTEST")
        done < <(crane manifest "$index" | python3 -c '
import json, sys
for m in json.load(sys.stdin).get("manifests", []):
    p = m.get("platform", {})
    if p.get("os") == "linux":
        print(p.get("architecture", ""), m["digest"], sep="\t")')
    done
    ledger_push "$ledger" "doctor: canonical $(basename "$run")"
    ledger_remove "$ledger"
    ;;
*) echo "unknown command $cmd" >&2; exit 2 ;;
esac
