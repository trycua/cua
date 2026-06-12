#!/bin/bash

# Find Docker binary path
BINARY_PATH=$(which docker || command -v docker)
if [ -z "$BINARY_PATH" ]; then
    echo '{"error": "Docker binary not found"}' >&2
    exit 1
fi

# Derive binary name from the full path
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version from the binary
VERSION=$(docker --version 2>/dev/null | grep -oP 'version \K[0-9]+\.[0-9]+\.[0-9]+' | head -1)

# Initialize arrays and variables
DISPLAY_NAME=""
DESKTOP_ENTRY=""
ICON_PATHS=()

# Try to find and parse .desktop files to extract display name
if [ -d "/usr/share/applications" ]; then
    DESKTOP_FILES=$(find /usr/share/applications -name "*docker*" -type f 2>/dev/null)
    for desktop_file in $DESKTOP_FILES; do
        if grep -q "^Name=" "$desktop_file" 2>/dev/null; then
            # Extract Name field from desktop file
            NAME_FROM_DESKTOP=$(grep "^Name=" "$desktop_file" | head -1 | sed 's/^Name=//')
            if [ -n "$NAME_FROM_DESKTOP" ]; then
                DISPLAY_NAME="$NAME_FROM_DESKTOP"
                DESKTOP_ENTRY="$desktop_file"
                break
            fi
        fi
    done
fi

# Fallback to capitalize binary name if no desktop file found
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME=$(echo "$BINARY_NAME" | sed 's/^./\U&/')
fi

# Use package manager to find all installed files for docker
if command -v dpkg &>/dev/null; then
    # For Debian/Ubuntu systems
    PKG_FILES=$(dpkg -L docker-ce 2>/dev/null || dpkg -L docker 2>/dev/null || true)
    
    # Search for .desktop files in package files
    DESKTOP_FROM_PKG=$(echo "$PKG_FILES" | grep "\.desktop$" | head -1)
    if [ -n "$DESKTOP_FROM_PKG" ] && [ -f "$DESKTOP_FROM_PKG" ]; then
        DESKTOP_ENTRY="$DESKTOP_FROM_PKG"
        # Extract name from the desktop file if we didn't get it before
        if [ -z "$DISPLAY_NAME" ] || [ "$DISPLAY_NAME" = "Docker" ]; then
            PARSED_NAME=$(grep "^Name=" "$DESKTOP_FROM_PKG" 2>/dev/null | head -1 | sed 's/^Name=//')
            if [ -n "$PARSED_NAME" ]; then
                DISPLAY_NAME="$PARSED_NAME"
            fi
        fi
    fi
    
    # Find icons from package files
    ICON_FILES=$(echo "$PKG_FILES" | grep -E "\.(png|svg|ico)$" | grep -i docker)
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done <<< "$ICON_FILES"
elif command -v rpm &>/dev/null; then
    # For RedHat/CentOS systems
    PKG_FILES=$(rpm -ql docker 2>/dev/null || rpm -ql docker-ce 2>/dev/null || true)
    
    DESKTOP_FROM_PKG=$(echo "$PKG_FILES" | grep "\.desktop$" | head -1)
    if [ -n "$DESKTOP_FROM_PKG" ] && [ -f "$DESKTOP_FROM_PKG" ]; then
        DESKTOP_ENTRY="$DESKTOP_FROM_PKG"
        if [ -z "$DISPLAY_NAME" ] || [ "$DISPLAY_NAME" = "Docker" ]; then
            PARSED_NAME=$(grep "^Name=" "$DESKTOP_FROM_PKG" 2>/dev/null | head -1 | sed 's/^Name=//')
            if [ -n "$PARSED_NAME" ]; then
                DISPLAY_NAME="$PARSED_NAME"
            fi
        fi
    fi
    
    ICON_FILES=$(echo "$PKG_FILES" | grep -E "\.(png|svg|ico)$" | grep -i docker)
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done <<< "$ICON_FILES"
fi

# Also search standard icon directories
if [ -d "/usr/share/icons" ]; then
    ICON_SEARCH=$(find /usr/share/icons -type f \( -name "*docker*" -o -iname "*docker*" \) 2>/dev/null | head -10)
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            # Avoid duplicates
            if ! printf '%s\n' "${ICON_PATHS[@]}" | grep -q "^$icon_file$"; then
                ICON_PATHS+=("$icon_file")
            fi
        fi
    done <<< "$ICON_SEARCH"
fi

if [ -d "/usr/share/pixmaps" ]; then
    ICON_SEARCH=$(find /usr/share/pixmaps -type f \( -name "*docker*" -o -iname "*docker*" \) 2>/dev/null | head -10)
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            if ! printf '%s\n' "${ICON_PATHS[@]}" | grep -q "^$icon_file$"; then
                ICON_PATHS+=("$icon_file")
            fi
        fi
    done <<< "$ICON_SEARCH"
fi

# Set default display name if still not found
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="Docker"
fi

# Build desktop entry JSON
if [ -n "$DESKTOP_ENTRY" ]; then
    DESKTOP_JSON="\"$DESKTOP_ENTRY\""
else
    DESKTOP_JSON="null"
fi

# Build icon paths JSON array
ICON_JSON="["
for i in "${!ICON_PATHS[@]}"; do
    if [ $i -gt 0 ]; then
        ICON_JSON="${ICON_JSON},"
    fi
    ICON_JSON="${ICON_JSON}\"${ICON_PATHS[$i]}\""
done
ICON_JSON="${ICON_JSON}]"

# Output JSON
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_JSON,
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF
