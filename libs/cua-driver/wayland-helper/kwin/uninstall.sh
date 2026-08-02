#!/usr/bin/env bash
# Disable and remove only the current user's cua KWin adapter package.
set -euo pipefail

readonly ADAPTER_ID="cua-kwin@cua"
readonly PLUGIN_ID="cua-kwin.cua"
readonly DATA_HOME="${XDG_DATA_HOME:-${HOME:?HOME is required}/.local/share}"
readonly DEST="$DATA_HOME/kwin/scripts/$PLUGIN_ID"

if [[ "$(id -u)" == "0" ]]; then
  echo "Refusing to modify a desktop-session KWin configuration as root." >&2
  echo "Run this script as the user who owns the Plasma session." >&2
  exit 1
fi
if ! command -v kpackagetool6 >/dev/null 2>&1; then
  echo "kpackagetool6 is required (install the Plasma 6 KPackage tools)." >&2
  exit 1
fi
if ! command -v kwriteconfig6 >/dev/null 2>&1; then
  echo "kwriteconfig6 is required (install the Plasma 6 configuration tools)." >&2
  exit 1
fi

kwriteconfig6 \
  --file kwinrc \
  --group Plugins \
  --key "${PLUGIN_ID}Enabled" \
  --type bool \
  false

qdbus_bin=""
if command -v qdbus6 >/dev/null 2>&1; then
  qdbus_bin="$(command -v qdbus6)"
elif command -v qdbus >/dev/null 2>&1; then
  qdbus_bin="$(command -v qdbus)"
fi
if [[ -n "$qdbus_bin" ]] && "$qdbus_bin" org.kde.KWin /KWin org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  "$qdbus_bin" org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$PLUGIN_ID" >/dev/null 2>&1 || true
elif command -v gdbus >/dev/null 2>&1 \
  && gdbus call --session --dest org.kde.KWin --object-path /KWin \
    --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  gdbus call --session --dest org.kde.KWin --object-path /Scripting \
    --method org.kde.kwin.Scripting.unloadScript "$PLUGIN_ID" >/dev/null 2>&1 || true
fi

if [[ -e "$DEST" ]]; then
  kpackagetool6 --type KWin/Script --remove "$PLUGIN_ID"
else
  echo "$PLUGIN_ID is not installed at $DEST."
fi

if [[ -n "$qdbus_bin" ]] && "$qdbus_bin" org.kde.KWin /KWin org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  "$qdbus_bin" org.kde.KWin /KWin org.kde.KWin.reconfigure >/dev/null
  echo "KWin reconfigured; no logout or login is required."
elif command -v gdbus >/dev/null 2>&1 \
  && gdbus call --session --dest org.kde.KWin --object-path /KWin \
    --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  gdbus call --session --dest org.kde.KWin --object-path /KWin \
    --method org.kde.KWin.reconfigure >/dev/null
  echo "KWin reconfigured; no logout or login is required."
else
  echo "KWin is not reachable; the disabled setting takes effect at the next Plasma session start."
fi
echo "Removed the current-user $ADAPTER_ID adapter (KWin package $PLUGIN_ID)."
