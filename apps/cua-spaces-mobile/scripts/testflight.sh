#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# One command: build the core for device, prebuild, archive, sign, upload to
# TestFlight.
#   scripts/testflight.sh             sign and upload
#   scripts/testflight.sh --dry-run   unsigned device archive only (proves it builds)
# Auth (pick one):
#   (a) Xcode > Settings > Accounts is signed in to the team in app.json (ios.appleTeamId)
#   (b) ASC_KEY_PATH (.p8), ASC_KEY_ID, ASC_ISSUER_ID in the environment
# Credentials come from the environment or the Xcode account only; no key is read from the repo.
# Optional: CUA_APPLE_TEAM_ID (default: app.json), CUA_BUILD_NUMBER (default yymmdd.HHMM), BUILD_ROOT (default ./.testflight-build).
set -euo pipefail
cd "$(dirname "$0")/.."
APP=$PWD
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1
TEAM="${CUA_APPLE_TEAM_ID:-$(node -p "require('./app.json').expo.ios.appleTeamId")}"
BUILD_ROOT="${BUILD_ROOT:-$APP/.testflight-build}"
export CUA_IOS_RELEASE=1
export CUA_BUILD_NUMBER="${CUA_BUILD_NUMBER:-$(date +%y%m%d.%H%M)}"
export EXPO_NO_TELEMETRY=1 CI=1
# Production: client id cua-spaces-mobile, redirect ai.cua.spaces:/oauth/callback,
# relay https://relay.cua.ai are the defaults in src/auth/config.ts. A stray
# sample or staging flag must never reach a release.
unset EXPO_PUBLIC_CUA_SAMPLE EXPO_PUBLIC_CUA_SAMPLE_PERSIST EXPO_PUBLIC_CUA_ENV \
      EXPO_PUBLIC_CUA_RELAY_URL EXPO_PUBLIC_CUA_OIDC_ISSUER EXPO_PUBLIC_CUA_DEV_ACCOUNT EXPO_PUBLIC_CUA_DEV_TOKEN_URL
export EXPO_PUBLIC_CUA_OIDC_CLIENT_ID=cua-spaces-mobile

if [ "$DRY" = 0 ]; then
  if [ -n "${ASC_KEY_PATH:-}${ASC_KEY_ID:-}${ASC_ISSUER_ID:-}" ]; then
    [ -f "${ASC_KEY_PATH:-}" ] && [ -n "${ASC_KEY_ID:-}" ] && [ -n "${ASC_ISSUER_ID:-}" ] \
      || { echo "ASC_KEY_PATH, ASC_KEY_ID and ASC_ISSUER_ID must all be set (path must exist)" >&2; exit 2; }
    AUTH=(-authenticationKeyPath "$ASC_KEY_PATH" -authenticationKeyID "$ASC_KEY_ID" -authenticationKeyIssuerID "$ASC_ISSUER_ID")
    echo "auth: App Store Connect API key"
  else
    AUTH=()
    echo "auth: the Xcode account (Settings > Accounts must show team $TEAM)"
  fi
fi

echo "build number $CUA_BUILD_NUMBER, version $(node -p "require('./app.json').expo.version")"
[ -d node_modules ] || pnpm install --frozen-lockfile
# The gRPC-Web types are a committed copy of the viewer's: sync them (pnpm proto:check is the drift gate).
pnpm proto
[ -d modules/cua-core/ios/CuaCoreLib.xcframework ] || pnpm core:ios
npx expo prebuild --platform ios --clean

mkdir -p "$BUILD_ROOT"
ARCHIVE="$BUILD_ROOT/CuaSpaces.xcarchive"
rm -rf "$ARCHIVE"
cd ios
if [ "$DRY" = 1 ]; then
  xcodebuild archive -workspace CuaSpaces.xcworkspace -scheme CuaSpaces -configuration Release \
    -destination generic/platform=iOS -archivePath "$ARCHIVE" \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" DEVELOPMENT_TEAM=$TEAM
  echo "dry run ok (unsigned): $ARCHIVE, build $CUA_BUILD_NUMBER"
  du -sh "$ARCHIVE"
  exit 0
fi
xcodebuild archive -workspace CuaSpaces.xcworkspace -scheme CuaSpaces -configuration Release \
  -destination generic/platform=iOS -archivePath "$ARCHIVE" \
  -allowProvisioningUpdates DEVELOPMENT_TEAM=$TEAM CODE_SIGN_STYLE=Automatic ${AUTH[@]+"${AUTH[@]}"}
xcodebuild -exportArchive -archivePath "$ARCHIVE" -exportPath "$BUILD_ROOT/export" \
  -exportOptionsPlist "$APP/scripts/ExportOptions.plist" -allowProvisioningUpdates ${AUTH[@]+"${AUTH[@]}"}
echo "uploaded build $CUA_BUILD_NUMBER to App Store Connect (processing takes a few minutes)"
