#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# check-golden-coverage.sh — static, VM-free guard against the CI golden and the
# hand-run golden drifting apart again.
#
# History: .github/workflows/cd-macos-golden.yml ran build-macos-golden.sh, a
# SECOND definition of "the golden" that installed the driver, the old capture
# and command daemons, and Chrome and stopped there — no Unity, no Blender, no
# skills, no ~/.cua payload, no sanitize-golden.sh. Publishing its output to the
# tag the app pulls would have looked like a successful release and shipped a
# broken product. build-macos-golden.sh now DELEGATES the whole provisioning
# phase to build-golden.sh and keeps only CI concerns (VM lifecycle, push
# preflight, lume push).
#
# This script asserts that arrangement still holds, in milliseconds, so a future
# step added to one script and not the other fails a cheap gate rather than a
# multi-hour build — or worse, a publish.
#
#   ./check-golden-coverage.sh
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_GOLDEN="$HERE/build-golden.sh"
CI_BUILD="$HERE/../build-macos-golden.sh"
MANIFEST="$HERE/golden-required.txt"

fails=0
ok()   { printf '    ok   %s\n' "$*"; }
bad()  { printf '\033[1;31m    FAIL %s\033[0m\n' "$*" >&2; fails=$((fails + 1)); }

printf '\033[1;36m==> Golden coverage check\033[0m\n'

for f in "$BUILD_GOLDEN" "$CI_BUILD" "$MANIFEST" "$HERE/verify-golden.sh" "$HERE/sanitize-golden.sh"; do
  [ -f "$f" ] || bad "missing $f"
done
[ "$fails" -eq 0 ] || { printf '\033[1;31m!! %d problem(s)\033[0m\n' "$fails" >&2; exit 1; }

# 1. Every helper build-golden.sh runs in the guest must actually exist here.
#    A step that names a script nobody committed dies an hour into the build.
while IFS= read -r s; do
  [ -f "$HERE/$s" ] && ok "step script present: $s" || bad "build-golden.sh runs $s, which does not exist in $HERE"
done < <(grep -oE '\$REMOTE_SRC/[A-Za-z0-9._-]+\.(sh|py)' "$BUILD_GOLDEN" | sed 's|.*/||' | sort -u)

# 2. Every file build-golden.sh installs into ~/.cua/ must be declared required.
#    This is the rule that catches "a new payload file was added to one place".
while IFS= read -r s; do
  grep -qE "^[[:space:]]*~/\.cua/$(printf '%s' "$s" | sed 's/[.[\*^$]/\\&/g')[[:space:]]*$" "$MANIFEST" \
    && ok "~/.cua payload declared: $s" \
    || bad "build-golden.sh installs $s into ~/.cua but golden-required.txt does not require it"
done < <(grep -E 'install -m [0-9]+ .*~/\.cua/' "$BUILD_GOLDEN" \
           | grep -oE '\$REMOTE_SRC/[A-Za-z0-9._-]+\.(sh|py)' | sed 's|.*/||' | sort -u)

# 3. build-golden.sh must still end with the two gates, in this order:
#    sanitize (strips the builder's identity) then verify (content contract).
order="$(grep -noE 'sanitize-golden\.sh|verify-golden\.sh' "$BUILD_GOLDEN" | sed 's/.*://' | uniq | tail -2 | tr '\n' ' ')"
case "$order" in
  "sanitize-golden.sh verify-golden.sh ") ok "sanitize runs before verify, both last" ;;
  *) bad "build-golden.sh must finish with sanitize-golden.sh then verify-golden.sh (saw: ${order:-none})" ;;
esac

# 4. The CI script must DELEGATE, not re-implement. If it ever grows its own
#    provisioning again there are two definitions of the golden and this whole
#    class of bug is back.
grep -q 'golden/build-golden\.sh' "$CI_BUILD" \
  && ok "build-macos-golden.sh delegates provisioning to build-golden.sh" \
  || bad "build-macos-golden.sh no longer invokes golden/build-golden.sh"

# Markers of a re-inlined provisioning phase. Deliberately narrow: these are the
# exact things the CI script used to duplicate.
while IFS='|' read -r pat what; do
  if grep -qE "$pat" "$CI_BUILD"; then
    bad "build-macos-golden.sh appears to re-implement $what (matched /$pat/); that belongs in build-golden.sh"
  else
    ok "no duplicate $what in the CI script"
  fi
done <<'PATTERNS'
LaunchAgents|the LaunchAgent definitions
seed-tcc-guest\.sh|TCC seeding
uv pip install|the helper-venv install
install-chrome\.sh|the Chrome install
PATTERNS

# 5. The CI script must not regress the push shape: --single-layer needs ~30 GB
#    of scratch on a runner that has just built a VM (see 302c06604).
if grep -qE '^[[:space:]]*_push_args\+=\(--single-layer\)' "$CI_BUILD" && grep -q 'PUSH_SINGLE_LAYER' "$CI_BUILD"; then
  ok "lume push defaults to chunked (single-layer only behind PUSH_SINGLE_LAYER=1)"
else
  bad "lume push must default to --chunk-size-mb, with --single-layer gated behind PUSH_SINGLE_LAYER"
fi
grep -q 'GITHUB_USERNAME' "$CI_BUILD" && grep -q 'ghcr.io/token' "$CI_BUILD" \
  && ok "push-credential preflight intact (lume push ignores docker's credential store)" \
  || bad "the ghcr push-credential preflight is gone"

# 6. cua-spacesd is THE in-guest daemon. The golden must require its signed
#    bundle, and nothing may install the daemons it replaced (rcdpd,
#    rcdp-handoff, computer-server) — two capture daemons compete for the same
#    TCC grants, and a second teleport receiver is a second place for secrets.
grep -qE '^/Applications/Cua Spacesd\.app[[:space:]]*$' "$MANIFEST" \
  && ok "golden-required.txt requires Cua Spacesd.app" \
  || bad "golden-required.txt does not require /Applications/Cua Spacesd.app"
if grep -vE '^[[:space:]]*#' "$MANIFEST" | grep -qE 'rcdp|RCDP|computer_server|computer-server'; then
  bad "golden-required.txt still requires a removed daemon (rcdp*/computer-server)"
else
  ok "no removed daemons required"
fi
for f in "$BUILD_GOLDEN" "$CI_BUILD"; do
  if grep -vE '^[[:space:]]*#' "$f" | grep -qE 'rcdp-handoff|RCDP Host\.app|RCDP_TARGET|HANDOFF_BIN'; then
    bad "$(basename "$f") still stages a removed daemon"
  else
    ok "$(basename "$f") stages no removed daemons"
  fi
done

# 7. Everything must parse. bash -n is what would have caught the apostrophe in
#    "Google's signature" that left this file unrunnable for its entire life.
for f in "$HERE"/*.sh "$CI_BUILD"; do
  bash -n "$f" 2>/dev/null && ok "parses: $(basename "$f")" || bad "SYNTAX ERROR: $f"
done

echo
if [ "$fails" -gt 0 ]; then
  printf '\033[1;31m!! golden coverage check failed (%d problem(s))\033[0m\n' "$fails" >&2
  exit 1
fi
printf '\033[1;32m==> golden coverage check passed\033[0m\n'
