#!/bin/zsh
# Amendment 11: install the pieces of arm cc-claude-cu-helper ("Claude Desktop 2.31226.0 computer-use helper via a
# minimal adapter"). VM ONLY: run it in the disposable bench VM clone made for this arm, never on a Mac anyone uses.
#
#   zsh install_cu_helper.sh        # download + verify Claude Desktop 2.31226.0, build winlist, print the pin values
#
# What it does, and what it never does:
#   1. downloads the pinned Claude Desktop archive from downloads.claude.ai and checks its byte size and sha256;
#   2. unpacks it with ditto into $CDB_BENCH_WORK/claude-desktop-2.31226.0/ (the app is NOT moved to /Applications
#      and is NEVER launched: Desktop updates itself when it starts);
#   3. checks the sha256 of Contents/Helpers/app-cu-helper and its signature (codesign --verify, Anthropic's team id);
#   4. builds winlist (public CGWindowList API) with swiftc and cu-disclaim (cu_disclaim.c) with clang.
#   It never re-signs anything, never removes the quarantine attribute (curl sets none), never changes entitlements.
set -euo pipefail
HERE=${0:A:h}
W=${CDB_BENCH_WORK:-$HOME/bench-work}
D=$W/claude-desktop-2.31226.0
URL=https://downloads.claude.ai/releases/darwin/universal/2.31226.0/Claude-eb794d1033f2a59c2cd89aa29ed1f8b5de12aa73.zip
ZIP_SHA=bd38f1051a14cabd893c7422e106ad6318e8785f19472c8064eafa64ce235820
ZIP_BYTES=383954641
HELPER_SHA=7bd631e8b5ae36941ac551c474055e2eff32163db6684628eac5cfcabcb3dc14
TEAM=Q6L2SF6YDW

mkdir -p $D
Z=$D/Claude-2.31226.0.zip
if [ ! -f $Z ] || [ "$(shasum -a 256 $Z | cut -d' ' -f1)" != $ZIP_SHA ]; then
  curl -fL --retry 3 -o $Z.part $URL
  mv $Z.part $Z
fi
[ "$(stat -f %z $Z)" = $ZIP_BYTES ] || { echo "archive size mismatch: $(stat -f %z $Z)" >&2; exit 2; }
[ "$(shasum -a 256 $Z | cut -d' ' -f1)" = $ZIP_SHA ] || { echo "archive sha256 mismatch" >&2; exit 2; }
echo "archive ok: $ZIP_BYTES bytes, sha256 $ZIP_SHA"

H=$D/extracted/Claude.app/Contents/Helpers/app-cu-helper
# unpack once: the helper's Accessibility grant (System Settings, A11) is for this file, so it is not re-extracted
if [ ! -f $H ] || [ "$(shasum -a 256 $H | cut -d' ' -f1)" != $HELPER_SHA ]; then
  rm -rf $D/extracted
  mkdir -p $D/extracted
  ditto -x -k $Z $D/extracted
fi
[ "$(shasum -a 256 $H | cut -d' ' -f1)" = $HELPER_SHA ] || { echo "helper sha256 mismatch" >&2; exit 2; }
codesign --verify --strict --verbose=2 $H
codesign -dv --verbose=2 $H 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier)="
[[ "$(codesign -dv $H 2>&1)" == *"TeamIdentifier=$TEAM"* ]] || { echo "helper team id is not $TEAM" >&2; exit 2; }
echo "helper ok: sha256 $HELPER_SHA"
xattr -l $H | sed 's/^/xattr: /' || true

mkdir -p $W/claude-cu-helper
swiftc -O -o $W/claude-cu-helper/winlist $HERE/winlist.swift
clang -O2 -Wall -o $W/claude-cu-helper/cu-disclaim $HERE/cu_disclaim.c
cp $HERE/cu_helper_mcp.py $W/claude-cu-helper/cu_helper_mcp.py

echo "--- values for pins.json claude_cu_helper"
cd ${HERE:h:h}
CDB_BENCH_WORK=$W /opt/homebrew/bin/python3 -c "import json, claude_arms as ca; print(json.dumps(ca.cu_helper_observed(), indent=2))"
