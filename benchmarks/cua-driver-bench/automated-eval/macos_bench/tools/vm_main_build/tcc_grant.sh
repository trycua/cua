#!/bin/zsh
# Grant Accessibility + Screen Recording (system TCC.db) and Automation (user TCC.db) to the main-build app.
# VM only (SIP is disabled in this VM). Mirrors the grants com.trycua.driver (0.34.0) already has.
set -e
A=~/bench-work/cua-main/CuaDriverBenchMain.app
ID=com.trycua.driver.benchmain
REQ=$(codesign -d -r- $A 2>&1 | sed -n 's/^# *designated => //p')
echo "$REQ" | csreq -r- -b /tmp/benchmain.csreq
HEX=$(xxd -p /tmp/benchmain.csreq | tr -d '\n')
SYS="/Library/Application Support/com.apple.TCC/TCC.db"
for S in kTCCServiceAccessibility kTCCServiceScreenCapture; do
  sudo sqlite3 "$SYS" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified, pid, pid_version, boot_uuid, last_reminded) VALUES ('$S', '$ID', 0, 2, 4, 1, X'$HEX', NULL, 0, 'UNUSED', NULL, 0, CAST(strftime('%s','now') AS INTEGER), NULL, NULL, 'UNUSED', CAST(strftime('%s','now') AS INTEGER));"
done
USR=~/Library/Application\ Support/com.apple.TCC/TCC.db
sqlite3 "$USR" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified, pid, pid_version, boot_uuid, last_reminded) SELECT service, '$ID', client_type, auth_value, auth_reason, auth_version, X'$HEX', policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, CAST(strftime('%s','now') AS INTEGER), NULL, NULL, 'UNUSED', CAST(strftime('%s','now') AS INTEGER) FROM access WHERE client='com.trycua.driver' AND service='kTCCServiceAppleEvents';"
sudo sqlite3 "$SYS" "select service, client, auth_value, length(csreq) from access where client='$ID';"
sqlite3 "$USR" "select service, client, auth_value, indirect_object_identifier from access where client='$ID';"
# restart tccd (exact PIDs) so the new rows are read
for P in $(pgrep -x tccd); do sudo kill $P 2>/dev/null || kill $P 2>/dev/null; done
sleep 2; pgrep -x tccd | wc -l

