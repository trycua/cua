#!/bin/bash

# Extract metadata for Vim installation

# Find the main Vim binary
BINARY_PATH=$(command -v gvim 2>/dev/null || command -v vim 2>/dev/null)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get full version information from vim --version
VERSION_LINE=$(vim --version 2>/dev/null | head -1)
# Extract version number - looks for pattern like "8.2.3995" or "8.2"
VERSION=$(echo "$VERSION_LINE" | grep -oE "[0-9]+\.[0-9]+(\.[0-9]+)?")

# Find .desktop files - search for vim-related .desktop files
DESKTOP_ENTRY=""
for desk_file in /usr/share/applications/{gvim,vim}.desktop; do
    if [ -f "$desk_file" ]; then
        DESKTOP_ENTRY="$desk_file"
        break
    fi
done

# If still not found, search more broadly
if [ -z "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY=$(find /usr/share/applications -name "*vim*.desktop" -type f 2>/dev/null | head -1)
fi

# Extract display name from .desktop file
DISPLAY_NAME="Vim"
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    DISPLAY_NAME=$(grep -E "^Name=" "$DESKTOP_ENTRY" 2>/dev/null | head -1 | cut -d'=' -f2)
    if [ -z "$DISPLAY_NAME" ]; then
        DISPLAY_NAME="Vim"
    fi
fi

# Find icon files - use dpkg to get all files from packages, then filter for icons
ICON_PATHS=()
for pkg in vim-gtk3 vim gvim; do
    if dpkg -l 2>/dev/null | grep -q "^ii.*$pkg"; then
        # For each file in the package, check if it matches icon patterns
        while IFS= read -r file; do
            if [ -f "$file" ] && [[ "$file" =~ \.(png|svg|ico)$ ]]; then
                # Check if this is a vim icon
                if [[ "$file" =~ (vim|gvim) ]]; then
                    ICON_PATHS+=("$file")
                fi
            fi
        done < <(dpkg -L "$pkg" 2>/dev/null)
    fi
done

# Also search standard icon directories
while IFS= read -r icon; do
    ICON_PATHS+=("$icon")
done < <(find /usr/share/icons /usr/share/pixmaps -name "*vim*" -o -name "*gvim*" 2>/dev/null | sort -u)

# Remove duplicates from ICON_PATHS
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
    ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))
fi

# Build JSON output
DESKTOP_JSON="null"
if [ -n "$DESKTOP_ENTRY" ]; then
    DESKTOP_JSON="\"$DESKTOP_ENTRY\""
fi

ICON_JSON="[]"
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
    ICON_JSON="["
    for i in "${!ICON_PATHS[@]}"; do
        ICON_JSON+="\"${ICON_PATHS[$i]}\""
        if [ $i -lt $((${#ICON_PATHS[@]} - 1)) ]; then
            ICON_JSON+=", "
        fi
    done
    ICON_JSON+="]"
fi

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