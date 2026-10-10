#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Would the installed Swift app's Sparkle take this update? Builds a small
# checker (validate.m) from Sparkle's own sources, at the version
# apps/cua-spaces-macos pins (Package.resolved), and runs it with the old
# app, the update's disk image and the image's EdDSA signature (the
# appcast item's sparkle:edSignature). Nothing is installed.
#
#   scripts/sparkle/accepts.sh "/Applications/Cua Spaces.app" Cua-Spaces-0.9.0-universal.dmg <edSignature>
#
# SPARKLE_SRC=<checkout> uses a local Sparkle source tree instead of a
# shallow clone of the pinned tag.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
old="${1:?usage: accepts.sh OLD.app UPDATE.dmg ED_SIGNATURE}"
dmg="${2:?usage: accepts.sh OLD.app UPDATE.dmg ED_SIGNATURE}"
signature="${3:?usage: accepts.sh OLD.app UPDATE.dmg ED_SIGNATURE}"
work="$(mktemp -d)"
mnt="$work/mnt"
cleanup() {
  hdiutil detach -quiet "$mnt" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

src="${SPARKLE_SRC:-}"
if [ -z "$src" ]; then
  resolved="$here/../../../cua-spaces-macos/Package.resolved"
  version="$(python3 -c 'import json,sys; print(next(p["state"]["version"] for p in json.load(open(sys.argv[1]))["pins"] if p["identity"] == "sparkle"))' "$resolved")"
  git clone -q --depth 1 --branch "$version" https://github.com/sparkle-project/Sparkle.git "$work/Sparkle" 2>/dev/null
  src="$work/Sparkle"
fi

# The files SUUpdateValidator, SUInstaller and the version check need, with
# the build settings Sparkle's ConfigCommon.xcconfig gives them.
mkdir -p "$work/include"
ln -s "$(cd "$src/Sparkle" && pwd)" "$work/include/Sparkle"
defs=(
  "-DSPU_OBJC_DIRECT=__attribute__((objc_direct))"
  "-DSPU_OBJC_DIRECT_MEMBERS=__attribute__((objc_direct_members))"
  -DSPARKLE_NORMALIZE_INSTALLED_APPLICATION_NAME=0 -DSPARKLE_BUILD_UI_BITS=0 -DSPARKLE_COPY_LOCALIZATIONS=0
  -DSPARKLE_BUILD_LEGACY_SUUPDATER=0 -DSPARKLE_BUILD_PACKAGE_SUPPORT=1 -DSPARKLE_BUILD_LEGACY_DELTA_SUPPORT=0
  -DSPARKLE_BUILD_BZIP2_DELTA_SUPPORT=0 -DSPARKLE_BUILD_LEGACY_DSA_SUPPORT=0
  '-DSPARKLE_BUNDLE_IDENTIFIER="org.sparkle-project.Sparkle"' '-DSPARKLE_RELAUNCH_TOOL_NAME="Autoupdate"'
  '-DSPARKLE_INSTALLER_PROGRESS_TOOL_NAME="Updater"' '-DMARKETING_VERSION="2"' '-DCURRENT_PROJECT_VERSION="2"'
)
srcs=("$here/validate.m")
for f in Sparkle/SUUpdateValidator Sparkle/SUHost Sparkle/SUSignatures Sparkle/SPUVerifierInformation Sparkle/SULog \
  Sparkle/SUConstants Sparkle/SUStandardVersionComparator Sparkle/SUFileManager Sparkle/SUOperatingSystem \
  Sparkle/SUNormalization Autoupdate/SUSignatureVerifier Autoupdate/SUCodeSigningVerifier Autoupdate/SUInstaller \
  Autoupdate/SUPlainInstaller Autoupdate/SUGuidedPackageInstaller; do
  srcs+=("$src/$f.m")
done
srcs+=("$src"/Vendor/ed25519-sparkle/src/*.c)
clang -fobjc-arc -w -I"$work/include" "${defs[@]}" -I"$src/Sparkle" -I"$src/Autoupdate" \
  -I"$src/Vendor/ed25519-sparkle/src" "${srcs[@]}" -framework Foundation -framework Security -o "$work/validate"

mkdir -p "$mnt"
hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$mnt" "$dmg"
"$work/validate" "$old" "$dmg" "$signature" "$mnt"
