#!/usr/bin/env bash
# Contract gate for libs/cua/proto: lint, formatting and breaking-change
# detection against the base branch.
#
# Usage: libs/cua/scripts/check-proto.sh [--fix]
#   --fix                 rewrite files with `buf format -w` instead of diffing
# Environment:
#   CUA_PROTO_BASE_REF    git ref to compare against (default: main, then
#                         origin/main)
#   CUA_PROTO_SKIP_BREAKING=1  skip the breaking check (local iteration only)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
proto_dir="$(cd "$here/../proto" && pwd)"
repo_root="$(git -C "$proto_dir" rev-parse --show-toplevel)"
proto_subdir="${proto_dir#"$repo_root"/}"

command -v buf >/dev/null || { echo "error: buf not found on PATH (https://buf.build/docs/installation)" >&2; exit 127; }

cd "$proto_dir"

echo "==> buf lint"
buf lint

if [[ "${1:-}" == "--fix" ]]; then
  echo "==> buf format -w"
  buf format -w
else
  echo "==> buf format --diff --exit-code"
  buf format --diff --exit-code
fi

echo "==> buf build"
buf build -o /dev/null

if [[ "${CUA_PROTO_SKIP_BREAKING:-0}" == "1" ]]; then
  echo "==> buf breaking: skipped (CUA_PROTO_SKIP_BREAKING=1)"
  exit 0
fi

base_ref=""
for candidate in "${CUA_PROTO_BASE_REF:-}" main origin/main; do
  [[ -n "$candidate" ]] || continue
  if git -C "$repo_root" rev-parse --verify --quiet "$candidate^{commit}" >/dev/null; then
    base_ref="$candidate"
    break
  fi
done
if [[ -z "$base_ref" ]]; then
  echo "error: no base ref found (tried \$CUA_PROTO_BASE_REF, main, origin/main); fetch it or set CUA_PROTO_SKIP_BREAKING=1" >&2
  exit 1
fi

# Compare against the merge base so unrelated changes on the base branch
# never show up as breaking changes of this branch.
base_commit="$(git -C "$repo_root" merge-base HEAD "$base_ref" 2>/dev/null || git -C "$repo_root" rev-parse "$base_ref")"

if ! git -C "$repo_root" cat-file -e "$base_commit:$proto_subdir/buf.yaml" 2>/dev/null; then
  echo "==> buf breaking: $proto_subdir does not exist at $base_ref ($base_commit); first introduction, nothing to compare"
  exit 0
fi

# `git archive` instead of buf's `.git#ref=` input: it works in linked
# worktrees (where .git is a file) and shallow CI clones alike.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
git -C "$repo_root" archive "$base_commit" "$proto_subdir" | tar -x -C "$tmp"

echo "==> buf breaking --against $base_ref ($base_commit)"
buf breaking --against "$tmp/$proto_subdir"
