#!/bin/bash
# Remove everything the build and its doctor run left behind, so no token,
# log or build input ships. Runs IN the guest as the desktop user, last,
# right before the VM is shut down for the push. Exits non-zero if a token
# or build input survives.
set -u
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }

rm -rf "$HOME/.cua/spacesd" "$HOME"/Downloads/cua-doctor-* /tmp/cua-doctor /tmp/cua-spacesd.log \
    "$HOME/.zsh_history" "$HOME/.bash_history" "$HOME/.zsh_sessions" "$HOME/.vnc.env"
mkdir -p "$HOME/.cua/spacesd"
chmod 700 "$HOME/.cua" "$HOME/.cua/spacesd"
sudo_run rm -rf /tmp/cua-build-* /var/root/.zsh_history /var/root/.bash_history
# The build's accessibility probe (tools/ax_probe.py) runs Calculator.
rm -rf "$HOME/Library/Saved Application State/com.apple.calculator.savedState" \
    "$HOME/Library/Containers/com.apple.calculator"

# macOS posts "App Background Activity: bash can run in the background" when
# the LaunchAgent is first registered. It is an alert that stays until it is
# dismissed, so every clone would show it at login; drop it from the
# notification store (the item itself stays approved in BTM).
NDB="$HOME/Library/Group Containers/group.com.apple.usernoted/db2/db"
drop_btm_alerts() {
    /usr/bin/sqlite3 "$NDB" "DELETE FROM record WHERE app_id IN
      (SELECT app_id FROM app WHERE identifier='com.apple.btmnotificationagent');" 2>/dev/null
}
if [ -f "$NDB" ]; then
    drop_btm_alerts
    launchctl kill SIGKILL "gui/$(id -u)/com.apple.usernoted" 2>/dev/null
    sleep 2
    drop_btm_alerts
fi

# The login keychain must take the account password ("lume"), or no installed
# secret can ever be pre-authorized for its app: teleporting Chrome's session
# raises "Google Chrome wants to access key 'Chrome Safe Storage' ... enter the
# 'login' keychain password" in an unattended Space, and cua-spacesd's
# `set-generic-password-partition-list -k lume` fails with "passphrase you
# entered is not correct". Cause: /etc/kcpassword (the autologin password) is
# the password XOR a fixed 11-byte key, and the plaintext must be NUL-padded
# to a multiple of 12 BEFORE the XOR. lume's offline patcher XOR'd first and
# appended raw zeroes, so loginwindow decodes the password as "lume" plus
# garbage, cannot unlock the login keychain at autologin, moves it aside
# (login_renamed_N.keychain-db, every boot) and makes a replacement keyed to
# a passphrase nobody knows. Autologin itself still works, so nothing looks
# broken. Rewrite the file correctly and give the login keychain the account
# password; safe here, at build time, because the guest shuts down right
# after (swapping a keychain under a live session leaves the session holding
# a different, locked one). Same repair as the Spaces golden image.
/usr/bin/python3 - >/tmp/kcpassword.new <<'KCP'
import sys
key = bytes.fromhex("7d895223d2bcddeaa3b91f")
pw = b"lume"
padded = pw + b"\x00" * (12 - len(pw) % 12)
sys.stdout.buffer.write(bytes(b ^ key[i % len(key)] for i, b in enumerate(padded)))
KCP
sudo_run /bin/cp /tmp/kcpassword.new /etc/kcpassword
sudo_run /usr/sbin/chown root:wheel /etc/kcpassword
sudo_run /bin/chmod 600 /etc/kcpassword
rm -f /tmp/kcpassword.new
KC="$HOME/Library/Keychains"
rm -f "$KC/login.keychain-db"* "$KC"/login_renamed_*.keychain-db "$KC/cua.keychain-db"*
security create-keychain -p lume "$KC/login.keychain-db"
security default-keychain -d user -s "$KC/login.keychain-db"
security list-keychains -d user -s "$KC/login.keychain-db"
security unlock-keychain -p lume "$KC/login.keychain-db"
security set-keychain-settings "$KC/login.keychain-db"
# Prove it takes the password (unlock-keychain on an unlocked keychain proves
# nothing; only a command that takes -k checks it).
security add-generic-password -U -s cua-keychain-probe -a probe -w x -A "$KC/login.keychain-db" 2>/dev/null
if security set-generic-password-partition-list -S "apple:,apple-tool:" -k lume \
    -s cua-keychain-probe -a probe "$KC/login.keychain-db" >/dev/null 2>&1; then
    echo "login keychain takes the account password"
else
    echo "WARNING: the login keychain does not take the account password; teleported app keys will prompt" >&2
fi
security delete-generic-password -s cua-keychain-probe -a probe "$KC/login.keychain-db" >/dev/null 2>&1
security unlock-keychain -p lume "$KC/login.keychain-db"

left=""
[ -z "$(ls -A "$HOME/.cua/spacesd")" ] || left="$left ~/.cua/spacesd/*"
[ ! -e /etc/cua/env-token ] || left="$left /etc/cua/env-token"
n="$(/usr/bin/sqlite3 "$NDB" "SELECT count(*) FROM record r JOIN app a ON a.app_id=r.app_id
  WHERE a.identifier='com.apple.btmnotificationagent';" 2>/dev/null || echo 0)"
[ "${n:-0}" = 0 ] || left="$left btm-notification($n)"
if [ -n "$left" ]; then
    echo "FAILED: still present:$left" >&2
    exit 1
fi
echo "sanitized"
