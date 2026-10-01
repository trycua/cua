#!/usr/bin/env bash
# Pre-approve the macOS 15+/Tahoe periodic "Screen & System Audio Recording"
# reminder for the given bundle identifiers.
#
# macOS Sequoia (15) added — and Tahoe (26) keeps — a periodic screen-recording
# reminder dialog that fires for ANY material use of ScreenCaptureKit which does
# not go through the interactive content-sharing picker (SCContentFilter
# window/display capture). It is SEPARATE from the TCC Screen Recording grant
# (kTCCServiceScreenCapture): even a fully TCC-granted app is reminded, and on a
# fresh macOS user the FIRST capture is reminded immediately. In a headless,
# cloned Lume VM nobody can click "Allow", so any screen-capturing dependency
# (rcdpd, the Cua driver) would stall the automation.
#
# The reminder schedule lives in replayd's per-user approvals plist, keyed by
# bundle id. Pre-dating kScreenCapturePrivacyHintDate (the next-prompt time)
# far into the future suppresses the reminder for good; a clone inherits it.
#
# The proper Apple-blessed alternative is the com.apple.developer.persistent-
# content-capture entitlement, but that needs a provisioning profile Apple only
# issues to allow-listed teams, so it is not available to a dev-signed build.
#
# Run inside the Lume golden AS THE TARGET USER (no sudo: this is the user's own
# group container). Pass one or more bundle ids:
#   seed-screencapture-approvals.sh com.trycua.rcdphost com.trycua.driver.local
set -euo pipefail

FUTURE_DATE="${CUA_SCREENCAPTURE_FUTURE_DATE:-3024-01-01T00:00:00Z}"

(($#)) || { echo "usage: $0 <bundle-id> [<bundle-id>...]" >&2; exit 2; }

MODEL="$(/usr/sbin/sysctl -n hw.model 2>/dev/null || true)"
[[ "${MODEL}" == VirtualMac* ]] \
  || { echo "refusing to edit screen-capture approvals outside a Lume/VirtualMac guest (hw.model=${MODEL:-unknown})" >&2; exit 2; }

GROUP_CONTAINER="${HOME}/Library/Group Containers/group.com.apple.replayd"
PLIST="${GROUP_CONTAINER}/ScreenCaptureApprovals.plist"
/bin/mkdir -p "${GROUP_CONTAINER}"

# Rewrite the plist from scratch with a far-future reminder for each bundle id.
# In the golden only our own capturers ever appear here, so a full rewrite is
# simpler and deterministic than an in-place edit.
{
  printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
  printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
  printf '%s\n' '<plist version="1.0">'
  printf '%s\n' '<dict>'
  for bid in "$@"; do
    printf '  <key>%s</key>\n' "${bid}"
    printf '%s\n' '  <dict>'
    printf '%s\n' '    <key>kScreenCaptureAlertableUsageCount</key><integer>0</integer>'
    printf '    <key>kScreenCaptureApprovalLastAlerted</key><date>%s</date>\n' "${FUTURE_DATE}"
    printf '    <key>kScreenCaptureApprovalLastUsed</key><date>%s</date>\n' "${FUTURE_DATE}"
    printf '    <key>kScreenCapturePrivacyHintDate</key><date>%s</date>\n' "${FUTURE_DATE}"
    printf '%s\n' '    <key>kScreenCapturePrivacyHintPolicy</key><integer>2592000</integer>'
    printf '%s\n' '  </dict>'
  done
  printf '%s\n' '</dict>'
  printf '%s\n' '</plist>'
} > "${PLIST}"

/usr/bin/plutil -lint "${PLIST}" >/dev/null
/usr/bin/plutil -convert binary1 "${PLIST}"

# replayd caches the schedule; restart it so the far-future dates take effect
# without requiring a logout/login. (A clone boots replayd fresh anyway.)
/usr/bin/killall replayd 2>/dev/null || true

# Flush to the disk image. The golden is frozen with a hard `lume stop`
# (power-off, not a graceful shutdown), so an unflushed write would be lost and
# clones would inherit the pre-seed state. sync guarantees the plist is on disk.
/bin/sync

echo "seeded screen-capture approvals (next reminder ${FUTURE_DATE}) for: $*"
