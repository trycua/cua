#!/bin/zsh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Assemble the private script-arm app (CuaDriver.app (own directory), bundle id com.trycua.driver.benchscript) from ANY
# Cua Driver build, so one run can compare two builds (for example the 0.41.3 release and a main build) under the
# same app identity, daemon socket and TCC grant. Inside the benchmark VM, as user lume.
#
#   assemble_version.sh RELEASE_APP DRIVER_BIN CURSOR_THEME_BIN SKILLS_DIR LABEL
#
#   RELEASE_APP        any released CuaDriver.app (supplies Info.plist and the icon)
#   DRIVER_BIN         the cua-driver executable to test (the release app's own, or a cargo-built one)
#   CURSOR_THEME_BIN   the matching cua-cursor-theme executable
#   SKILLS_DIR         the matching skill directory (.../Skills/cua-driver or the unpacked skills tarball)
#   LABEL              short name for the output (for example v0413 or head)
#
# Env: SIGN_IDENTITY (default "-", ad hoc). Use one persistent identity for every build so the Accessibility and
# Screen Recording grant of the bundle id survives a swap; an ad-hoc signature changes with every binary and needs a
# new grant each time. GIT_SHA (source commit the binary was built from, for pins).
# Output: ~/bench-work/cua-script/CuaDriver.app, ~/bench-work/cua-script/skills/cua-driver,
# ~/bench-work/pins-LABEL.json (pins.json with cua_main set to this build; pass it with --pins).
set -e
REL=${1:?release app}; BIN=${2:?driver bin}; CUR=${3:?cursor theme bin}; SK=${4:?skills dir}; LABEL=${5:?label}
SIGN=${SIGN_IDENTITY:--}
ID=com.trycua.driver.benchscript
S=~/bench-work/cua-script
A=$S/CuaDriver.app
HERE=${0:A:h}
mkdir -p $S; rm -rf $A; mkdir -p $A/Contents/MacOS $A/Contents/Resources
cp $BIN $A/Contents/MacOS/cua-driver
cp $CUR $A/Contents/MacOS/cua-cursor-theme
cp $REL/Contents/Resources/AppIcon.icns $A/Contents/Resources/ 2>/dev/null || true
cp $REL/Contents/Info.plist $A/Contents/Info.plist
VERSION=$($A/Contents/MacOS/cua-driver --version | awk '{print $2}')
plutil -replace CFBundleIdentifier -string $ID $A/Contents/Info.plist
plutil -replace CFBundleName -string "Cua Driver Bench Script" $A/Contents/Info.plist
plutil -replace CFBundleDisplayName -string "Cua Driver Bench Script" $A/Contents/Info.plist
plutil -replace CFBundleShortVersionString -string "$VERSION" $A/Contents/Info.plist
plutil -replace CFBundleVersion -string "${VERSION}.${GIT_SHA:0:9}" $A/Contents/Info.plist
cat > $S/ent.plist <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.automation.apple-events</key><true/>
<key>com.apple.security.device.screen-capture</key><true/>
</dict></plist>
PL
codesign --force --sign "$SIGN" --identifier $ID --options runtime --entitlements $S/ent.plist $A/Contents/MacOS/cua-cursor-theme
codesign --force --sign "$SIGN" --identifier $ID --options runtime --entitlements $S/ent.plist $A
codesign --verify --strict $A && echo signed-ok
rm -rf $S/skills; mkdir -p $S/skills; cp -R $SK $S/skills/cua-driver
# The VM's own task pack may differ from the pinned public copy; ~/bench-work/cdb-observed-digests.json ({pack_task: digest})
# records the digests of the pack both builds are run against.
[ -f ~/bench-work/cdb-observed-digests.json ] && export CDB_OBSERVED_DIGESTS=~/bench-work/cdb-observed-digests.json
/usr/bin/python3 "$HERE/make_driver_pins.py" --app $A --skills $S/skills/cua-driver --git-sha "${GIT_SHA:?GIT_SHA}" \
  --out ~/bench-work/pins-$LABEL.json
codesign -d -r- $A 2>&1 | tail -1
