#!/bin/bash

# Extract metadata for Visual Studio Code
# This script dynamically extracts and outputs JSON metadata from installed artifacts

# Find the code binary
BINARY_PATH=$(which code)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version using dpkg-query for reliability
VERSION=$(dpkg-query -W -f='${Version}' code 2>/dev/null | cut -d- -f1 || echo "unknown")

# Find the main desktop file (prefer code.desktop over code-url-handler.desktop)
DESKTOP_ENTRY=""
if [ -f "/usr/share/applications/code.desktop" ]; then
  DESKTOP_ENTRY="/usr/share/applications/code.desktop"
else
  # Search for any code desktop file, excluding url-handler
  for desktop_file in /usr/share/applications/code*.desktop; do
    if [ -f "$desktop_file" ] && [[ "$desktop_file" != *"url-handler"* ]]; then
      DESKTOP_ENTRY="$desktop_file"
      break
    fi
  done
  # If still not found, use the first one
  if [ -z "$DESKTOP_ENTRY" ]; then
    for desktop_file in /usr/share/applications/code*.desktop; do
      if [ -f "$desktop_file" ]; then
        DESKTOP_ENTRY="$desktop_file"
        break
      fi
    done
  fi
fi

# Extract display name from desktop file (primary source)
DISPLAY_NAME=""
if [ -f "$DESKTOP_ENTRY" ]; then
  # Extract Name field from desktop file
  DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | head -1 | cut -d= -f2-)
  # Trim whitespace
  DISPLAY_NAME=$(echo "$DISPLAY_NAME" | xargs)
fi

# If display name is still empty, try to get from dpkg
if [ -z "$DISPLAY_NAME" ]; then
  DISPLAY_NAME=$(dpkg-query -W -f='${Summary}' code 2>/dev/null || echo "")
fi

# Fallback to package description if still empty
if [ -z "$DISPLAY_NAME" ]; then
  DISPLAY_NAME=$(apt-cache show code 2>/dev/null | grep -i "^Description:" | cut -d: -f2- | head -1 | xargs)
fi

# Extract icon name from desktop file
ICON_NAME=""
if [ -f "$DESKTOP_ENTRY" ]; then
  ICON_NAME=$(grep "^Icon=" "$DESKTOP_ENTRY" | head -1 | cut -d= -f2)
  # Trim whitespace
  ICON_NAME=$(echo "$ICON_NAME" | xargs)
fi

# Find actual icon files dynamically based on discovered icon name
ICON_PATHS=()

# If we have an icon name, search for it in standard icon directories
if [ -n "$ICON_NAME" ]; then
  while IFS= read -r icon_file; do
    if [ -f "$icon_file" ]; then
      ICON_PATHS+=("$icon_file")
    fi
  done < <(find /usr/share/icons /usr/share/pixmaps -name "${ICON_NAME}" -o -name "${ICON_NAME}.*" 2>/dev/null)
fi

# Build JSON output
build_json() {
  local binary_path="$1"
  local binary_name="$2"
  local display_name="$3"
  local desktop_entry="$4"
  local version="$5"
  shift 5
  local icon_paths=("$@")
  
  # Build icon array JSON
  local icon_json="["
  local first=true
  for icon in "${icon_paths[@]}"; do
    if [ "$first" = true ]; then
      icon_json+="\"$icon\""
      first=false
    else
      icon_json+=",\"$icon\""
    fi
  done
  icon_json+="]"
  
  # Output compact JSON
  cat <<EOF
{"binary_path":"$binary_path","binary_name":"$binary_name","display_name":"$display_name","desktop_entry":"$desktop_entry","icon_paths":$icon_json,"version":"$version"}
EOF
}

build_json "$BINARY_PATH" "$BINARY_NAME" "$DISPLAY_NAME" "$DESKTOP_ENTRY" "$VERSION" "${ICON_PATHS[@]}"