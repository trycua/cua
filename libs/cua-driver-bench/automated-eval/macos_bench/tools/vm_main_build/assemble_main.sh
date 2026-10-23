#!/bin/zsh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Assemble the private main-build app (ad-hoc signed, own bundle id) and its skill, inside the VM
set -e
M=~/bench-work/cua-main
R=$M/src/libs/cua-driver/rust
A=$M/CuaDriverBenchMain.app
OLD=~/bench-work/cua-0.34.0/CuaDriver-0.34.0.app
SHA=$(cat $M/SOURCE_SHA)
VERSION=$(awk -F '"' '/^\[workspace\.package\][[:space:]]*$/ { package=1; next } /^\[/ { package=0 } package && /^[[:space:]]*version[[:space:]]*=/ { print $2; exit }' "$R/Cargo.toml")
[[ -n $VERSION ]] || { echo "no [workspace.package] version in $R/Cargo.toml" >&2; exit 1; }
rm -rf $A; mkdir -p $A/Contents/MacOS $A/Contents/Resources
cp $R/target/release/cua-driver $A/Contents/MacOS/cua-driver
cp $R/target/release/cua-cursor-theme $A/Contents/MacOS/cua-cursor-theme
cp $OLD/Contents/Resources/AppIcon.icns $A/Contents/Resources/
cp $OLD/Contents/Info.plist $A/Contents/Info.plist
plutil -replace CFBundleIdentifier -string com.trycua.driver.benchmain $A/Contents/Info.plist
plutil -replace CFBundleName -string "Cua Driver Bench Main" $A/Contents/Info.plist
plutil -replace CFBundleDisplayName -string "Cua Driver Bench Main" $A/Contents/Info.plist
plutil -replace CFBundleShortVersionString -string "$VERSION" $A/Contents/Info.plist
plutil -replace CFBundleVersion -string "${VERSION}.${SHA:0:9}" $A/Contents/Info.plist
cat > $M/ent.plist <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.automation.apple-events</key><true/>
<key>com.apple.security.device.screen-capture</key><true/>
</dict></plist>
PL
codesign --force --sign - --identifier com.trycua.driver.benchmain --options runtime --entitlements $M/ent.plist $A/Contents/MacOS/cua-cursor-theme
codesign --force --sign - --identifier com.trycua.driver.benchmain --options runtime --entitlements $M/ent.plist $A
codesign --verify --strict $A && echo signed-ok
rm -rf $M/skills; mkdir -p $M/skills; cp -R $R/Skills/cua-driver $M/skills/cua-driver
ls $M/skills/cua-driver
$A/Contents/MacOS/cua-driver --version
shasum -a 256 $A/Contents/MacOS/cua-driver
codesign -d -r- $A 2>&1 | tail -1

