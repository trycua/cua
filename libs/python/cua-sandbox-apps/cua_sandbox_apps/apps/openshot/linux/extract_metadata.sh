#!/bin/bash

# Extract metadata for OpenShot

# Find the binary path
BINARY_PATH=$(which openshot-qt)
if [ -z "$BINARY_PATH" ]; then
    BINARY_PATH=$(command -v openshot-qt)
fi

# Derive binary name from the binary path
BINARY_NAME=$(basename "$BINARY_PATH")

# Find desktop entry
DESKTOP_ENTRY=""
DESKTOP_CANDIDATES=(
    "/usr/share/applications/org.openshot.OpenShot.desktop"
    "/usr/share/applications/openshot-qt.desktop"
    "/usr/share/applications/openshot.desktop"
)

for candidate in "${DESKTOP_CANDIDATES[@]}"; do
    if [ -f "$candidate" ]; then
        DESKTOP_ENTRY="$candidate"
        break
    fi
done

if [ -z "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY=$(find /usr/share/applications -name "*openshot*" -type f 2>/dev/null | head -1)
fi

# Extract display_name from .desktop file's Name field
DISPLAY_NAME=""
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | cut -d'=' -f2- | head -1)
fi

# If still empty, fall back to trying package metadata
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME=$(apt-cache show openshot-qt 2>/dev/null | grep "^Description:" | cut -d' ' -f2- | head -1)
fi

# If still empty, use a reasonable default derived from binary name
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="OpenShot"
fi

# Get version from package manager (multiple fallback methods)
VERSION=""

# Try dpkg first (Debian/Ubuntu)
VERSION=$(dpkg -l 2>/dev/null | grep "openshot-qt" | awk '{print $3}' | head -1)

# If dpkg failed, try rpm (RedHat/Fedora)
if [ -z "$VERSION" ]; then
    VERSION=$(rpm -q openshot-qt 2>/dev/null | sed 's/openshot-qt-//' | head -1)
fi

# If still empty, try apt-cache show
if [ -z "$VERSION" ]; then
    VERSION=$(apt-cache show openshot-qt 2>/dev/null | grep "^Version:" | awk '{print $2}' | head -1)
fi

# If still empty, try the binary --version flag
if [ -z "$VERSION" ]; then
    VERSION=$("$BINARY_PATH" --version 2>&1 | head -1 | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?')
fi

# If still empty, leave it as empty string
if [ -z "$VERSION" ]; then
    VERSION=""
fi

# Find icon paths
ICON_PATHS=()

# Try to find from desktop entry
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    ICON_FROM_DESKTOP=$(grep "^Icon=" "$DESKTOP_ENTRY" | cut -d'=' -f2)
    if [ -n "$ICON_FROM_DESKTOP" ]; then
        # Try to find the actual icon file
        if [ -f "$ICON_FROM_DESKTOP" ]; then
            ICON_PATHS+=("$ICON_FROM_DESKTOP")
        else
            # Search in standard icon directories
            FOUND_ICON=$(find /usr/share/icons -name "${ICON_FROM_DESKTOP}*" -type f 2>/dev/null | head -1)
            if [ -n "$FOUND_ICON" ]; then
                ICON_PATHS+=("$FOUND_ICON")
            fi
        fi
    fi
fi

# Also search for openshot icon files directly
DIRECT_ICONS=$(find /usr/share/icons -name "*openshot*" -type f 2>/dev/null | head -3)
while IFS= read -r icon; do
    if [ -n "$icon" ] && [[ ! " ${ICON_PATHS[@]} " =~ " ${icon} " ]]; then
        ICON_PATHS+=("$icon")
    fi
done <<< "$DIRECT_ICONS"

# Check pixmaps
PIXMAP_ICONS=$(find /usr/share/pixmaps -name "*openshot*" -type f 2>/dev/null | head -2)
while IFS= read -r icon; do
    if [ -n "$icon" ] && [[ ! " ${ICON_PATHS[@]} " =~ " ${icon} " ]]; then
        ICON_PATHS+=("$icon")
    fi
done <<< "$PIXMAP_ICONS"

# Build JSON array for icon_paths
ICON_JSON="["
FIRST=true
for icon in "${ICON_PATHS[@]}"; do
    if [ "$FIRST" = true ]; then
        ICON_JSON="${ICON_JSON}\"${icon}\""
        FIRST=false
    else
        ICON_JSON="${ICON_JSON}, \"${icon}\""
    fi
done
ICON_JSON="${ICON_JSON}]"

# Output JSON with proper escaping
cat << EOF
{
  "binary_path": "${BINARY_PATH}",
  "binary_name": "${BINARY_NAME}",
  "display_name": "${DISPLAY_NAME}",
  "desktop_entry": "${DESKTOP_ENTRY}",
  "icon_paths": ${ICON_JSON},
  "version": "${VERSION}"
}
EOF