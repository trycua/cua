#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Checks that the xtradeb/apps PPA still has the Chromium major
# libs/images/linux/Dockerfile pins for each arch (the major of
# CHROMIUM_VERSION_AMD64 / CHROMIUM_VERSION_ARM64, or CHROMIUM_MAJOR_<ARCH>). The PPA keeps only its newest build per arch, so a
# major disappears without notice; the image build then falls back to the
# newest build and warns, and this check fails loudly with what the PPA has, so
# the pin is bumped on purpose.
#
#   scripts/images/check-chromium-pin.sh [Dockerfile]
set -euo pipefail
dockerfile="${1:-libs/images/linux/Dockerfile}"
ppa="https://ppa.launchpadcontent.net/xtradeb/apps/ubuntu/dists/noble/main"
status=0
for arch in amd64 arm64; do
  ARCH="$(echo "$arch" | tr a-z A-Z)"
  var="CHROMIUM_MAJOR_${ARCH}"
  major="$(sed -n -E "s/^ARG ${var}=([0-9]+)\$/\\1/p" "$dockerfile")"
  if [ -z "$major" ]; then
    var="CHROMIUM_VERSION_${ARCH}"
    major="$(sed -n -E "s/^ARG ${var}=([0-9]+)\..*\$/\\1/p" "$dockerfile")"
  fi
  if [ -z "$major" ]; then
    echo "::error file=${dockerfile}::no ARG ${var}=<major> in ${dockerfile}"
    status=1
    continue
  fi
  packages="$(curl -fsSL --retry 3 "${ppa}/binary-${arch}/Packages.gz" | gunzip -c)"
  for pkg in chromium chromium-common; do
    have="$(printf '%s\n' "$packages" | awk -v p="$pkg" '/^Package: /{ok=($2==p)} ok&&/^Version: /{print $2}' | sort -uV)"
    if printf '%s\n' "$have" | grep -q "^${major}\."; then
      echo "ok: ${pkg} ${major}.* for ${arch}: $(printf '%s\n' "$have" | grep "^${major}\." | tail -n 1)"
    else
      echo "::error file=${dockerfile}::the xtradeb PPA has no ${pkg} ${major}.* for ${arch} (${var}=${major}); it has: $(echo ${have:-nothing}). Bump ${var} in ${dockerfile}."
      status=1
    fi
  done
done
exit "$status"
