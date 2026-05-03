#!/bin/bash

# Find wireshark binary
BINARY_PATH=$(which wireshark)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version - extract from first line "Wireshark X.Y.Z"
VERSION=$(wireshark --version 2>&1 | head -1 | sed 's/Wireshark //' | awk '{print $1}')

# Find desktop file
DESKTOP_FILE=$(find /usr/share/applications -name "*wireshark*" 2>/dev/null | head -1)

# Extract display name from desktop file
DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_FILE" 2>/dev/null | cut -d= -f2 | head -1)

# Find all app icons (not mimetypes)
ICON_PATHS=($(find /usr/share/icons -path "*/apps/*ireshark*" 2>/dev/null | sort -u))

# Build icon array for JSON
ICON_JSON="["
count=0
for icon in "${ICON_PATHS[@]}"; do
  if [ $count -gt 0 ]; then
    ICON_JSON+=","
  fi
  ICON_JSON+="\"$icon\""
  count=$((count+1))
done
ICON_JSON+="]"

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