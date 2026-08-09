#!/usr/bin/env bash
# Install or update the cua KWin adapter for the current desktop user.
set -euo pipefail

readonly ADAPTER_ID="cua-kwin@cua"
readonly PLUGIN_ID="cua-kwin.cua"
readonly PROTOCOL_VERSION="1"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PACKAGE_DIR="$SCRIPT_DIR/$ADAPTER_ID"
readonly DATA_HOME="${XDG_DATA_HOME:-${HOME:?HOME is required}/.local/share}"
readonly DEST="$DATA_HOME/kwin/scripts/$PLUGIN_ID"

if [[ "$(id -u)" == "0" ]]; then
  echo "Refusing to install a desktop-session KWin script as root." >&2
  echo "Run this script as the user who owns the Plasma session." >&2
  exit 1
fi
if [[ ! -f "$PACKAGE_DIR/metadata.json" || ! -f "$PACKAGE_DIR/contents/code/main.js" ]]; then
  echo "The packaged KWin adapter is incomplete: $PACKAGE_DIR" >&2
  exit 1
fi
if ! grep -Eq '"X-Cua-Protocol-Version"[[:space:]]*:[[:space:]]*1([[:space:]}]|,)' \
  "$PACKAGE_DIR/metadata.json"; then
  echo "The packaged KWin adapter does not declare protocol v$PROTOCOL_VERSION." >&2
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

if [[ -e "$DEST" ]]; then
  kpackagetool6 --type KWin/Script --upgrade "$PACKAGE_DIR"
  action="Updated"
else
  kpackagetool6 --type KWin/Script --install "$PACKAGE_DIR"
  action="Installed"
fi
kwriteconfig6 \
  --file kwinrc \
  --group Plugins \
  --key "${PLUGIN_ID}Enabled" \
  --type bool \
  true

reload_result="KWin is not reachable on the session bus; it will load the adapter at the next Plasma session start."
qdbus_bin=""
if command -v qdbus6 >/dev/null 2>&1; then
  qdbus_bin="$(command -v qdbus6)"
elif command -v qdbus >/dev/null 2>&1; then
  qdbus_bin="$(command -v qdbus)"
fi

qdbus_script_loaded() {
  local loaded
  loaded="$("$qdbus_bin" org.kde.KWin /Scripting \
    org.kde.kwin.Scripting.isScriptLoaded "$PLUGIN_ID" 2>/dev/null)"
  [[ "$loaded" == "true" || "$loaded" == "1" ]]
}

gdbus_script_loaded() {
  local loaded
  loaded="$(gdbus call --session --dest org.kde.KWin --object-path /Scripting \
    --method org.kde.kwin.Scripting.isScriptLoaded "$PLUGIN_ID" 2>/dev/null)"
  [[ "$loaded" == *true* ]]
}

if [[ -n "$qdbus_bin" ]] && "$qdbus_bin" org.kde.KWin /KWin org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  # Unload only this package so an upgrade cannot leave its old source alive.
  "$qdbus_bin" org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$PLUGIN_ID" >/dev/null 2>&1 || true
  "$qdbus_bin" org.kde.KWin /KWin org.kde.KWin.reconfigure >/dev/null
  for _attempt in {1..20}; do
    qdbus_script_loaded && break
    sleep 0.05
  done
  if ! qdbus_script_loaded; then
    # KWin can cache its package inventory across a live install. Its public
    # scripting API can register this exact installed entry point immediately;
    # the enabled kwinrc key makes the same package persistent next session.
    "$qdbus_bin" org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript \
      "$DEST/contents/code/main.js" "$PLUGIN_ID" >/dev/null
    "$qdbus_bin" org.kde.KWin /Scripting org.kde.kwin.Scripting.start >/dev/null
  fi
  for _attempt in {1..20}; do
    qdbus_script_loaded && break
    sleep 0.05
  done
  if qdbus_script_loaded; then
    reload_result="KWin reconfigured and the adapter was loaded; no logout or login is required."
  else
    echo "KWin did not report $ADAPTER_ID loaded after reconfiguration." >&2
    echo "The package remains installed and enabled for the next Plasma session start." >&2
    exit 1
  fi
elif command -v gdbus >/dev/null 2>&1 \
  && gdbus call --session --dest org.kde.KWin --object-path /KWin \
    --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  gdbus call --session --dest org.kde.KWin --object-path /Scripting \
    --method org.kde.kwin.Scripting.unloadScript "$PLUGIN_ID" >/dev/null 2>&1 || true
  gdbus call --session --dest org.kde.KWin --object-path /KWin \
    --method org.kde.KWin.reconfigure >/dev/null
  for _attempt in {1..20}; do
    gdbus_script_loaded && break
    sleep 0.05
  done
  if ! gdbus_script_loaded; then
    gdbus call --session --dest org.kde.KWin --object-path /Scripting \
      --method org.kde.kwin.Scripting.loadScript \
      "$DEST/contents/code/main.js" "$PLUGIN_ID" >/dev/null
    gdbus call --session --dest org.kde.KWin --object-path /Scripting \
      --method org.kde.kwin.Scripting.start >/dev/null
  fi
  for _attempt in {1..20}; do
    gdbus_script_loaded && break
    sleep 0.05
  done
  if gdbus_script_loaded; then
    reload_result="KWin reconfigured and the adapter was loaded; no logout or login is required."
  else
    echo "KWin did not report $ADAPTER_ID loaded after reconfiguration." >&2
    echo "The package remains installed and enabled for the next Plasma session start." >&2
    exit 1
  fi
fi

echo "$action $ADAPTER_ID (KWin package $PLUGIN_ID) for the current user at $DEST."
echo "$reload_result"
echo "Verify with: $SCRIPT_DIR/diagnose.sh"
