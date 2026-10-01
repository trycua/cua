#!/bin/bash
# Extract metadata for GNU Emacs

# Find the binary
BINARY_PATH=$(which emacs)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get the version
VERSION=$($BINARY_PATH --version 2>&1 | head -1 | grep -oP 'GNU Emacs \K[0-9.]+' || echo "unknown")

# Find desktop file
DESKTOP_FILE=$(find /usr/share/applications -name "*emacs*.desktop" 2>/dev/null | head -1)

# Extract display name from desktop file or package metadata
DISPLAY_NAME=""
if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
    DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_FILE" | head -1 | cut -d'=' -f2)
fi

# Fallback to package description if not found in desktop file
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME=$(dpkg -L emacs 2>/dev/null | xargs -I {} cat {} 2>/dev/null | grep -i "GNU Emacs" | head -1 || echo "GNU Emacs")
    # If still empty, use default
    if [ -z "$DISPLAY_NAME" ]; then
        DISPLAY_NAME="GNU Emacs"
    fi
fi

# Find icon files
ICON_PATHS=()
# Check common icon locations
for dir in /usr/share/icons /usr/share/pixmaps; do
    if [ -d "$dir" ]; then
        while IFS= read -r icon; do
            ICON_PATHS+=("$icon")
        done < <(find "$dir" -name "*emacs*" \( -name "*.png" -o -name "*.svg" -o -name "*.xpm" \) 2>/dev/null)
    fi
done

# Also check desktop file for icon
if [ -n "$DESKTOP_FILE" ]; then
    ICON_NAME=$(grep "^Icon=" "$DESKTOP_FILE" | cut -d'=' -f2)
    if [ -n "$ICON_NAME" ]; then
        # Try to find the icon file
        FOUND_ICON=$(find /usr/share/icons /usr/share/pixmaps -name "${ICON_NAME}*" 2>/dev/null | head -1)
        if [ -n "$FOUND_ICON" ]; then
            ICON_PATHS+=("$FOUND_ICON")
        fi
    fi
fi

# Remove duplicates from ICON_PATHS
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))

# Format icon paths as JSON array
ICON_JSON="["
for icon in "${ICON_PATHS[@]}"; do
    ICON_JSON="$ICON_JSON\"$icon\","
done
ICON_JSON="${ICON_JSON%,}]"

# Output JSON
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": "$DESKTOP_FILE",
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF