#!/bin/bash

# Extract Godot Engine metadata

# Get binary path
BINARY_PATH="/opt/godot/godot"
if [ ! -f "$BINARY_PATH" ]; then
    BINARY_PATH=$(which godot 2>/dev/null)
    if [ -z "$BINARY_PATH" ]; then
        BINARY_PATH=""
    fi
fi

# Get version from binary
VERSION=""
if [ -n "$BINARY_PATH" ] && [ -x "$BINARY_PATH" ]; then
    VERSION=$("$BINARY_PATH" --version 2>/dev/null | head -1)
fi

# Initialize display name and binary name from defaults
DISPLAY_NAME="Godot Engine"
BINARY_NAME="godot"

# Try to extract from desktop files
DESKTOP_ENTRY=""
DESKTOP_FILES=(
    "/usr/share/applications/godot.desktop"
    "/usr/share/applications/org.godotengine.Godot.desktop"
    "/usr/share/applications/Godot*.desktop"
)

for desktop_file in "${DESKTOP_FILES[@]}"; do
    for expanded_file in $desktop_file; do
        if [ -f "$expanded_file" ]; then
            DESKTOP_ENTRY="$expanded_file"
            # Extract Name= field (the display name)
            DISPLAY_NAME=$(grep "^Name=" "$expanded_file" | head -1 | cut -d'=' -f2-)
            if [ -z "$DISPLAY_NAME" ]; then
                DISPLAY_NAME="Godot Engine"
            fi
            # Extract Exec= field to get the binary name
            EXEC_PATH=$(grep "^Exec=" "$expanded_file" | head -1 | cut -d'=' -f2- | awk '{print $1}')
            if [ -n "$EXEC_PATH" ]; then
                BINARY_NAME=$(basename "$EXEC_PATH")
            fi
            break 2
        fi
    done
done

# Find icon paths - search in standard locations
ICON_PATHS=()
ICON_FILES=$(find /usr/share/pixmaps /usr/share/icons -name "*godot*" -type f 2>/dev/null | head -5)
if [ -n "$ICON_FILES" ]; then
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done <<< "$ICON_FILES"
fi

# Also check if desktop file specifies an icon
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    ICON_NAME=$(grep "^Icon=" "$DESKTOP_ENTRY" | head -1 | cut -d'=' -f2-)
    if [ -n "$ICON_NAME" ]; then
        # Try to find the icon by name
        FOUND_ICON=$(find /usr/share/icons /usr/share/pixmaps -name "${ICON_NAME}*" -type f 2>/dev/null | head -1)
        if [ -n "$FOUND_ICON" ] && [ -f "$FOUND_ICON" ]; then
            ICON_PATHS+=("$FOUND_ICON")
        fi
    fi
fi

# Remove duplicates from icon paths
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))

# Format icon paths as JSON array
ICON_JSON="["
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
    for ((i = 0; i < ${#ICON_PATHS[@]}; i++)); do
        if [ $i -gt 0 ]; then
            ICON_JSON="$ICON_JSON, "
        fi
        ICON_JSON="$ICON_JSON\"${ICON_PATHS[$i]}\""
    done
fi
ICON_JSON="$ICON_JSON]"

# Create JSON output
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $([ -n "$DESKTOP_ENTRY" ] && echo "\"$DESKTOP_ENTRY\"" || echo "null"),
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF