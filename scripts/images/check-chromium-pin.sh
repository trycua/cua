#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Checks the Chromium pin of the Linux image (libs/images/linux/Dockerfile),
# which installs Playwright's pinned Chromium (stage playwright-chromium), the
# same build on amd64 and arm64:
#   1. the Dockerfile has PLAYWRIGHT_VERSION and MIN_CHROMIUM_MAJOR, and the
#      minimum is not below FLOOR (the first Chrome major without the open
#      critical CVEs: 158.0.8127.36 fixed CVE-2026-130959);
#   2. the Chromium that Playwright version ships (browsers.json of the
#      playwright-core package) is at least that minimum, so the pinned
#      version cannot quietly fall behind;
#   3. no image Dockerfile installs Chromium from the xtradeb PPA any more;
#   4. libs/cuabot/Dockerfile pins the same Playwright version.
# The image build itself also fails if the browser it installed is below
# MIN_CHROMIUM_MAJOR. Bump PLAYWRIGHT_VERSION (and FLOOR here, when a new
# Chrome CVE sets a higher one) on purpose.
#
#   scripts/images/check-chromium-pin.sh [Dockerfile]
set -euo pipefail
FLOOR=158
dockerfile="${1:-libs/images/linux/Dockerfile}"
status=0
fail() { echo "::error file=${dockerfile}::$*"; status=1; }

arg() { sed -n -E "s/^ARG $2=([0-9][0-9.]*)\$/\\1/p" "$1" | head -n 1; }

pw="$(arg "$dockerfile" PLAYWRIGHT_VERSION)"
min="$(arg "$dockerfile" MIN_CHROMIUM_MAJOR)"
[ -n "$pw" ] || fail "no ARG PLAYWRIGHT_VERSION=<x.y.z> in ${dockerfile}"
[ -n "$min" ] || fail "no ARG MIN_CHROMIUM_MAJOR=<major> in ${dockerfile}"
if [ -n "$min" ] && [ "$min" -lt "$FLOOR" ]; then
  fail "MIN_CHROMIUM_MAJOR=${min} is below ${FLOOR}, the first Chromium major with the CVE-2026-130959 fix"
fi

if [ -n "$pw" ] && [ -n "$min" ]; then
  browsers="$(curl -fsSL --retry 3 "https://cdn.jsdelivr.net/npm/playwright-core@${pw}/browsers.json")"
  have="$(printf '%s' "$browsers" | python3 -c 'import json,sys; print(next(b["browserVersion"] for b in json.load(sys.stdin)["browsers"] if b["name"]=="chromium"))')"
  if [ "${have%%.*}" -ge "$min" ]; then
    echo "ok: Playwright ${pw} ships Chromium ${have} (minimum ${min})"
  else
    fail "Playwright ${pw} ships Chromium ${have}, below MIN_CHROMIUM_MAJOR=${min}. Bump PLAYWRIGHT_VERSION in ${dockerfile}."
  fi
fi

for f in libs/images/linux/Dockerfile libs/images/bench/web/Dockerfile; do
  if [ -f "$f" ] && grep -qi 'xtradeb' "$f"; then
    fail "${f} still references the xtradeb PPA; Chromium comes from Playwright's pinned build"
  fi
done

cuabot="$(dirname "$dockerfile")/../../cuabot/Dockerfile"
if [ -n "$pw" ] && [ -f "$cuabot" ]; then
  cpw="$(arg "$cuabot" PLAYWRIGHT_VERSION)"
  if [ "$cpw" = "$pw" ]; then echo "ok: libs/cuabot pins the same Playwright ${pw}"
  else fail "libs/cuabot/Dockerfile pins Playwright ${cpw:-nothing}, the Linux image ${pw}; keep them equal"; fi
fi
exit "$status"
