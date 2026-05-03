#!/bin/bash
# Extract metadata for Nmap

# Find the binary path
BINARY_PATH=$(which nmap)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version
VERSION=$($BINARY_PATH --version 2>&1 | head -1 | grep -oP 'Nmap \K[0-9]+\.[0-9]+' || echo "7.80")

# Find desktop entry
DESKTOP_ENTRY=$(find /usr/share/applications -name "*nmap*" -type f 2>/dev/null | head -1 || echo "")

# Get display name from desktop entry if available, otherwise use default
DISPLAY_NAME="Nmap"
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
  EXTRACTED_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | head -1 | cut -d'=' -f2)
  if [ -n "$EXTRACTED_NAME" ]; then
    DISPLAY_NAME="$EXTRACTED_NAME"
  fi
fi

# Find icon paths
ICON_PATHS=()
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
  # Try to extract icon from .desktop file
  ICON_FROM_DESKTOP=$(grep "Icon=" "$DESKTOP_ENTRY" | head -1 | cut -d'=' -f2)
  if [ -n "$ICON_FROM_DESKTOP" ]; then
    # Try standard icon locations
    if [ -f "/usr/share/icons/hicolor/256x256/apps/$ICON_FROM_DESKTOP.png" ]; then
      ICON_PATHS+=("/usr/share/icons/hicolor/256x256/apps/$ICON_FROM_DESKTOP.png")
    fi
    if [ -f "/usr/share/icons/hicolor/128x128/apps/$ICON_FROM_DESKTOP.png" ]; then
      ICON_PATHS+=("/usr/share/icons/hicolor/128x128/apps/$ICON_FROM_DESKTOP.png")
    fi
    if [ -f "/usr/share/pixmaps/$ICON_FROM_DESKTOP.png" ]; then
      ICON_PATHS+=("/usr/share/pixmaps/$ICON_FROM_DESKTOP.png")
    fi
  fi
fi

# Look for common icon locations
for ICON_DIR in /usr/share/pixmaps /usr/share/icons/hicolor/*/apps; do
  if [ -d "$ICON_DIR" ]; then
    FOUND=$(find "$ICON_DIR" -name "*nmap*" -type f 2>/dev/null | head -1)
    if [ -n "$FOUND" ]; then
      ICON_PATHS+=("$FOUND")
    fi
  fi
done

# Remove duplicates
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))

# Format icon paths as JSON array
ICON_JSON="["
for i in "${!ICON_PATHS[@]}"; do
  if [ $i -gt 0 ]; then
    ICON_JSON="$ICON_JSON,"
  fi
  ICON_JSON="$ICON_JSON\"${ICON_PATHS[$i]}\""
done
ICON_JSON="$ICON_JSON]"

# Output JSON metadata
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $( [ -n "$DESKTOP_ENTRY" ] && echo "\"$DESKTOP_ENTRY\"" || echo "null" ),
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF