#!/bin/bash

# Extract binary path using which or command -v
BINARY_PATH=$(which gitlab-ctl 2>/dev/null || command -v gitlab-ctl 2>/dev/null)
if [ -z "$BINARY_PATH" ] || [ ! -x "$BINARY_PATH" ]; then
    # Fall back to checking the actual binary locations from dpkg
    BINARY_PATH=$(dpkg -L gitlab-ce 2>/dev/null | grep '/bin/gitlab-ctl$' | head -1)
fi
# Ensure we have an absolute path
if [ ! -x "$BINARY_PATH" ]; then
    BINARY_PATH=$(find / -name gitlab-ctl -type f -executable 2>/dev/null | head -1)
fi

# Extract binary name
BINARY_NAME=$(basename "$BINARY_PATH" 2>/dev/null || echo "gitlab-ctl")

# Extract display name from package metadata
DISPLAY_NAME=$(dpkg -s gitlab-ce 2>/dev/null | grep "^Description:" | sed 's/^Description: //' || echo "GitLab Community Edition")

# Extract desktop entry
DESKTOP_ENTRY=$(find /usr/share/applications -name "*gitlab*" -type f 2>/dev/null | head -1)
if [ -z "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY=null
else
    DESKTOP_ENTRY="\"$DESKTOP_ENTRY\""
fi

# Extract icon paths - search for actual icon files
ICON_PATHS=()

# Search in dpkg package files
if command -v dpkg-query &>/dev/null; then
    ICON_PATHS+=($(dpkg -L gitlab-ce 2>/dev/null | grep -E '\.(png|svg|ico|jpg)$' | sort -u | head -5))
fi

# Search in standard icon locations
ICON_PATHS+=($(find /usr/share/icons -name "*gitlab*" -type f 2>/dev/null | head -3))
ICON_PATHS+=($(find /opt/gitlab -name "*.png" -o -name "*.svg" -o -name "*.ico" -type f 2>/dev/null | head -3))

# Extract version from package manager
VERSION=$(dpkg-query -W -f='${Version}' gitlab-ce 2>/dev/null)
if [ -z "$VERSION" ]; then
    # Try alternate method
    VERSION=$(dpkg -l 2>/dev/null | grep '^ii.*gitlab-ce' | awk '{print $3}')
fi
if [ -z "$VERSION" ]; then
    VERSION="unknown"
fi

# Format icon paths as JSON array
ICON_JSON="["
first=true
unique_icons=$(printf '%s\n' "${ICON_PATHS[@]}" | sort -u | grep -v '^$')
while IFS= read -r icon; do
    if [ -n "$icon" ] && [ -f "$icon" ]; then
        if [ "$first" = true ]; then
            ICON_JSON="$ICON_JSON\"$icon\""
            first=false
        else
            ICON_JSON="$ICON_JSON, \"$icon\""
        fi
    fi
done <<< "$unique_icons"
ICON_JSON="$ICON_JSON]"

# Create JSON output
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY,
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF
