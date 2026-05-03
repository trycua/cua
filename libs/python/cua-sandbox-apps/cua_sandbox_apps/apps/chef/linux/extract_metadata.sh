#!/bin/bash
# Extract Chef metadata for app installation verification

# Find the chef-cli binary
BINARY_PATH=$(/usr/bin/which chef-cli 2>/dev/null || echo "/usr/local/bin/chef-cli")

# Extract binary name from the full path (last component)
BINARY_NAME=$(basename "$BINARY_PATH")

# Try to find .desktop file for Chef and extract metadata from it
DESKTOP_FILE=""
DISPLAY_NAME=""
DESKTOP_ICON=""

# Search for chef-related desktop files
for desktop_path in /usr/share/applications /usr/local/share/applications ~/.local/share/applications; do
  if [ -d "$desktop_path" ]; then
    for desktop in "$desktop_path"/*chef*.desktop; do
      if [ -f "$desktop" ]; then
        DESKTOP_FILE="$desktop"
        # Extract Name= field from desktop file (localized entries first)
        DISPLAY_NAME=$(grep -E "^Name(\[en\])?=" "$desktop" | head -1 | cut -d'=' -f2- | head -1)
        # Extract Icon= field from desktop file
        DESKTOP_ICON=$(grep "^Icon=" "$desktop" | cut -d'=' -f2- | head -1)
        break 2
      fi
    done
  fi
done

# Get version from the binary
VERSION=$($BINARY_PATH --version 2>&1 | grep -oP 'version: \K[0-9.]+' || $BINARY_PATH -v 2>&1 | grep -oP '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "unknown")

# Find icon files using multiple strategies
ICON_ARRAY=()

# Strategy 1: If we found an Icon= field in .desktop file, try to resolve it
if [ -n "$DESKTOP_ICON" ]; then
  # If it's an absolute path
  if [ -f "$DESKTOP_ICON" ]; then
    ICON_ARRAY+=("$DESKTOP_ICON")
  else
    # Try to find it in icon directories
    for icon_dir in /usr/share/icons /usr/local/share/icons ~/.local/share/icons; do
      if [ -d "$icon_dir" ]; then
        icon_found=$(find "$icon_dir" -type f -name "${DESKTOP_ICON}*" 2>/dev/null | head -1)
        if [ -n "$icon_found" ]; then
          ICON_ARRAY+=("$icon_found")
          break
        fi
      fi
    done
  fi
fi

# Strategy 2: Query package manager for icon files associated with chef package
if [ ${#ICON_ARRAY[@]} -eq 0 ] && command -v dpkg &>/dev/null; then
  # Try to find icons using dpkg for chef-cli or chef package
  for pkg_name in chef-cli chef chef-workstation; do
    pkg_icons=$(dpkg -L "$pkg_name" 2>/dev/null | grep -E '\.(png|svg|ico)$' | head -3)
    if [ -n "$pkg_icons" ]; then
      while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
          ICON_ARRAY+=("$icon_file")
        fi
      done <<< "$pkg_icons"
      [ ${#ICON_ARRAY[@]} -gt 0 ] && break
    fi
  done
fi

# Strategy 3: Search for chef-related icons in standard icon directories
if [ ${#ICON_ARRAY[@]} -eq 0 ]; then
  for icon_dir in /usr/share/icons /usr/local/share/icons ~/.local/share/icons; do
    if [ -d "$icon_dir" ]; then
      icon_found=$(find "$icon_dir" -type f -name "*chef*" 2>/dev/null | head -1)
      if [ -n "$icon_found" ]; then
        ICON_ARRAY+=("$icon_found")
        break
      fi
    fi
  done
fi

# If DISPLAY_NAME is still empty, derive it from binary name
if [ -z "$DISPLAY_NAME" ]; then
  DISPLAY_NAME=$(echo "$BINARY_NAME" | sed 's/-/ /g' | sed 's/^./\U&/g')
fi

# Format icon paths as JSON array
if [ ${#ICON_ARRAY[@]} -gt 0 ]; then
  ICON_JSON=$(printf '"%%s"' "${ICON_ARRAY[@]}" | paste -sd ',' | sed 's/^/[/; s/,$/]/')
else
  ICON_JSON="[]"
fi

# Output JSON
cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": "$DESKTOP_FILE",
  "icon_paths": $ICON_JSON,
  "version": "$VERSION"
}
EOF