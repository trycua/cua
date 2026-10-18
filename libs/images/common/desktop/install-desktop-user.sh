#!/usr/bin/env bash
# Build-time: create the unprivileged desktop user and its quiet XFCE profile.
# Usage: install-desktop-user.sh <user> <uid>
set -euo pipefail
USER_NAME="${1:-cua}"
USER_UID="${2:-1000}"
SRC="$(cd "$(dirname "$0")" && pwd)"

# ubuntu:24.04 ships an "ubuntu" user on uid 1000; take the uid over.
if id -u ubuntu >/dev/null 2>&1 && [ "$(id -u ubuntu)" = "$USER_UID" ]; then
    userdel -r ubuntu 2>/dev/null || userdel ubuntu
fi
useradd -m -u "$USER_UID" -s /bin/bash -G sudo,audio,video "$USER_NAME"
passwd -l "$USER_NAME" >/dev/null
echo "$USER_NAME ALL=(ALL) NOPASSWD:ALL" >"/etc/sudoers.d/90-$USER_NAME"
chmod 0440 "/etc/sudoers.d/90-$USER_NAME"

H="/home/$USER_NAME"
install -d -m 0755 "$H/.config/xfce4/xfconf/xfce-perchannel-xml" "$H/.config/autostart" \
    "$H/Desktop" "$H/.vnc" "$H/.cache"
install -m 0644 "$SRC/xfce4-power-manager.xml" "$H/.config/xfce4/xfconf/xfce-perchannel-xml/"
printf 'WebBrowser=firefox\nTerminalEmulator=xfce4-terminal\n' >"$H/.config/xfce4/helpers.rc"
# No screensaver, locker, power manager or first-run tips: they steal focus and
# blank the screen under test.
for app in xfce4-screensaver light-locker xfce4-power-manager xscreensaver xfce4-tips-autostart \
    blueman update-notifier nm-applet; do
    printf '[Desktop Entry]\nHidden=true\n' >"$H/.config/autostart/$app.desktop"
done
chown -R "$USER_NAME:$USER_NAME" "$H"

# tmpfiles.d so the VM variant gets the session runtime dir. It is NOT
# /run/user/<uid>: logind mounts a fresh tmpfs there for every login (e.g. an
# SSH session) and removes it at logout, which would pull the desktop's bus and
# audio sockets out from under it. (The container entrypoint creates it itself.)
cat >/usr/lib/tmpfiles.d/cua-desktop-user.conf <<EOT
d /run/cua-desktop 0700 $USER_NAME $USER_NAME -
EOT
