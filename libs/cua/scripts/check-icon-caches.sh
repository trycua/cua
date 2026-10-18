#!/usr/bin/env bash
# Fails when a UI keeps its own app icon cache. Every app icon goes through
# the SDK's one cache (libs/cua/crates/cua-icon-cache: `Space.appIcons`,
# `Teleport.appIconPng`); apps and samples keep only the icons of what they
# are showing now.
#
#   libs/cua/scripts/check-icon-caches.sh
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"
pattern='\b(IconCache|icon_cache|iconCache|appIconCache|AppIconStore|IconStore)\b'
hits=$(grep -rEn "$pattern" apps samples libs/spaces-sdk-swift/Sources \
  --include='*.swift' --include='*.ts' --include='*.tsx' --include='*.rs' --include='*.mjs' \
  --exclude-dir=node_modules --exclude-dir=.build --exclude-dir=target --exclude-dir=dist \
  | grep -vE 'cua_icon_cache::' || true)
if [ -n "$hits" ]; then
  echo "A UI keeps its own icon cache; use the SDK's (Space.appIcons, Teleport.appIconPng):" >&2
  echo "$hits" >&2
  exit 1
fi
echo "no UI keeps its own icon cache"
