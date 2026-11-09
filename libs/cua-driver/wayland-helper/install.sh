#!/usr/bin/env bash
# Install the cua WinRects GNOME Shell extension — supplies window screen
# geometry (for AT-SPI coordinate reconstruction) and renders the agent cursor
# on GNOME Mutter Wayland, where a normal client can do neither. Best-effort:
# cua-driver works without it (just no screen coords / no cursor on Mutter).
#
# Other apps bundle the same helper (same UUID, shared additive API versions),
# so a newer installed copy is kept. Pass --force to install this one anyway.
set -euo pipefail
UUID="winrects@cua"
SRC="$(cd "$(dirname "$0")" && pwd)/$UUID"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"
FORCE=0
if [ "${1:-}" = "--force" ]; then FORCE=1; fi
metadata_version() {
  sed -n 's/.*"version"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$1" 2>/dev/null | head -n 1 || true
}
BUNDLED=$(metadata_version "$SRC/metadata.json")
INSTALLED=$(metadata_version "$DEST/metadata.json")
if [ "$FORCE" != 1 ] && [ "${INSTALLED:-0}" -gt "${BUNDLED:-0}" ]; then
  echo "Keeping the installed $UUID v$INSTALLED: it is newer than this bundle's v${BUNDLED:-?}."
  echo "Its API includes everything this cua-driver uses. Pass --force to replace it."
else
  mkdir -p "$DEST"
  cp -f "$SRC/metadata.json" "$SRC/extension.js" "$DEST/"
  echo "Installed $UUID v${BUNDLED:-?} to $DEST."
fi
# Add to the enabled set (preserves existing).
cur=$(gsettings get org.gnome.shell enabled-extensions 2>/dev/null || echo "@as []")
python3 - "$cur" "$UUID" <<'PY'
import sys, ast
try: l = ast.literal_eval(sys.argv[1])
except Exception: l = []
if sys.argv[2] not in l: l.append(sys.argv[2])
import subprocess
subprocess.run(["gsettings","set","org.gnome.shell","enabled-extensions",str(l)])
print("enabled-extensions ->", l)
PY
echo "GNOME Shell scans extensions only at startup, so log out/in (or restart the"
echo "session) ONCE to load it. After that: gnome-extensions info $UUID should show State: ACTIVE."
