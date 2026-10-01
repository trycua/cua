#!/bin/bash
# Extract Elasticsearch metadata

# Dynamically find the Elasticsearch binary via package manager
BINARY_PATH=""

# Try to find via dpkg first (most reliable for Debian/Ubuntu)
if command -v dpkg &>/dev/null; then
  BINARY_PATH=$(dpkg -L elasticsearch 2>/dev/null | grep -E "bin/elasticsearch$" | head -1)
fi

# If not found via dpkg, try rpm
if [ -z "$BINARY_PATH" ] && command -v rpm &>/dev/null; then
  BINARY_PATH=$(rpm -ql elasticsearch 2>/dev/null | grep -E "bin/elasticsearch$" | head -1)
fi

# Try to find via 'which' as last resort
if [ -z "$BINARY_PATH" ]; then
  BINARY_PATH=$(command -v elasticsearch 2>/dev/null)
fi

# Fallback to standard path
if [ -z "$BINARY_PATH" ]; then
  BINARY_PATH="/usr/share/elasticsearch/bin/elasticsearch"
fi

# Get binary name from the path
BINARY_NAME=$(basename "$BINARY_PATH")

# Extract version from package metadata using dpkg
VERSION=""
if command -v dpkg &>/dev/null; then
  VERSION=$(dpkg -s elasticsearch 2>/dev/null | grep "^Version:" | head -1 | awk '{print $2}' | cut -d'-' -f1)
fi

# If no version from dpkg, try rpm
if [ -z "$VERSION" ] && command -v rpm &>/dev/null; then
  VERSION=$(rpm -qi elasticsearch 2>/dev/null | grep "^Version" | head -1 | awk '{print $3}')
fi

# If still no version, try running the binary
if [ -z "$VERSION" ] && [ -x "$BINARY_PATH" ]; then
  VERSION=$(sudo -n "$BINARY_PATH" --version 2>/dev/null | grep -oP 'Version: \K[^,]+')
fi

# If we still don't have a version, return empty string (not hardcoded)
[ -z "$VERSION" ] && VERSION=""

# Extract display name from package metadata
DISPLAY_NAME=""

# Try to get from dpkg description
if command -v dpkg &>/dev/null; then
  DISPLAY_NAME=$(dpkg -s elasticsearch 2>/dev/null | grep "^Description:" | head -1 | sed 's/^Description: //')
fi

# If no display name from dpkg, try rpm
if [ -z "$DISPLAY_NAME" ] && command -v rpm &>/dev/null; then
  DISPLAY_NAME=$(rpm -qi elasticsearch 2>/dev/null | grep "^Summary" | head -1 | sed 's/^Summary.*: //')
fi

# If still no display name, try to extract from desktop entry if it exists
if [ -z "$DISPLAY_NAME" ]; then
  DESKTOP_FILE=$(find /usr/share/applications -name "*elasticsearch*" 2>/dev/null | head -1)
  if [ -f "$DESKTOP_FILE" ]; then
    DISPLAY_NAME=$(grep -E "^Name=" "$DESKTOP_FILE" 2>/dev/null | head -1 | cut -d'=' -f2-)
  fi
fi

# Find desktop entry dynamically
DESKTOP_ENTRY=""
for desktop_file in $(find /usr/share/applications -name "*elasticsearch*" 2>/dev/null); do
  if [ -f "$desktop_file" ]; then
    DESKTOP_ENTRY="$desktop_file"
    break
  fi
done

# Find icon paths dynamically
declare -a ICON_PATHS

# First, query dpkg for all files in the elasticsearch package
if command -v dpkg &>/dev/null; then
  while IFS= read -r file_path; do
    if [ -f "$file_path" ] && [[ "$file_path" =~ \.(png|svg|ico|xpm)$ ]]; then
      ICON_PATHS+=("$file_path")
    fi
  done < <(dpkg -L elasticsearch 2>/dev/null | grep -iE "\.(png|svg|ico|xpm)$")
fi

# Second, try to extract icon path from desktop entry
if [ -f "$DESKTOP_ENTRY" ]; then
  icon_name=$(grep -E "^Icon=" "$DESKTOP_ENTRY" 2>/dev/null | cut -d'=' -f2- | head -1)
  if [ ! -z "$icon_name" ]; then
    # Search for the icon in standard locations
    found_icon=$(find /usr/share/icons /usr/share/pixmaps -name "${icon_name}*" 2>/dev/null | head -1)
    [ ! -z "$found_icon" ] && ICON_PATHS+=("$found_icon")
  fi
fi

# Third, search for elasticsearch-related icons in standard locations
while IFS= read -r icon_path; do
  if [ ! -z "$icon_path" ]; then
    ICON_PATHS+=("$icon_path")
  fi
done < <(find /usr/share/icons /usr/share/pixmaps -iname "*elasticsearch*" 2>/dev/null | head -10)

# Remove duplicates and filter out empty strings
declare -a UNIQUE_ICONS
declare -A seen
for icon in "${ICON_PATHS[@]}"; do
  if [ ! -z "$icon" ] && [ ${seen["$icon"]:-0} -eq 0 ] && [ -f "$icon" ]; then
    seen["$icon"]=1
    UNIQUE_ICONS+=("$icon")
  fi
done

# Build JSON array for icons
ICON_JSON="[]"
if [ ${#UNIQUE_ICONS[@]} -gt 0 ]; then
  ICON_JSON=$(printf '%s\n' "${UNIQUE_ICONS[@]}" | jq -R -s -c 'split("\n") | map(select(length > 0))')
fi

# Build the JSON output
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $([ -z "$DESKTOP_ENTRY" ] && echo "null" || echo "\"$DESKTOP_ENTRY\""),
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF
