#!/bin/bash

# Find the SQLite binary
BINARY_PATH=$(which sqlite3)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get the version
VERSION_OUTPUT=$(sqlite3 --version 2>&1)
VERSION=$(echo "$VERSION_OUTPUT" | awk '{print $1}' | sed 's/^[^0-9]*//')
# Validate version format - if invalid format, extract what we can
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+ ]]; then
    VERSION=$(echo "$VERSION_OUTPUT" | grep -oE '^[0-9]+\.[0-9]+' | head -n1)
fi

# Extract display_name from .desktop file or use package metadata
DISPLAY_NAME=""
DESKTOP_FILE=""
ICON_FROM_DESKTOP=""

# Find .desktop file in package contents
DESKTOP_FILE=$(dpkg -L sqlite3 2>/dev/null | grep '\.desktop$' | head -n1)
if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
    # Extract display name from .desktop file
    DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_FILE" | head -n1 | cut -d'=' -f2- | head -c 100)
    # Extract icon reference from .desktop file
    ICON_FROM_DESKTOP=$(grep "^Icon=" "$DESKTOP_FILE" | head -n1 | cut -d'=' -f2-)
fi

# If no display name found, try package description
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME=$(dpkg -s sqlite3 2>/dev/null | grep "^Description:" | cut -d' ' -f2-)
fi

# Find icon files
ICON_PATHS=()

# If we have an icon reference from .desktop file, resolve it using icon theme
if [ -n "$ICON_FROM_DESKTOP" ]; then
    # Use find to search for icon files matching the icon name
    for ext in .png .svg .ico .jpg; do
        while IFS= read -r file; do
            if [ -f "$file" ]; then
                ICON_PATHS+=("$file")
            fi
        done < <(find /usr/share/icons /usr/share/pixmaps -name "*$ICON_FROM_DESKTOP*$ext" -type f 2>/dev/null | head -n 5)
    done
fi

# Search for sqlite-specific icons in system icon directories
for search_term in "sqlite" "database" "sql"; do
    for ext in .png .svg .ico .jpg; do
        while IFS= read -r file; do
            if [ -f "$file" ]; then
                ICON_PATHS+=("$file")
            fi
        done < <(find /usr/share/icons /usr/share/pixmaps -iname "*${search_term}*${ext}" -type f 2>/dev/null | head -n 3)
    done
done

# Remove duplicates, sort, and limit to reasonable number
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u | head -n 20))

# Build icon path array JSON
ICON_JSON="["
for i in "${!ICON_PATHS[@]}"; do
    if [ $i -gt 0 ]; then
        ICON_JSON="$ICON_JSON,"
    fi
    # Escape backslashes and quotes
    ESCAPED_PATH="${ICON_PATHS[$i]//\\/\\\\}"
    ESCAPED_PATH="${ESCAPED_PATH//\"/\\\"}"
    ICON_JSON="$ICON_JSON\"$ESCAPED_PATH\""
done
ICON_JSON="$ICON_JSON]"

# Create the metadata JSON object
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $( [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ] && echo \"\"$DESKTOP_FILE\"\" || echo null ),
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF