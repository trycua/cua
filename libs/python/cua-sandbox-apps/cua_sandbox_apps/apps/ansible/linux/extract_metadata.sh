#!/bin/bash
set -e

# Find the ansible binary
BINARY_PATH=$(which ansible)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version
VERSION=$(ansible --version 2>/dev/null | head -1 | awk '{print $2}')

# Extract display_name and icon from package metadata first
DISPLAY_NAME=""
ICON_PATHS=()

# Try to get display name from apt/dpkg package metadata
if command -v apt-cache &>/dev/null; then
    # Get the short description from package
    PKG_DESC=$(apt-cache show ansible 2>/dev/null | grep "^Description:" | cut -d':' -f2- | sed 's/^ *//g' | head -1 || echo "")
    if [ ! -z "$PKG_DESC" ]; then
        DISPLAY_NAME="$PKG_DESC"
    fi
fi

# If we have a display_name, extract icon paths from package files
if [ ! -z "$DISPLAY_NAME" ]; then
    # Try to find icon files within the package
    while IFS= read -r icon_file; do
        if [ ! -z "$icon_file" ]; then
            ICON_PATHS+=("\"$icon_file\"")
        fi
    done < <(dpkg -L ansible 2>/dev/null | grep -E \"\\.(png|svg|ico)$\" | head -5 || true)
fi

# Find desktop file
DESKTOP_ENTRY=$(find /usr/share/applications -name "*ansible*" 2>/dev/null | head -1 || echo "")

# If desktop file exists, prefer its name and icons
if [ ! -z "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    EXTRACTED_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" 2>/dev/null | cut -d'=' -f2 | head -1 || echo "")
    if [ ! -z "$EXTRACTED_NAME" ]; then
        DISPLAY_NAME="$EXTRACTED_NAME"
    fi
    
    # Try to extract icon from desktop file
    ICON_NAME=$(grep "^Icon=" "$DESKTOP_ENTRY" 2>/dev/null | cut -d'=' -f2 | head -1 || echo "")
    if [ ! -z "$ICON_NAME" ]; then
        ICON_PATHS=()
        ICON_PATHS+=("\"$ICON_NAME\"")
    fi
fi

# If no icon found from desktop or package, search system icon directories
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    # First try ansible-specific icons
    for icon_dir in /usr/share/icons /usr/share/pixmaps; do
        if [ -d "$icon_dir" ]; then
            while IFS= read -r icon_file; do
                if [ ! -z "$icon_file" ]; then
                    ICON_PATHS+=("\"$icon_file\"")
                fi
            done < <(find "$icon_dir" -name "*ansible*" 2>/dev/null | head -5)
        fi
    done
    
    # If still no icon, try terminal icon as fallback
    if [ ${#ICON_PATHS[@]} -eq 0 ]; then
        TERMINAL_ICON=$(find /usr/share/icons -name "*terminal*" -type f 2>/dev/null | head -1 || echo "")
        if [ ! -z "$TERMINAL_ICON" ]; then
            ICON_PATHS+=("\"$TERMINAL_ICON\"")
        fi
    fi
fi

# Build icon array JSON
ICON_JSON="["
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
    ICON_JSON+=$(IFS=,; echo "${ICON_PATHS[*]}")
fi
ICON_JSON+="]"

# Format desktop_entry
if [ -z "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY_JSON="null"
else
    DESKTOP_ENTRY_JSON="\"$DESKTOP_ENTRY\""
fi

# Format display_name (ensure it's not empty)
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="ansible"
fi

# Output JSON metadata
cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY_JSON,
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF