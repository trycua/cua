#!/bin/bash

# Find Git binary path
BINARY_PATH=$(which git)
BINARY_NAME=$(basename "$BINARY_PATH")

# Get Git version
VERSION=$($BINARY_PATH --version | awk '{print $3}')

# Extract display name from package metadata
DISPLAY_NAME=$(dpkg -s git 2>/dev/null | grep -E '^Description:' | sed 's/^Description: //' | head -1)
if [ -z "$DISPLAY_NAME" ]; then
  DISPLAY_NAME="Git"
fi

# Find .desktop file
DESKTOP_FILE=$(find /usr/share/applications -name "*git*" -type f 2>/dev/null | head -1)

# Find icon files from package data or filesystem
ICON_PATHS=()

# Try to find icons in /usr/share/icons
for icon_dir in /usr/share/icons /usr/share/pixmaps /usr/share/app-install/icons; do
  if [ -d "$icon_dir" ]; then
    icons=$(find "$icon_dir" -name "*git*" \( -type f -o -type l \) 2>/dev/null | head -5)
    if [ ! -z "$icons" ]; then
      while IFS= read -r icon; do
        if [ ! -z "$icon" ]; then
          ICON_PATHS+=("$icon")
        fi
      done <<< "$icons"
    fi
  fi
done

# If no git-specific icons, try generic vcs/scm icons
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
  for icon_dir in /usr/share/icons /usr/share/pixmaps; do
    if [ -d "$icon_dir" ]; then
      icons=$(find "$icon_dir" -type f \( -name "*vcs*" -o -name "*scm*" -o -name "*version*" \) 2>/dev/null | head -3)
      if [ ! -z "$icons" ]; then
        while IFS= read -r icon; do
          if [ ! -z "$icon" ]; then
            ICON_PATHS+=("$icon")
          fi
        done <<< "$icons"
      fi
    fi
  done
fi

# Build JSON output
echo "{"
echo "  \"binary_path\": \"$BINARY_PATH\","
echo "  \"binary_name\": \"$BINARY_NAME\","
echo "  \"display_name\": \"$DISPLAY_NAME\","
echo "  \"desktop_entry\": $([ -z \"$DESKTOP_FILE\" ] && echo 'null' || echo \"\"$DESKTOP_FILE\")","
echo "  \"icon_paths\": ["
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
  for i in \"${!ICON_PATHS[@]}\"; do
    if [ $i -lt $((${#ICON_PATHS[@]} - 1)) ]; then
      echo "    \"${ICON_PATHS[$i]}\","
    else
      echo "    \"${ICON_PATHS[$i]}\""
    fi
  done
fi
echo "  ],"
echo "  \"version\": \"$VERSION\""
echo "}"