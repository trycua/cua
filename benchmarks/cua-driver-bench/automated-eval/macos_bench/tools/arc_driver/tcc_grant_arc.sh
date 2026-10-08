#!/bin/zsh
# Amendment 9: grant Accessibility and Screen Recording (system TCC.db) to ArcDriverBench.app only. VM only (SIP is
# disabled in the bench VM). Unlike tools/vm_main_build/tcc_grant.sh it grants no Automation (arc-driver sends no
# Apple events), and it never touches Terminal's rows.
#   tcc_grant_arc.sh <app> <bundle id>        e.g. ~/bench-work/arc-cua/ArcDriverBench.app com.trycua.bench.arcdriver
#   tcc_grant_arc.sh --revoke <bundle id>     remove both rows again
set -euo pipefail
SYS="/Library/Application Support/com.apple.TCC/TCC.db"
if [ "${1:-}" = "--revoke" ]; then
  sudo sqlite3 "$SYS" "DELETE FROM access WHERE client='$2' AND service IN ('kTCCServiceAccessibility','kTCCServiceScreenCapture');"
else
  A=$1
  ID=$2
  REQ=$(codesign -d -r- $A 2>&1 | sed -n 's/^# *designated => //p')
  T=$(mktemp -d)
  echo "$REQ" | csreq -r- -b $T/arc.csreq
  HEX=$(xxd -p $T/arc.csreq | tr -d '\n')
  rm -rf $T
  for S in kTCCServiceAccessibility kTCCServiceScreenCapture; do
    sudo sqlite3 "$SYS" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified, pid, pid_version, boot_uuid, last_reminded) VALUES ('$S', '$ID', 0, 2, 4, 1, X'$HEX', NULL, 0, 'UNUSED', NULL, 0, CAST(strftime('%s','now') AS INTEGER), NULL, NULL, 'UNUSED', CAST(strftime('%s','now') AS INTEGER));"
  done
fi
sudo sqlite3 "$SYS" "select service, client, auth_value, length(csreq) from access where client='${2:-}';"
# restart tccd (exact PIDs) so the rows are read again
for P in $(pgrep -x tccd); do sudo kill $P 2>/dev/null || kill $P 2>/dev/null; done
sleep 2
