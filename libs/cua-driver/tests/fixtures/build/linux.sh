#!/usr/bin/env bash
# Stage Linux-runnable test fixture apps into libs/cua-driver/rust/test-apps/harness-*/.
#
# GTK3 is PyGObject: a real GTK3 widget tree (identical AT-SPI exposure to a C
# app), so there's no compile step. Electron and Tauri are shared cross-platform
# harnesses built from apps/cross-platform/.
#
# Runtime deps (NOT installed here): python3-gi, gir1.2-gtk-3.0, at-spi2-core,
# Node.js/npm for Electron, Rust + WebKitGTK build deps for Tauri.
#
# Usage:
#   ./linux.sh
#   ./linux.sh --skip gtk3       # skip one target (gtk3|electron|tauri)
#   ./linux.sh --only electron,gtk3
#   ./linux.sh --clean
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_APPS_DIR="$(cd "$HARNESS_DIR/../../rust/test-apps" && pwd)"
STAGE="$TEST_APPS_DIR/harness-gtk3"
SRC="$HARNESS_DIR/apps/linux/gtk3"
SKIP="none"
ONLY=",gtk3,electron,tauri,"
CLEAN=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip)
            SKIP="${2:-}"
            shift 2
            ;;
        --clean)
            CLEAN=1
            shift
            ;;
        --only)
            ONLY=",${2:-},"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--skip gtk3|electron|tauri] [--only comma-separated-targets] [--clean]" >&2
            exit 2
            ;;
    esac
done

if [[ "$CLEAN" == "1" ]]; then
    rm -rf "$TEST_APPS_DIR/harness-gtk3" "$TEST_APPS_DIR/harness-electron" "$TEST_APPS_DIR/harness-tauri"
fi

if [[ "$SKIP" != "gtk3" && "$ONLY" == *",gtk3,"* ]]; then
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

    echo "==> Staged GTK3 harness -> $STAGE/CuaTestHarness.Gtk3"
    if python3 -c "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk" 2>/dev/null; then
        echo "==> PyGObject/GTK3 present"
    else
        echo "WARNING: PyGObject/GTK3 not importable - install python3-gi gir1.2-gtk-3.0 at-spi2-core" >&2
    fi
fi

if [[ "$SKIP" != "electron" && "$ONLY" == *",electron,"* ]]; then
    "$HARNESS_DIR/apps/cross-platform/electron/build.sh"
fi

if [[ "$SKIP" != "tauri" && "$ONLY" == *",tauri,"* ]]; then
    "$HARNESS_DIR/apps/cross-platform/tauri/build.sh"
fi
