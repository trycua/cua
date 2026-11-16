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

if git rebase origin/main; then
  exit 0
fi

for _ in $(seq 1 20); do
  in_rebase || exit 0
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
      ${@+"$@"} > .release-please-manifest.json
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
