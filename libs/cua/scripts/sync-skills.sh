#!/usr/bin/env bash
# Vendors the upstream default skills into libs/cua/skills/ (bundled into
# cua-agent-setup with include_dir). cua-sandboxes and cua-spaces live only
# here; cua-driver and gui-automation are copies of their canonical sources.
#
# Usage: scripts/sync-skills.sh [--check]
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
check=0
[[ "${1:-}" == "--check" ]] && check=1

pairs=(
  "$repo/libs/cua-driver/rust/Skills/cua-driver:$here/skills/cua-driver"
  "$repo/skills/gui-automation:$here/skills/gui-automation"
)
status=0
for pair in "${pairs[@]}"; do
  src="${pair%%:*}"
  dst="${pair##*:}"
  if [[ ! -f "$src/SKILL.md" ]]; then
    echo "missing source skill: $src" >&2
    exit 1
  fi
  if (( check )); then
    if ! diff -r "$src" "$dst" >/dev/null 2>&1; then
      echo "drift: $dst differs from $src (run scripts/sync-skills.sh)" >&2
      status=1
    fi
  else
    rm -rf "$dst"
    mkdir -p "$(dirname "$dst")"
    cp -R "$src" "$dst"
    echo "synced $(basename "$dst")"
  fi
done
exit $status
