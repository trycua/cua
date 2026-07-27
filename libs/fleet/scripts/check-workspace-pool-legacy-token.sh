#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
allowlist="$repo_root/libs/fleet/scripts/workspace-pool-legacy-token-allowlist.txt"
legacy_token='OSGymWorkspacePool'

mapfile -t actual < <(rg -l -I -F "$legacy_token" "$repo_root" | sed "s#^$repo_root/##" | LC_ALL=C sort)
mapfile -t allowed < <(awk -F '\t' 'NF { print $1 }' "$allowlist" | LC_ALL=C sort)
unexpected="$(comm -23 <(printf '%s\n' "${actual[@]}") <(printf '%s\n' "${allowed[@]}"))"
missing="$(comm -13 <(printf '%s\n' "${actual[@]}") <(printf '%s\n' "${allowed[@]}"))"

if [ -n "$unexpected" ] || [ -n "$missing" ]; then
  echo "workspace-pool legacy-token allowlist drift" >&2
  [ -z "$unexpected" ] || printf 'unexpected:\n%s\n' "$unexpected" >&2
  [ -z "$missing" ] || printf 'missing:\n%s\n' "$missing" >&2
  exit 1
fi

count="$(rg -n -I -F "$legacy_token" "$repo_root" | wc -l)"
if [ "$count" -gt 46 ]; then
  echo "workspace-pool legacy-token budget exceeded: $count > 46" >&2
  exit 1
fi
