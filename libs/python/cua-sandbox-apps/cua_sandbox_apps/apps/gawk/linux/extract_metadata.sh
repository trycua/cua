#!/bin/bash
# Extract metadata for GNU Awk

# Find binary path
BINARY_PATH=$(which gawk)
BINARY_NAME="gawk"

# Get version - extract from version output
# Pattern: "GNU Awk X.Y.Z" - capture the first version number after "Awk"
VERSION=$(gawk --version 2>/dev/null | head -n1 | sed -n 's/.*Awk \([0-9][0-9.]*\).*/\1/p')

# Get display name from package manager
DISPLAY_NAME="GNU Awk"

# Search for desktop entries dynamically
DESKTOP_ENTRY=""
if [ -d "/usr/share/applications" ]; then
  for desktop_file in /usr/share/applications/*.desktop; do
    if [ -f "$desktop_file" ] && (grep -q "Exec=.*\bgawk\b" "$desktop_file" 2>/dev/null || grep -q "^Name=.*[Gg]awk" "$desktop_file" 2>/dev/null); then
      DESKTOP_ENTRY="$desktop_file"
      break
    fi
  done
fi

# Try to parse display name from desktop file if available
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
  PARSED_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | cut -d= -f2- | head -1)
  if [ -n "$PARSED_NAME" ]; then
    DISPLAY_NAME="$PARSED_NAME"
  fi
fi

# Search for icon paths dynamically
declare -a ICON_PATHS_ARRAY

# Check package-provided files for icons
if command -v dpkg &>/dev/null && dpkg -l | grep -q "^ii.*gawk"; then
  while IFS= read -r icon_file; do
    if [ -f "$icon_file" ]; then
      ICON_PATHS_ARRAY+=("$icon_file")
    fi
  done < <(dpkg -L gawk 2>/dev/null | grep -E '\.(png|svg|ico|xpm)$')
fi

# Check standard icon directories for gawk/awk related icons
for icon_dir in /usr/share/pixmaps /usr/share/icons; do
  if [ -d "$icon_dir" ]; then
    # Look for any gawk or awk related icon files
    while IFS= read -r icon_file; do
      if [ -f "$icon_file" ]; then
        ICON_PATHS_ARRAY+=("$icon_file")
      fi
    done < <(find "$icon_dir" -maxdepth 3 \( -iname "*gawk*" -o -iname "*awk*" \) \( -name "*.png" -o -name "*.svg" -o -name "*.ico" -o -name "*.xpm" \) 2>/dev/null)
  fi
done

# Parse icon from desktop file if available
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
  ICON_VALUE=$(grep "^Icon=" "$DESKTOP_ENTRY" | cut -d= -f2- | head -1)
  if [ -n "$ICON_VALUE" ]; then
    # Search for this icon in standard paths
    for icon_dir in /usr/share/pixmaps /usr/share/icons; do
      if [ -d "$icon_dir" ]; then
        while IFS= read -r icon_file; do
          if [ -f "$icon_file" ]; then
            ICON_PATHS_ARRAY+=("$icon_file")
          fi
        done < <(find "$icon_dir" -maxdepth 3 -name "${ICON_VALUE}*" \( -name "*.png" -o -name "*.svg" -o -name "*.ico" -o -name "*.xpm" \) 2>/dev/null)
      fi
    done
  fi
fi

# Remove duplicates
ICON_PATHS_ARRAY=($(printf '%s\n' "${ICON_PATHS_ARRAY[@]}" | sort -u))

# Format icon paths as JSON array
ICON_JSON="["
first=true
for icon in "${ICON_PATHS_ARRAY[@]}"; do
  if [ "$first" = true ]; then
    first=false
  else
    ICON_JSON="$ICON_JSON, "
  fi
  # Escape backslashes and quotes in path
  escaped_icon=$(printf '%s\n' "$icon" | sed 's/\\/\\\\/g; s/"/\\"/g')
  ICON_JSON="$ICON_JSON\"$escaped_icon\""
done
ICON_JSON="$ICON_JSON]"

# Output JSON with proper escaping
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $( [ -z "$DESKTOP_ENTRY" ] && echo "null" || echo "\"$DESKTOP_ENTRY\"" ),
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF