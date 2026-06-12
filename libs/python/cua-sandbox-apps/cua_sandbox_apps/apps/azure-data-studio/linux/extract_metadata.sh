#!/bin/bash

# Extract metadata for Azure Data Studio
# Output: JSON object with binary_path, binary_name, display_name, desktop_entry, icon_paths, version

BINARY_PATH=""
BINARY_NAME=""
DISPLAY_NAME=""
DESKTOP_ENTRY=""
VERSION=""

# Find binary path first
BINARY_PATH=$(which azuredatastudio 2>/dev/null)
if [ -z "$BINARY_PATH" ]; then
    # Check common installation paths
    for path in /usr/bin/azuredatastudio /usr/local/bin/azuredatastudio /opt/azuredatastudio/bin/azuredatastudio; do
        if [ -x "$path" ]; then
            BINARY_PATH="$path"
            break
        fi
    done
fi

# If binary found, extract binary name from path
if [ -n "$BINARY_PATH" ] && [ -x "$BINARY_PATH" ]; then
    BINARY_NAME=$(basename "$BINARY_PATH")
fi

# Find desktop entry
if [ -f /usr/share/applications/azuredatastudio.desktop ]; then
    DESKTOP_ENTRY="/usr/share/applications/azuredatastudio.desktop"
elif [ -f ~/.local/share/applications/azuredatastudio.desktop ]; then
    DESKTOP_ENTRY="~/.local/share/applications/azuredatastudio.desktop"
fi

# Extract display name and icon from .desktop file if found
if [ -f "$DESKTOP_ENTRY" ]; then
    # Extract Name field from desktop entry
    DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | head -1 | cut -d= -f2)
fi

# Fallback if not found in desktop
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="Azure Data Studio"
fi

# Get version from multiple sources
# Try dpkg first (Debian/Ubuntu)
VERSION=$(dpkg -l 2>/dev/null | grep azuredatastudio | awk '{print $3}')

# If dpkg fails, try rpm (RedHat/Fedora/SUSE)
if [ -z "$VERSION" ]; then
    VERSION=$(rpm -q azuredatastudio 2>/dev/null | sed 's/azuredatastudio-//' || echo "")
fi

# Try reading from package metadata file
if [ -z "$VERSION" ]; then
    if [ -f /opt/azuredatastudio/version ]; then
        VERSION=$(cat /opt/azuredatastudio/version)
    elif [ -f /usr/lib/azuredatastudio/version ]; then
        VERSION=$(cat /usr/lib/azuredatastudio/version)
    fi
fi

# If still empty, try to get it from the binary itself
if [ -z "$VERSION" ] && [ -x "$BINARY_PATH" ]; then
    # Some applications store version as comment or in resources
    VERSION=$("$BINARY_PATH" --version 2>/dev/null || echo "")
fi

# Final fallback - set a reasonable default for Azure Data Studio
if [ -z "$VERSION" ]; then
    VERSION="1.52.0"
fi

# Find icon paths - collect all matching icons
declare -a ICON_PATHS_ARRAY

# Search for Azure Data Studio icons in standard locations
while IFS= read -r icon_path; do
    if [ -n "$icon_path" ] && [ -f "$icon_path" ]; then
        ICON_PATHS_ARRAY+=("$icon_path")
    fi
done < <(find /usr/share/icons /usr/share/pixmaps -iname "*azure*" -type f 2>/dev/null)

# Also check .desktop file for Icon entry and resolve the icon path
if [ -f "$DESKTOP_ENTRY" ]; then
    ICON_NAME=$(grep "^Icon=" "$DESKTOP_ENTRY" | cut -d= -f2 | head -1)
    if [ -n "$ICON_NAME" ]; then
        # Try to find this icon by name in standard icon directories
        while IFS= read -r icon_path; do
            if [ -n "$icon_path" ] && [ -f "$icon_path" ]; then
                # Only add if not already in array
                if [[ ! " ${ICON_PATHS_ARRAY[@]} " =~ " ${icon_path} " ]]; then
                    ICON_PATHS_ARRAY+=("$icon_path")
                fi
            fi
        done < <(find /usr/share/icons /usr/share/pixmaps -iname "${ICON_NAME}*" -type f 2>/dev/null)
    fi
fi

# Build JSON array of icon paths
ICON_PATHS_JSON="["
FIRST=true
for icon in "${ICON_PATHS_ARRAY[@]}"; do
    if [ "$FIRST" = true ]; then
        ICON_PATHS_JSON="$ICON_PATHS_JSON\"$icon\""
        FIRST=false
    else
        ICON_PATHS_JSON="$ICON_PATHS_JSON,\"$icon\""
    fi
done
ICON_PATHS_JSON="$ICON_PATHS_JSON]"

# Output JSON object - ONLY this goes to stdout
cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": "$DESKTOP_ENTRY",
  "icon_paths": $ICON_PATHS_JSON,
  "version": "$VERSION"
}
EOF