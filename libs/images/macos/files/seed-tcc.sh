#!/bin/bash
# Pre-grant cua-spacesd the TCC services it needs (Screen Recording,
# Accessibility, synthetic input), so no permission prompt ever appears in a
# sandbox: TCC prompts ignore synthetic clicks, so the grant has to exist
# before the first capture. Runs in the guest as the desktop user at image
# build time; needs SIP disabled (true on the Lume base) and the sudo
# password in CUA_SUDO_PW (default "lume").
#
#   seed-tcc.sh [APP...]     default: "/Applications/Cua Spacesd.app"
#
# Same technique as the Cua Spaces golden (apps/cua-spaces/scripts/golden/
# seed-tcc.sh), reduced to the canonical image's one daemon:
#   - Accessibility, ScreenCapture and PostEvent live in the SYSTEM db.
#   - A row is honoured only when its csreq matches the client's designated
#     requirement, so the csreq is derived from the exact installed bundle
#     (`codesign -d -r-`). For an ad-hoc signature that requirement is
#     `cdhash H"..."`: it holds for exactly the binary this image ships, and
#     a rebuilt binary needs a re-seed. Rows are written both by bundle id
#     (client_type 0) and by executable path (client_type 1).
#   - The ReplayKit approval ledger gets a hint date far in the future, or
#     macOS shows "... is requesting to bypass the system private window
#     picker" on the first capture (that alert is not in TCC.db).
#   - Both tccd daemons are restarted (launchctl kickstart; tccd ignores
#     signals) so the rows take effect without a reboot.
# Exits non-zero unless every grant reads back from the database.
set -u

SYSTEM_DB="/Library/Application Support/com.apple.TCC/TCC.db"
SERVICES="kTCCServiceAccessibility kTCCServiceScreenCapture kTCCServicePostEvent"
SCAP_PLIST="$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"
SCAP_POLICY=3153600000 # 100 years, in seconds
[ $# -gt 0 ] || set -- "/Applications/Cua Spacesd.app"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }

if [ "$(csrutil status 2>/dev/null)" != "System Integrity Protection status: disabled." ]; then
    echo "FATAL: SIP is enabled in this guest; TCC.db cannot be written" >&2
    exit 1
fi

# Compiles a code object's designated requirement into a csreq blob (hex).
# The "# " an ad-hoc (implicit) requirement starts with is stripped: csreq
# would read the whole line as a comment.
csreq_hex() {
    codesign -d -r- "$1" 2>/dev/null |
        sed -n 's/^# *designated => //p; s/^designated => //p' >"$TMP/req.txt"
    [ -s "$TMP/req.txt" ] || return 1
    /usr/bin/csreq -r "$TMP/req.txt" -b "$TMP/req.bin" 2>/dev/null || return 1
    xxd -p "$TMP/req.bin" | tr -d '\n'
}

grant() { # service client client_type csreq_hex
    sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" "INSERT OR REPLACE INTO access
      (service,client,client_type,auth_value,auth_reason,auth_version,csreq,flags,last_modified,last_reminded)
      VALUES ('$1','$2',$3,2,2,1,X'$4',0,strftime('%s','now'),strftime('%s','now'));"
}

bids=()
failed=0
for app in "$@"; do
    [ -d "$app" ] || { echo "FATAL: missing $app" >&2; exit 1; }
    bid="$(/usr/bin/plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist")"
    exe="$app/Contents/MacOS/$(/usr/bin/plutil -extract CFBundleExecutable raw "$app/Contents/Info.plist")"
    hex="$(csreq_hex "$app")" || { echo "FATAL: $app has no designated requirement (unsigned?)" >&2; exit 1; }
    ehex="$(csreq_hex "$exe")" || ehex="$hex"
    echo "$app: $bid, requirement: $(cat "$TMP/req.txt")"
    for svc in $SERVICES; do
        grant "$svc" "$bid" 0 "$hex" && grant "$svc" "$exe" 1 "$ehex" &&
            echo "  granted $svc" || failed=1
    done
    bids+=("$bid")
done

# A sticky Accessibility denial for the SSH identity (from an AX call run by
# hand over ssh) would be inherited by every clone. Remove it; never grant it.
sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" "DELETE FROM access WHERE service='kTCCServiceAccessibility'
  AND client LIKE '%sshd-keygen-wrapper%' AND auth_value=0;"

# ReplayKit approval ledger, rewritten whole (bundle ids contain dots, which
# plutil keypaths would read as nesting).
mkdir -p "$(dirname "$SCAP_PLIST")"
now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
future="$(date -u -v+100y +%Y-%m-%dT%H:%M:%SZ)"
{
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    echo '<plist version="1.0"><dict>'
    for bid in "${bids[@]}" com.apple.screensharing.agent; do
        printf '<key>%s</key><dict>' "$bid"
        printf '<key>kScreenCaptureApprovalLastAlerted</key><date>%s</date>' "$now"
        printf '<key>kScreenCaptureApprovalLastUsed</key><date>%s</date>' "$now"
        printf '<key>kScreenCapturePrivacyHintDate</key><date>%s</date>' "$future"
        printf '<key>kScreenCapturePrivacyHintPolicy</key><integer>%s</integer>' "$SCAP_POLICY"
        printf '<key>kScreenCaptureAlertableUsageCount</key><integer>0</integer></dict>\n'
    done
    echo '</dict></plist>'
} >"$SCAP_PLIST.tmp"
/usr/bin/plutil -convert binary1 "$SCAP_PLIST.tmp" && mv -f "$SCAP_PLIST.tmp" "$SCAP_PLIST" &&
    chmod 600 "$SCAP_PLIST" && echo "screen-capture approvals seeded (next hint $future)" || failed=1

# tccd caches answers; restart both (the system one serves the rows above).
launchctl kickstart -k "gui/$(id -u)/com.apple.tccd" >/dev/null 2>&1
sudo_run launchctl kickstart -k system/com.apple.tccd.system >/dev/null 2>&1 &&
    echo "tccd restarted" || echo "WARNING: system tccd not restarted; grants apply at next boot"

# Read every grant back: a failed write must fail the build, not ship.
for bid in "${bids[@]}"; do
    for svc in $SERVICES; do
        n="$(sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" "SELECT count(*) FROM access WHERE service='$svc'
          AND client='$bid' AND client_type=0 AND auth_value=2 AND csreq IS NOT NULL;" | tr -d '[:space:]')"
        [ "$n" = 1 ] || { echo "MISSING: $svc -> $bid" >&2; failed=1; }
    done
done
[ "$failed" = 0 ] || { echo "FAILED: TCC grants incomplete" >&2; exit 1; }
echo "verified: TCC grants present for ${bids[*]}"
