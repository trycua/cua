#!/bin/bash

# Find the redis-server binary
BINARY_PATH=$(which redis-server)
BINARY_NAME=$(basename "$BINARY_PATH")
DISPLAY_NAME="Redis"

# Get the version - extract just the version number
VERSION=$(redis-server --version | grep -oP 'v=\K[\d.]+' || echo "6.0.16")

# Try to find display name from .desktop files
if [ -f /usr/share/applications/redis-server.desktop ]; then
    DISPLAY_NAME=$(grep "^Name=" /usr/share/applications/redis-server.desktop | cut -d= -f2)
    DESKTOP_ENTRY="/usr/share/applications/redis-server.desktop"
else
    DESKTOP_ENTRY=""
fi

# Find icon files using dpkg for redis-server package
ICON_PATHS_ARRAY=()

# Try to find icons from the installed package using dpkg
if command -v dpkg &> /dev/null; then
    while IFS= read -r icon_path; do
        if [ -f "$icon_path" ] && [[ "$icon_path" =~ \.(png|svg|ico|jpg)$ ]]; then
            ICON_PATHS_ARRAY+=("$icon_path")
        fi
    done < <(dpkg -L redis-server 2>/dev/null | grep -E "\.(png|svg|ico|jpg)$" || true)
fi

# Search for generic database/server icons in standard locations
if [ ${#ICON_PATHS_ARRAY[@]} -eq 0 ]; then
    # Look in /usr/share/icons for database-related icons
    for icon_dir in /usr/share/icons /usr/share/pixmaps; do
        if [ -d "$icon_dir" ]; then
            while IFS= read -r icon_file; do
                if [ -f "$icon_file" ]; then
                    ICON_PATHS_ARRAY+=("$icon_file")
                fi
            done < <(find "$icon_dir" -type f \( -name "*database*" -o -name "*redis*" -o -name "*server*" \) 2>/dev/null | head -5)
        fi
    done
fi

# If still no icons, look for any database or server themed icons
if [ ${#ICON_PATHS_ARRAY[@]} -eq 0 ]; then
    for icon_dir in /usr/share/icons /usr/share/pixmaps; do
        if [ -d "$icon_dir" ]; then
            ICON=$(find "$icon_dir" -type f -name "*server*" 2>/dev/null | head -1)
            if [ -n "$ICON" ]; then
                ICON_PATHS_ARRAY+=("$ICON")
                break
            fi
        fi
    done
fi

# Convert array to JSON format
ICON_PATHS_JSON="["
for i in "${!ICON_PATHS_ARRAY[@]}"; do
    ICON_PATHS_JSON+="\"${ICON_PATHS_ARRAY[$i]}\""
    if [ $i -lt $((${#ICON_PATHS_ARRAY[@]} - 1)) ]; then
        ICON_PATHS_JSON+=","
    fi
done
ICON_PATHS_JSON+="]"

# Output JSON metadata
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": "$DESKTOP_ENTRY",
  "icon_paths": $ICON_PATHS_JSON,
  "version": "$VERSION"
}
EOF