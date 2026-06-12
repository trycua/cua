#!/bin/bash

# Extract Puppet metadata for installation verification

# Find the puppet binary
PUPPET_BIN=$(which puppet)
BINARY_NAME=$(basename "$PUPPET_BIN")

# Get version
VERSION=$(puppet --version 2>/dev/null | head -1)

# Get display name from package metadata
DISPLAY_NAME=$(dpkg -s puppet 2>/dev/null | grep "^Description:" | sed 's/^Description: //' | head -1)
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="Puppet"
fi

# Find .desktop file
DESKTOP_FILE=$(find /usr/share/applications -name "*puppet*" -o -name "*hiera*" 2>/dev/null | head -1)
if [ -z "$DESKTOP_FILE" ]; then
    DESKTOP_FILE="null"
fi

# Find icon files
ICON_PATHS=()

# Search in standard icon directories
for ICON_DIR in /usr/share/icons /usr/share/pixmaps ~/.local/share/icons; do
    if [ -d "$ICON_DIR" ]; then
        # Look for puppet-related icons
        ICONS=$(find "$ICON_DIR" -type f \( -name "*puppet*" -o -name "*hiera*" \) 2>/dev/null)
        while IFS= read -r ICON; do
            if [ -n "$ICON" ] && [ -f "$ICON" ]; then
                ICON_PATHS+=("$ICON")
            fi
        done <<< "$ICONS"
    fi
done

# If no puppet-specific icons found, use a generic admin icon
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    # Look for generic config/admin icons
    for GENERIC_ICON in /usr/share/pixmaps/application-x-executable.png /usr/share/pixmaps/system-software-install.svg; do
        if [ -f "$GENERIC_ICON" ]; then
            ICON_PATHS+=("$GENERIC_ICON")
        fi
    done
fi

# Build JSON output manually
echo "{"
echo "  \"binary_path\": \"$PUPPET_BIN\","
echo "  \"binary_name\": \"$BINARY_NAME\","
echo "  \"display_name\": \"$DISPLAY_NAME\","
echo "  \"desktop_entry\": $DESKTOP_FILE,"
echo "  \"icon_paths\": ["

# Add icon paths as JSON array elements
for i in "${!ICON_PATHS[@]}"; do
    if [ $i -lt $((${#ICON_PATHS[@]} - 1)) ]; then
        echo "    \"${ICON_PATHS[$i]}\","
    else
        echo "    \"${ICON_PATHS[$i]}\""
    fi
done

echo "  ],"
echo "  \"version\": \"$VERSION\""
echo "}"