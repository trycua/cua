#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# unity-hub-markers.sh — keep Unity Hub's first-run markers suppressed.
#
# Unity Hub rewrites its own onboarding state, and it does so on QUIT. That
# defeats writing the markers once:
#
#   * at build time — the Hub runs during the build (the Editor is installed
#     through its CLI) and resets them on exit, so the golden ships
#     hideGetSetUp.json=false and hasSeenUnityCliAnnouncement=false;
#   * at boot — a teleport terminates the Hub and relaunches it, and the dying
#     instance flushes the reset values on the way out, so the relaunched Hub
#     comes up with the "Unity CLI is now available!" modal over the "Get set
#     up" pane. That is exactly what a demo run saw, after boot-time writes.
#
# So this runs BOTH at boot and, via the com.trycua.unity-hub-markers
# LaunchAgent's WatchPaths, every time anything in the Hub's support directory
# changes. It only writes when a value is actually wrong, so it does not chase
# its own file-system events forever.
SUPPORT="$HOME/Library/Application Support/UnityHub"
mkdir -p "$SUPPORT"

changed=0

# Bare JSON booleans, not objects: an earlier revision wrote {"seen":true} into
# firstTimeOpenKey.json, which the Hub reads as falsy.
want_bool() {  # file, value
  [ "$(cat "$SUPPORT/$1" 2>/dev/null)" = "$2" ] && return 0
  printf '%s' "$2" > "$SUPPORT/$1"
  changed=1
}
want_bool firstTimeOpenKey.json     true
want_bool hideGetSetUp.json         true
want_bool hideHubOnEditorOpen.json  true

read -r -d '' FIRST_TIME <<'JSON'
{"showLicenseProvisioning":false,"showEditorRecommendation":false,"showWelcomeModal":false,"showLearnTemplates":false,"showPersonalLicenseEulaFirstTimeModal":false,"hasSeenGetSetUp":true,"hubLicenseEulaAcceptedAt":"2026-06-22T00:00:00Z","hasSeenUnityCliAnnouncement":true}
JSON
if [ "$(cat "$SUPPORT/firstTimeSettings.json" 2>/dev/null)" != "$FIRST_TIME" ]; then
  printf '%s' "$FIRST_TIME" > "$SUPPORT/firstTimeSettings.json"
  changed=1
fi

/usr/bin/python3 - "$SUPPORT/hubConfig.json" <<'PY' 2>/dev/null
import json, sys
p = sys.argv[1]
try:
    cfg = json.load(open(p))
except Exception:
    cfg = {}
if cfg.get("hubDisableWelcomeScreen") is not True:
    cfg["hubDisableWelcomeScreen"] = True
    json.dump(cfg, open(p, "w"))
PY

# --- "Enable Developer Data Framework" banner -------------------------------
# A persistent blue banner on the Hub's home screen from first launch:
#   Enable **Developer Data Framework** data collection and sharing to access
#   advanced optimization, usage analytics, and the full range of product
#   capabilities.                                    [Enable Developer Data]
#
# It is NOT one of the Sanity CMS marketing banners. prepare-space.sh already
# null-routes fuvbjjlp.apicdn.sanity.io and that has no effect here: this banner
# is compiled into app.asar with local i18n strings and rendered by its own
# component, outside the `amplitudeFeatureFlag` guard every CMS banner carries.
# Neither is it behind a feature flag — `show-developer-data-framework` sits in
# cachedFeatureFlags.json but the string does not appear in app.asar at all, so
# Hub 3.21.2 never reads it, and no hubDisable* flag touches it.
#
# Its visibility, from app.asar:
#   shouldShowHomeBannerForCurrentOrg: e && !!t && !i.includes(t)
#   WK: s = canManageDeveloperData && !isBannerPrefsLoading && shouldShow...
# where t is the ORGANIZATION's genesis id and i is
#   developerData.bannerHiddenForOrganizationGenesisIds
# in user-settings.json — a plain array of strings compared with .includes(), so
# there is no wildcard and no sentinel. canManageDeveloperData is
# `role === "owner" || role === "manager"` on that same org, and a fresh Unity
# Personal user owns their own org, so it is true for every teleported identity.
#
# WHY THIS CANNOT BE A BUILD-TIME WRITE, unlike every other marker above. The
# suppression key is per-ORGANIZATION, and the golden ships no account at all —
# accounts.db is sanitised out and the identity arrives by teleport, different
# for every Space. At build time there is no genesis id to write. (With no
# account the org list is empty, genesisId is "", and the banner is correctly
# hidden; it only appears after a teleport.)
#
# The genesis id is also not derivable offline: accounts.db's `primary_org` is
# the org SLUG, and the slug -> genesis id mapping lives only in the response to
#   GET https://core.cloud.unity3d.com/api/users/me?include=orgs
# which is never persisted. That request does fail closed — a rejection sets the
# org list to [], which blanks genesisId AND the role, hiding the banner — but
# core.cloud.unity3d.com also serves /api/login/refresh and /api/users/{id}, so
# null-routing it would break the teleported login. There is no narrower host.
#
# So it is harvested at runtime instead. The id lands on disk incidentally, in
# the Hub's own log and Sentry breadcrumbs, as the org-scoped call
#   GET https://services.unity.com/api/unity/legacy/v1/organizations/<ID>
# a few seconds after login. This script runs from a WatchPaths agent over the
# Hub's support directory AND on a short interval, so it simply retries until
# the id is greppable rather than firing once and giving up.
hide_developer_data_banner() {
  local id
  id=$(grep -aoh 'unity/legacy/v1/organizations/[0-9]\{6,\}' \
         "$SUPPORT/logs/info-log.json" "$SUPPORT/sentry/scope_v3.json" 2>/dev/null \
       | grep -o '[0-9]\{6,\}' | tail -1)
  [ -n "$id" ] || return 0
  /usr/bin/python3 - "$SUPPORT/user-settings.json" "$id" <<'PY'
import json, os, sys
path, org = sys.argv[1], sys.argv[2]
try:
    cfg = json.load(open(path))
except Exception:
    cfg = {}
if not isinstance(cfg, dict):
    cfg = {}
dd = cfg.setdefault("developerData", {})
hidden = dd.setdefault("bannerHiddenForOrganizationGenesisIds", [])
if not isinstance(hidden, list):
    hidden = []
if org in hidden:
    sys.exit(1)          # already suppressed: write nothing, stay quiet
hidden.append(org)
dd["bannerHiddenForOrganizationGenesisIds"] = hidden
tmp = path + ".tmp"
with open(tmp, "w") as fh:
    json.dump(cfg, fh)
os.replace(tmp, path)
print(org)
PY
}
if NEWORG=$(hide_developer_data_banner) && [ -n "$NEWORG" ]; then
  echo "$(date '+%H:%M:%S') hid Developer Data Framework banner for org $NEWORG"
  changed=1
fi

[ "$changed" = 1 ] && echo "$(date '+%H:%M:%S') rewrote Unity Hub onboarding markers"
exit 0
