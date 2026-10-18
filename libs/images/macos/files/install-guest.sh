#!/bin/bash
# Install cua-spacesd into a macOS Lume guest. Runs IN the guest as the
# autologin (desktop) user, from a staging directory the host shared in
# (build.sh), with the sudo password in CUA_SUDO_PW (default "lume").
#
#   install-guest.sh STAGE_DIR
#
# STAGE_DIR holds:
#   Cua Spacesd.app/          signed app bundle (build-macos-app.sh)
#   start-spacesd.sh          -> /opt/cua/bin/start-spacesd.sh
#   com.trycua.spacesd.plist  -> /Library/LaunchAgents (root:wheel 0644)
#   seed-tcc.sh               run once here, not shipped
#   image.json manifest.json  -> /etc/cua-image/ (build identity)
#   spacesd-source            optional: local (default) or release
#
# Layout (mirrors the Linux image where macOS allows):
#   /Applications/Cua Spacesd.app       the daemon, by bundle (TCC identity)
#   /usr/local/bin/cua-spacesd          symlink to the bundle's executable
#   /opt/cua/bin/start-spacesd.sh       token resolution + exec
#   /etc/cua-image/{image,manifest}.json, spacesd-source, variant
#   ~/.cua/spacesd/                     0700, token file (0600) at runtime
set -euo pipefail
STAGE="${1:?staging directory}"
APP="/Applications/Cua Spacesd.app"
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }
say() { echo "==> $*"; }

for f in "Cua Spacesd.app" start-spacesd.sh com.trycua.spacesd.plist seed-tcc.sh image.json manifest.json; do
    [ -e "$STAGE/$f" ] || { echo "missing $STAGE/$f" >&2; exit 1; }
done

say "app bundle -> $APP"
sudo_run rm -rf "$APP"
sudo_run ditto "$STAGE/Cua Spacesd.app" "$APP"
sudo_run chown -R root:wheel "$APP"
sudo_run xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true
codesign --verify --deep --strict "$APP"
sudo_run mkdir -p /usr/local/bin
sudo_run ln -sfn "$APP/Contents/MacOS/cua-spacesd" /usr/local/bin/cua-spacesd

say "launcher and LaunchAgent"
sudo_run install -d -o root -g wheel -m 0755 /opt/cua /opt/cua/bin
sudo_run install -o root -g wheel -m 0755 "$STAGE/start-spacesd.sh" /opt/cua/bin/start-spacesd.sh
sudo_run install -o root -g wheel -m 0644 "$STAGE/com.trycua.spacesd.plist" /Library/LaunchAgents/com.trycua.spacesd.plist
plutil -lint /Library/LaunchAgents/com.trycua.spacesd.plist

say "build identity -> /etc/cua-image"
sudo_run install -d -o root -g wheel -m 0755 /etc/cua-image /etc/cua
sudo_run install -o root -g wheel -m 0644 "$STAGE/image.json" /etc/cua-image/image.json
sudo_run install -o root -g wheel -m 0644 "$STAGE/manifest.json" /etc/cua-image/manifest.json
# (sudo_run owns stdin for the password, so no pipe into it.)
source_kind="$(cat "$STAGE/spacesd-source" 2>/dev/null || echo local)"
case "$source_kind" in local|release) ;; *) echo "bad spacesd-source $source_kind" >&2; exit 1 ;; esac
sudo_run sh -c "printf '%s\\n' '$source_kind' >/etc/cua-image/spacesd-source; printf 'lume\\n' >/etc/cua-image/variant"
sudo_run chmod 0644 /etc/cua-image/spacesd-source /etc/cua-image/variant
/usr/local/bin/cua-spacesd build-info

say "state directory"
mkdir -p "$HOME/.cua/spacesd"
chmod 700 "$HOME/.cua" "$HOME/.cua/spacesd"

say "TCC grants"
bash "$STAGE/seed-tcc.sh" "$APP"

say "load the LaunchAgent in the running session"
uid="$(id -u)"
launchctl bootout "gui/$uid/com.trycua.spacesd" 2>/dev/null || true
sudo_run launchctl bootstrap "gui/$uid" /Library/LaunchAgents/com.trycua.spacesd.plist ||
    echo "  (bootstrap deferred to the next login)"
echo "installed"
