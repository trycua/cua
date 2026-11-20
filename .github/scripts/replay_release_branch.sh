#!/usr/bin/env bash
# Replay a Release Please branch (checked out) onto origin/main.
#
#   replay_release_branch.sh REMOTE_BRANCH_REF [--component NAME ...]
#
# The branch carries Release Please's release commit and, after an earlier run,
# a "chore(...): synchronize ..." commit with generated files. Replaying it can
# stop on:
#   - the shared .release-please-manifest.json (every component PR rewrites it):
#     reconcile with merge_release_please_manifests.py, keeping main's versions
#     for other components and the --component version(s) owned by this PR;
#   - the generated "synchronize" commit (docs and lockfiles): drop it, because
#     the caller regenerates the same files right after the replay.
# Anything else is a real conflict: abort and fail.
set -euo pipefail

REMOTE_BRANCH="${1:?remote branch ref, e.g. origin/release-please--...}"
shift
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="${RUNNER_TEMP:-$(mktemp -d)}"

in_rebase() { [[ -d "$(git rev-parse --git-path rebase-merge)" || -d "$(git rev-parse --git-path rebase-apply)" ]]; }

# A clean rebase can still carry a wrong shared manifest: Release Please
# rewrites it from a snapshot that may predate main, and that line applies
# without a conflict. Put main's versions back for every component this
# branch does not own.
reconcile_manifest() {
  [[ ${#COMPONENTS[@]} -gt 0 ]] || return 0
  git show origin/main:.release-please-manifest.json > "$TMP/main-release-manifest.json"
  python3 "$HERE/merge_release_please_manifests.py" \
    --main "$TMP/main-release-manifest.json" \
    --release .release-please-manifest.json \
    "${COMPONENTS[@]}" > "$TMP/reconciled-release-manifest.json"
  if ! cmp -s "$TMP/reconciled-release-manifest.json" .release-please-manifest.json; then
    cp "$TMP/reconciled-release-manifest.json" .release-please-manifest.json
    git add .release-please-manifest.json
    git commit -q -m "chore(release): restore main's versions for other components in the manifest"
  fi
}

COMPONENTS=("$@")

if git rebase origin/main; then
  reconcile_manifest
  exit 0
fi

for _ in $(seq 1 20); do
  in_rebase || { reconcile_manifest; exit 0; }
  CONFLICTS="$(git diff --name-only --diff-filter=U)"
  SUBJECT="$(git log -1 --format=%s REBASE_HEAD 2>/dev/null || true)"
  if [[ "$SUBJECT" =~ ^chore\(.*\):\ synchronize ]]; then
    echo "Dropping the generated commit '$SUBJECT': it is regenerated after the replay"
    git rebase --skip || true
  elif [[ "$CONFLICTS" == ".release-please-manifest.json" ]]; then
    git show origin/main:.release-please-manifest.json > "$TMP/main-release-manifest.json"
    git show "$REMOTE_BRANCH:.release-please-manifest.json" > "$TMP/branch-release-manifest.json"
    python3 "$HERE/merge_release_please_manifests.py" \
      --main "$TMP/main-release-manifest.json" \
      --release "$TMP/branch-release-manifest.json" \
      "${COMPONENTS[@]}" > .release-please-manifest.json
    git add .release-please-manifest.json
    GIT_EDITOR=true git rebase --continue || true
  else
    git rebase --abort
    echo "::error::Release branch has conflicts beyond the shared manifest and generated files: $(echo $CONFLICTS)"
    exit 1
  fi
done
in_rebase && { git rebase --abort; echo "::error::Release branch replay did not finish"; exit 1; }
exit 0
