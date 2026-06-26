#!/usr/bin/env bash
# Stage the Linux GTK3 test-harness app into ../../rust/test-apps/harness-gtk3/ —
# the Linux peer of macos.sh (AppKit/SwiftUI) and windows.ps1 (WPF/WinUI3).
#
# The app is PyGObject (GTK3): a real GTK3 widget tree (identical AT-SPI exposure
# to a C app), so there's no compile step — we stage main.py plus an executable
# launcher named CuaTestHarness.Gtk3 (matching scenarios.json's exe_relative_path).
#
# Runtime deps (NOT installed here): python3-gi, gir1.2-gtk-3.0, at-spi2-core.
#   apt-get install -y python3-gi gir1.2-gtk-3.0 at-spi2-core
#
# Usage: ./linux.sh [--clean]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
STAGE="$(cd "$HARNESS_DIR/../rust/test-apps" && pwd)/harness-gtk3"
SRC="$HARNESS_DIR/apps/linux/gtk3"

rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$SRC/main.py" "$STAGE/main.py"

cat > "$STAGE/CuaTestHarness.Gtk3" <<'LAUNCHER'
#!/usr/bin/env bash
# Force the X11 backend so the window is enumerable via cua-driver's X11
# list_windows (_NET_CLIENT_LIST) — under a Wayland session this routes through
# Xwayland; on a pure-X11 session it's a no-op. AT-SPI works over D-Bus either way.
export GDK_BACKEND=x11
exec python3 "$(dirname "$(readlink -f "$0")")/main.py" "$@"
LAUNCHER
chmod +x "$STAGE/CuaTestHarness.Gtk3"

echo "==> Staged GTK3 harness → $STAGE/CuaTestHarness.Gtk3"
if python3 -c "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk" 2>/dev/null; then
    echo "==> PyGObject/GTK3 present"
else
    echo "WARNING: PyGObject/GTK3 not importable — install python3-gi gir1.2-gtk-3.0 at-spi2-core" >&2
fi
