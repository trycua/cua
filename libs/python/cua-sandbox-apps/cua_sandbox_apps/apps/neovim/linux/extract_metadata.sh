#!/bin/bash
set -e

# Find the nvim binary
BINARY_PATH=$(which nvim)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get version
VERSION=$(nvim --version | head -1 | awk '{print $2}')

# Find .desktop file
DESKTOP_ENTRY=$(find /usr/share/applications -name "*neovim*" -o -name "*nvim*" 2>/dev/null | head -1 || echo "")

# Extract display_name from .desktop file if it exists
DISPLAY_NAME="Neovim"
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
  DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | cut -d= -f2- | head -1 || echo "Neovim")
fi

# Find and validate icon files
ICON_PATHS=()

# Check for icons in standard locations
if [ -d /usr/share/icons ]; then
  while IFS= read -r icon_path; do
    if [ -f "$icon_path" ]; then
      ICON_PATHS+=("$icon_path")
    fi
  done < <(find /usr/share/icons -type f \( -name "*neovim*" -o -name "*nvim*" \) 2>/dev/null | head -5)
fi

# Check /usr/share/pixmaps
if [ -d /usr/share/pixmaps ]; then
  while IFS= read -r icon_path; do
    if [ -f "$icon_path" ]; then
      ICON_PATHS+=("$icon_path")
    fi
  done < <(find /usr/share/pixmaps -type f \( -name "*neovim*" -o -name "*nvim*" \) 2>/dev/null | head -5)
fi

# Also check for common vim/editor icons that might be used
if [ -d /usr/share/pixmaps ]; then
  while IFS= read -r icon_path; do
    if [ -f "$icon_path" ]; then
      ICON_PATHS+=("$icon_path")
    fi
  done < <(find /usr/share/pixmaps -type f -name "*vim*" 2>/dev/null | head -3)
fi

# Validate desktop entry exists
DESKTOP_JSON=""
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
  DESKTOP_JSON="\"$DESKTOP_ENTRY\""
else
  DESKTOP_JSON="null"
fi

# Format icon paths as JSON array
ICON_JSON="["
for icon in "${ICON_PATHS[@]}"; do
  if [ -n "$icon" ] && [ -f "$icon" ]; then
    ICON_JSON="$ICON_JSON\"$icon\","
  fi
done
# Remove trailing comma and close bracket
ICON_JSON="${ICON_JSON%,}]"

# Output JSON with all metadata
cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_JSON,
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF
