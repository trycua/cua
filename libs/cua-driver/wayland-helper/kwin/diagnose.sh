#!/usr/bin/env bash
# Read-only installation and live-session diagnostics for the cua KWin adapter.
set -uo pipefail

readonly ADAPTER_ID="cua-kwin@cua"
readonly PLUGIN_ID="cua-kwin.cua"
readonly PROTOCOL_VERSION="1"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PACKAGE_DIR="$SCRIPT_DIR/$ADAPTER_ID"
readonly DATA_HOME="${XDG_DATA_HOME:-${HOME:?HOME is required}/.local/share}"
readonly DEST="$DATA_HOME/kwin/scripts/$PLUGIN_ID"
status=0

pass() { printf 'ok: %s\n' "$1"; }
fail() { printf 'error: %s\n' "$1" >&2; status=1; }
note() { printf 'note: %s\n' "$1"; }

if [[ "${XDG_SESSION_TYPE:-}" == "wayland" && -n "${WAYLAND_DISPLAY:-}" ]]; then
  pass "Wayland session detected (${WAYLAND_DISPLAY})."
else
  fail "This is not a live Wayland session (XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unset}, WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unset})."
fi
case ";${XDG_CURRENT_DESKTOP:-};" in
  *';KDE;'*|*';KDE:'*|*':KDE;'*) pass "KDE Plasma desktop detected." ;;
  *) fail "KDE Plasma was not detected (XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-unset})." ;;
esac

if [[ -f "$DEST/metadata.json" && -f "$DEST/contents/code/main.js" ]]; then
  pass "Adapter package is installed for the current user at $DEST."
else
  fail "Adapter package is missing or incomplete at $DEST."
fi
if [[ -f "$DEST/metadata.json" ]] \
  && grep -Eq '"X-Cua-Protocol-Version"[[:space:]]*:[[:space:]]*1([[:space:]}]|,)' "$DEST/metadata.json"; then
  pass "Installed metadata declares supported protocol v$PROTOCOL_VERSION."
else
  fail "Installed metadata does not declare supported protocol v$PROTOCOL_VERSION."
fi
if [[ -f "$PACKAGE_DIR/metadata.json" && -f "$PACKAGE_DIR/contents/code/main.js" \
  && -f "$DEST/metadata.json" && -f "$DEST/contents/code/main.js" ]]; then
  if cmp -s "$PACKAGE_DIR/metadata.json" "$DEST/metadata.json" \
    && cmp -s "$PACKAGE_DIR/contents/code/main.js" "$DEST/contents/code/main.js"; then
    pass "Installed adapter matches this packaged source."
  else
    fail "Installed adapter differs from this packaged source (update required)."
  fi
fi

if command -v kreadconfig6 >/dev/null 2>&1; then
  enabled="$(kreadconfig6 --file kwinrc --group Plugins --key "${PLUGIN_ID}Enabled" --default false 2>/dev/null)"
  if [[ "$enabled" == "true" || "$enabled" == "1" ]]; then
    pass "Adapter is enabled in the current user's kwinrc."
  else
    fail "Adapter is not enabled in the current user's kwinrc."
  fi
else
  fail "kreadconfig6 is unavailable, so the enabled setting cannot be checked."
fi

qdbus_bin=""
if command -v qdbus6 >/dev/null 2>&1; then
  qdbus_bin="$(command -v qdbus6)"
elif command -v qdbus >/dev/null 2>&1; then
  qdbus_bin="$(command -v qdbus)"
fi
if [[ -n "$qdbus_bin" ]] && "$qdbus_bin" org.kde.KWin /KWin org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  pass "The KWin well-known service is reachable on the current session bus."
  loaded="$($qdbus_bin org.kde.KWin /Scripting org.kde.kwin.Scripting.isScriptLoaded "$PLUGIN_ID" 2>/dev/null)"
  if [[ "$loaded" == "true" || "$loaded" == "1" ]]; then
    pass "KWin reports $ADAPTER_ID loaded as package $PLUGIN_ID."
  else
    fail "KWin does not report $ADAPTER_ID loaded as package $PLUGIN_ID."
  fi
elif command -v gdbus >/dev/null 2>&1 \
  && gdbus call --session --dest org.kde.KWin --object-path /KWin \
    --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  pass "The KWin well-known service is reachable on the current session bus."
  loaded="$(gdbus call --session --dest org.kde.KWin --object-path /Scripting \
    --method org.kde.kwin.Scripting.isScriptLoaded "$PLUGIN_ID" 2>/dev/null)"
  if [[ "$loaded" == *true* ]]; then
    pass "KWin reports $ADAPTER_ID loaded as package $PLUGIN_ID."
  else
    fail "KWin does not report $ADAPTER_ID loaded as package $PLUGIN_ID."
  fi
else
  fail "org.kde.KWin is not reachable on this session bus."
fi

if (( status != 0 )); then
  note "Remediation: $SCRIPT_DIR/install.sh"
else
  note "Packaging checks passed. Run the locally built 'cua-driver doctor' for owner, protocol, enumeration, activation, and portal/libei checks."
fi
exit "$status"
