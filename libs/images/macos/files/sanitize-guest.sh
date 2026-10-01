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
