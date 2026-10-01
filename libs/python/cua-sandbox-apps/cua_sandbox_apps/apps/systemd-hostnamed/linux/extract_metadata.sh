#!/bin/bash

# Extract metadata for systemd-hostnamed

# Find binary using which and command -v
BINARY_PATH=$(which systemd-hostnamed 2>/dev/null || command -v systemd-hostnamed 2>/dev/null || echo "/lib/systemd/systemd-hostnamed")

# If not found in PATH, check standard systemd locations
if [ ! -f "$BINARY_PATH" ]; then
    for path in /lib/systemd/systemd-hostnamed /usr/lib/systemd/systemd-hostnamed /usr/libexec/systemd-hostnamed; do
        if [ -f "$path" ]; then
            BINARY_PATH="$path"
            break
        fi
    done
fi

# Extract binary name from the path
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version from binary or package
VERSION=""
if [ -f "$BINARY_PATH" ]; then
    VERSION=$($BINARY_PATH --version 2>/dev/null | head -1 | awk '{print $2}')
fi

# If version not found, try dpkg
if [ -z "$VERSION" ]; then
    VERSION=$(dpkg -l 2>/dev/null | grep systemd | head -1 | awk '{print $3}')
fi

# Get desktop entry path using dpkg or find
DESKTOP_ENTRY=""
if command -v dpkg &>/dev/null; then
    DESKTOP_ENTRY=$(dpkg -L systemd 2>/dev/null | grep -E "systemd-hostnamed\.desktop|hostname.*\.desktop" | head -1)
fi

if [ -z "$DESKTOP_ENTRY" ]; then
    for path in /usr/share/applications/systemd-hostnamed.desktop /usr/share/applications/dbus-org.freedesktop.hostname1.service.desktop; do
        if [ -f "$path" ]; then
            DESKTOP_ENTRY="$path"
            break
        fi
    done
fi

# Extract display name from desktop entry if it exists
DISPLAY_NAME=$(basename "$BINARY_PATH")
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    # Try to get Name from desktop file
    EXTRACTED_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" 2>/dev/null | cut -d'=' -f2- | head -1)
    if [ -n "$EXTRACTED_NAME" ]; then
        DISPLAY_NAME="$EXTRACTED_NAME"
    fi
fi

# Find icon paths using dpkg or standard locations
ICON_PATHS=()

# Get all icons from systemd package
if command -v dpkg &>/dev/null; then
    while IFS= read -r icon_path; do
        if [ -f "$icon_path" ]; then
            ICON_PATHS+=("$icon_path")
        fi
    done < <(dpkg -L systemd 2>/dev/null | grep -E "\.(png|svg|ico)$" | head -5)
fi

# If no icons found, search in standard locations
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    for dir in /usr/share/icons /usr/share/pixmaps; do
        if [ -d "$dir" ]; then
            while IFS= read -r icon; do
                ICON_PATHS+=("$icon")
            done < <(find "$dir" -type f \( -name "*system*" -o -name "*host*" -o -name "*debian*" \) \( -name "*.png" -o -name "*.svg" -o -name "*.ico" \) 2>/dev/null | head -3)
            if [ ${#ICON_PATHS[@]} -gt 0 ]; then
                break
            fi
        fi
    done
fi

# Build icon_paths JSON array
ICON_JSON="["
first=1
for icon in "${ICON_PATHS[@]}"; do
    if [ "$first" -eq 0 ]; then
        ICON_JSON="$ICON_JSON,"
    fi
    ICON_JSON="$ICON_JSON\"$icon\""
    first=0
done
ICON_JSON="$ICON_JSON]"

# Output JSON
cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $([ -z "$DESKTOP_ENTRY" ] && echo "null" || echo "\"$DESKTOP_ENTRY\""),
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF