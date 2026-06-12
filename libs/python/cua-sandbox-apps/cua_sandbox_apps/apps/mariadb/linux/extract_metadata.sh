#!/bin/bash

# Find the MariaDB binary
BINARY_PATH=$(which mysqld)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version
VERSION=$(mysqld --version | awk '{for(i=1;i<=NF;i++) if($i ~ /[0-9]+\.[0-9]+/) {print $i; exit}}')

# Find display name from package metadata
DISPLAY_NAME=""
if command -v dpkg &>/dev/null; then
    # Use dpkg to get package description
    DISPLAY_NAME=$(dpkg -s mariadb-server 2>/dev/null | grep "^Description:" | cut -d: -f2- | xargs | head -c 100)
fi

# Fallback to default if not found
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="MariaDB"
fi

# Find desktop entry - use package manager to discover it
DESKTOP_ENTRY=""
if command -v dpkg &>/dev/null; then
    # Query package files to find desktop entries
    DESKTOP_ENTRY=$(dpkg -L mariadb-server 2>/dev/null | grep -E "\.desktop$" | head -1)
fi

# If still not found, try common locations
if [ -z "$DESKTOP_ENTRY" ]; then
    for path in /usr/share/applications/mariadb*.desktop /usr/share/applications/mysql*.desktop; do
        if [ -f "$path" ]; then
            DESKTOP_ENTRY="$path"
            break
        fi
    done
fi

# Extract display name from desktop file if available
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    DESKTOP_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" 2>/dev/null | head -1 | cut -d= -f2)
    if [ -n "$DESKTOP_NAME" ]; then
        DISPLAY_NAME="$DESKTOP_NAME"
    fi
fi

# Find icon paths using package manager
ICON_PATHS=()
if command -v dpkg &>/dev/null; then
    # Find all icon files provided by the package
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done < <(dpkg -L mariadb-server 2>/dev/null | grep -E "\.(png|svg|ico|pixmap)$" | head -10)
fi

# If no icons from package, search standard directories
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    for icon_dir in /usr/share/icons /usr/share/pixmaps; do
        if [ -d "$icon_dir" ]; then
            # Look for mariadb or mysql icons
            while IFS= read -r icon_file; do
                ICON_PATHS+=("$icon_file")
            done < <(find "$icon_dir" -type f \( -name "*mariadb*" -o -name "*mysql*" \) 2>/dev/null | head -5)
        fi
    done
fi

# Try to extract icon from desktop file
if [ ${#ICON_PATHS[@]} -eq 0 ] && [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    ICON_NAME=$(grep "^Icon=" "$DESKTOP_ENTRY" | cut -d= -f2)
    if [ -n "$ICON_NAME" ]; then
        # Try to locate the icon
        ICON_FILE=$(find /usr/share/icons /usr/share/pixmaps -name "$ICON_NAME*" 2>/dev/null | head -1)
        if [ -n "$ICON_FILE" ]; then
            ICON_PATHS+=("$ICON_FILE")
        fi
    fi
fi

# Convert icon array to JSON
ICONS_JSON="["
for ((i=0; i<${#ICON_PATHS[@]}; i++)); do
    ICONS_JSON+="\"${ICON_PATHS[$i]}\""
    if [ $i -lt $((${#ICON_PATHS[@]} - 1)) ]; then
        ICONS_JSON+=","
    fi
done
ICONS_JSON+="]"

# Output JSON metadata
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $([ -n "$DESKTOP_ENTRY" ] && echo "\"$DESKTOP_ENTRY\"" || echo "null"),
  "icon_paths": $ICONS_JSON,
  "version": "$VERSION"
}
EOF