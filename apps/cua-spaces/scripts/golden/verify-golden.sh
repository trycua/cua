#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# verify-golden.sh — the publish gate for image CONTENT.
#
# Run INSIDE the guest as the last build step, after sanitize-golden.sh, so what
# it checks is exactly what ships. It reads golden-required.txt (installed next
# to it in ~/.cua) and fails if anything a Space needs is missing.
#
# Why this exists: the CI build script and the hand-run build script were two
# parallel definitions of "the golden", and CI's was materially thinner — no
# Unity, no Blender, no skills, no per-boot prep, no sanitizer. That publishes
# successfully and ships a broken product. Now there is one build script, and
# this is the backstop that makes a thin image fail loudly instead of quietly.
#
#   verify-golden.sh [path/to/golden-required.txt]
set -u

MANIFEST="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/golden-required.txt}"
[ -f "$MANIFEST" ] || { echo "!! missing manifest: $MANIFEST" >&2; exit 2; }

missing=0
checked=0

printf '\033[1;36m==> Verifying golden contents against %s\033[0m\n' "$MANIFEST"
while IFS= read -r line; do
  # Strip comments and surrounding whitespace; skip blanks.
  line="${line%%#*}"
  line="$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
  [ -n "$line" ] || continue
  # `~` only — never eval the line, so a manifest entry can never run anything.
  case "$line" in
    "~/"*) path="$HOME/${line#\~/}" ;;
    "~")   path="$HOME" ;;
    *)     path="$line" ;;
  esac
  path="${path%/}"
  checked=$((checked + 1))
  if [ -e "$path" ]; then
    printf '    ok      %s\n' "$line"
  else
    printf '\033[1;31m    MISSING %s\033[0m\n' "$line"
    missing=$((missing + 1))
  fi
done < "$MANIFEST"

# Beyond mere existence: the two things whose absence is silent and fatal.
# A driver bundle signed ad-hoc does NOT hold the seeded TCC grants, so the
# image would boot, look fine, and prompt on every clone.
for app in /Applications/CuaDriverLocal.app "/Applications/Cua Spacesd.app"; do
  if [ -d "$app" ]; then
    if codesign -dv --verbose=2 "$app" 2>&1 | grep -q "TeamIdentifier="; then
      printf '    ok      %s is certificate-backed\n' "$app"
    else
      printf '\033[1;31m    MISSING certificate-backed signature on %s (ad-hoc signatures do not hold seeded TCC grants)\033[0m\n' "$app"
      missing=$((missing + 1))
    fi
  fi
done

# cua-spacesd must be the bundle's own executable, or launchd crash-loops the
# agent on every boot and nothing ever binds :3211.
if [ -x "/Applications/Cua Spacesd.app/Contents/MacOS/cua-spacesd" ]; then
  printf '    ok      cua-spacesd executable present in its bundle\n'
else
  printf '\033[1;31m    MISSING cua-spacesd executable in /Applications/Cua Spacesd.app\033[0m\n'
  missing=$((missing + 1))
fi

# The spacesd must actually answer: /health is unauthenticated (it answers
# in bootstrap mode too) and returns 204. A driver that crash-loops under
# launchd looks installed and serves nothing.
health=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  health="$(curl -s -o /dev/null -m 3 -w '%{http_code}' http://127.0.0.1:3211/health 2>/dev/null)"
  [ "$health" = 204 ] && break
  sleep 3
done
if [ "$health" = 204 ]; then
  printf '    ok      cua-spacesd answers on :3211/health\n'
else
  printf '\033[1;31m    MISSING cua-spacesd on :3211/health (got %s)\033[0m\n' "${health:-nothing}"
  missing=$((missing + 1))
fi

# The daemons it replaced must be gone, or they compete for the capture grants
# and teleports land in the wrong place.
for stale in "/Applications/RCDP Host.app" "$HOME/.local/bin/rcdp-handoff" \
             "$HOME/Library/LaunchAgents/com.trycua.rcdphost.plist" \
             "$HOME/Library/LaunchAgents/com.trycua.rcdp-handoff.plist" \
             "$HOME/Library/LaunchAgents/com.trycua.computer_server.plist" \
             "$HOME/.cua/spacesd/token"; do
  if [ -e "$stale" ]; then
    printf '\033[1;31m    STALE   %s must not ship\033[0m\n' "$stale"
    missing=$((missing + 1))
  fi
done

echo
if [ "$missing" -gt 0 ]; then
  printf '\033[1;31m!! golden is INCOMPLETE: %d of %d required items missing. Do not publish.\033[0m\n' \
    "$missing" "$checked" >&2
  exit 1
fi
printf '\033[1;32m==> golden complete: %d required items present\033[0m\n' "$checked"
