#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Proves a Local build, "Cua Spaces Local.app" (scripts/build-app.sh --channel
# local), cannot touch an installed Cua Spaces (CUA-1776). It fails when the
# bundle has:
#
#  - the production bundle id, or any bundle id, name or app folder that is
#    not the Local one (the names come from CuaChannel, through
#    scripts/channel-names.sh);
#  - a Sparkle feed or key, or automatic checks (a Local build must never
#    update itself into the production app, or be offered one);
#  - the `cua` URL scheme (cua://add/<code> belongs to Stable), or anything
#    but `cua-local`;
#  - Info.plist keys that do not say Local: CuaChannel, CuaEnv (staging),
#    CuaHome, CuaHelperLabel, CuaUrlScheme;
#  - a helper login item that is not the Local one: its plist file name,
#    its Label and its AssociatedBundleIdentifiers, or Stable's helper plist
#    next to it;
#  - a signature under another identifier than the Local bundle id.
#
#   scripts/check-local-bundle.sh "path/Cua Spaces Local.app"
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="${1:?usage: check-local-bundle.sh <path/Cua Spaces Local.app>}"
app="${app%/}"
plist="$app/Contents/Info.plist"
[ -f "$plist" ] || { echo "$app has no Contents/Info.plist" >&2; exit 1; }

# The Stable names a Local bundle must never carry. They are the invariant
# this check exists for, so they are written out here and not read from the
# code under test.
prod_id=com.trycua.spaces.macos
prod_scheme=cua
prod_helper=com.trycua.spaces.helper

# The Local names, from CuaChannel.
names="$("$here/scripts/channel-names.sh" local)"
name() { printf '%s\n' "$names" | sed -n "s/^$1=//p"; }
want_app_name="$(name app_name)"
want_id="$(name bundle_id)"
want_helper="$(name helper_label)"
want_scheme="$(name url_scheme)"
want_home="$(name home)"
want_env="$(name env)"
for v in want_app_name want_id want_helper want_scheme want_home want_env; do
  [ -n "${!v}" ] || { echo "scripts/channel-names.sh local printed no ${v#want_}" >&2; exit 1; }
done

fail=0
bad() { echo "local bundle check: $1" >&2; fail=1; }
pb=/usr/libexec/PlistBuddy
key() { $pb -c "Print :$1" "$plist" 2>/dev/null || true; }
expect() { # expect KEY VALUE
  [ "$(key "$1")" = "$2" ] || bad "$1 is '$(key "$1")', want '$2'"
}

# Identity.
id="$(key CFBundleIdentifier)"
[ "$id" != "$prod_id" ] || bad "bundle id is the production one ($prod_id)"
expect CFBundleIdentifier "$want_id"
expect CFBundleName "$want_app_name"
expect CFBundleDisplayName "$want_app_name"
[ "$(basename "$app")" = "$want_app_name.app" ] || bad "app folder is '$(basename "$app")', want '$want_app_name.app'"

# No updater: no feed, no key, no automatic checks.
for k in SUFeedURL SUPublicEDKey SUEnableAutomaticChecks SUAutomaticallyUpdate SUVerifyUpdateBeforeExtraction; do
  [ -z "$(key "$k")" ] || bad "$k is set: a Local build could update itself into production"
done

# URL schemes: cua-local only, never cua.
schemes=()
i=0
while $pb -c "Print :CFBundleURLTypes:$i" "$plist" >/dev/null 2>&1; do
  j=0
  while s="$($pb -c "Print :CFBundleURLTypes:$i:CFBundleURLSchemes:$j" "$plist" 2>/dev/null)"; do
    schemes+=("$s")
    j=$((j + 1))
  done
  urlname="$(key "CFBundleURLTypes:$i:CFBundleURLName")"
  [ "$urlname" != "$prod_id" ] || bad "URL type $i is named for the production bundle id"
  i=$((i + 1))
done
for s in ${schemes[@]+"${schemes[@]}"}; do
  [ "$s" != "$prod_scheme" ] || bad "the '$prod_scheme' URL scheme is registered: cua:// belongs to Stable"
  [ "$s" = "$want_scheme" ] || bad "unexpected URL scheme '$s' (only '$want_scheme')"
done
case " ${schemes[*]-} " in
  *" $want_scheme "*) ;;
  *) bad "the '$want_scheme' URL scheme is not registered" ;;
esac

# What the app and its helpers read to know they are Local.
expect CuaChannel local
expect CuaEnv "$want_env"
expect CuaHome "$want_home"
expect CuaHelperLabel "$want_helper"
expect CuaUrlScheme "$want_scheme"

# The helper login item: its own plist, label and owner.
agents="$app/Contents/Library/LaunchAgents"
[ ! -e "$agents/$prod_helper.plist" ] || bad "Stable's helper plist ($prod_helper.plist) is in the bundle"
helper="$agents/$want_helper.plist"
if [ -f "$helper" ]; then
  label="$($pb -c "Print :Label" "$helper" 2>/dev/null || true)"
  [ "$label" = "$want_helper" ] || bad "helper plist Label is '$label', want '$want_helper'"
  [ "$label" != "$prod_helper" ] || bad "helper plist Label is the production one"
  owner="$($pb -c "Print :AssociatedBundleIdentifiers:0" "$helper" 2>/dev/null || true)"
  [ "$owner" = "$want_id" ] || bad "helper AssociatedBundleIdentifiers is '$owner', want '$want_id'"
  if $pb -c "Print :AssociatedBundleIdentifiers:1" "$helper" >/dev/null 2>&1; then
    bad "helper has more than one AssociatedBundleIdentifiers entry"
  fi
else
  bad "no helper plist $want_helper.plist in Contents/Library/LaunchAgents"
fi
others="$(find "$agents" -maxdepth 1 -name '*.plist' ! -name "$want_helper.plist" 2>/dev/null || true)"
[ -z "$others" ] || bad "other launch agents in the bundle: $(printf '%s' "$others" | tr '\n' ' ')"

# The signature (build-app.sh signs ad hoc under the bundle id; install-local
# re-signs with the stable local certificate, under the same identifier).
signed="$(codesign -dv "$app" 2>&1 | sed -n 's/^Identifier=//p')"
[ "$signed" = "$want_id" ] || bad "signed as '$signed', want '$want_id'"

[ "$fail" = 0 ] || exit 1
echo "local bundle ok: $id"
