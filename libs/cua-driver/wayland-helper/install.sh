#!/usr/bin/env bash
# Install the cua WinRects GNOME Shell extension — supplies window screen
# geometry (for AT-SPI coordinate reconstruction) and renders the agent cursor
# on GNOME Mutter Wayland, where a normal client can do neither. Best-effort:
# cua-driver works without it (just no screen coords / no cursor on Mutter).
set -euo pipefail
UUID="winrects@cua"
SRC="$(cd "$(dirname "$0")" && pwd)/$UUID"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"
mkdir -p "$DEST"
cp -f "$SRC/metadata.json" "$SRC/extension.js" "$DEST/"
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
echo "Installed $UUID to $DEST."
echo "GNOME Shell scans extensions only at startup, so log out/in (or restart the"
echo "session) ONCE to load it. After that: gnome-extensions info $UUID should show State: ACTIVE."
