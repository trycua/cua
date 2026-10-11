#!/bin/zsh
# Amendment 6: the script arm's private app, a copy of the main-build app with its own bundle id
# (com.trycua.driver.benchscript), so its TCC rows are its own. Same binary; the run_script flag is set by the
# runner in the daemon's environment, not here. Run after assemble_main.sh, then tcc_grant.sh with this app and id.
set -e
M=~/bench-work/cua-main/CuaDriverBenchMain.app
S=~/bench-work/cua-script
A=$S/CuaDriverBenchScript.app
ENT=~/bench-work/cua-main/ent.plist
mkdir -p $S
rm -rf $A
cp -R $M $A
plutil -replace CFBundleIdentifier -string com.trycua.driver.benchscript $A/Contents/Info.plist
plutil -replace CFBundleName -string "Cua Driver Bench Script" $A/Contents/Info.plist
plutil -replace CFBundleDisplayName -string "Cua Driver Bench Script" $A/Contents/Info.plist
codesign --force --sign - --identifier com.trycua.driver.benchscript --options runtime --entitlements $ENT $A/Contents/MacOS/cua-cursor-theme
codesign --force --sign - --identifier com.trycua.driver.benchscript --options runtime --entitlements $ENT $A
codesign --verify --strict $A && echo signed-ok
shasum -a 256 $A/Contents/MacOS/cua-driver $M/Contents/MacOS/cua-driver
