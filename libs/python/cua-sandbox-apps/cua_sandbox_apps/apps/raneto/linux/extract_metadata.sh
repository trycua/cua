#!/bin/bash

# Extract metadata for Raneto

# Dynamically find the Raneto installation directory
INSTALL_DIR=""

# Try various methods to find the installation
# Method 1: Check if globally installed via npm
if command -v raneto &>/dev/null; then
  RANETO_CMD=$(command -v raneto)
  INSTALL_DIR=$(npm list -g raneto 2>/dev/null | grep raneto | head -1 | sed 's/.*at //' | cut -d' ' -f1)
fi

# Method 2: Check in standard npm global locations
if [ -z "$INSTALL_DIR" ] || [ ! -d "$INSTALL_DIR" ]; then
  NPM_PREFIX=$(npm config get prefix 2>/dev/null)
  if [ -d "$NPM_PREFIX/lib/node_modules/raneto" ]; then
    INSTALL_DIR="$NPM_PREFIX/lib/node_modules/raneto"
  fi
fi

# Method 3: Check home directory (local installation)
if [ -z "$INSTALL_DIR" ] || [ ! -d "$INSTALL_DIR" ]; then
  if [ -d "$HOME/raneto" ]; then
    INSTALL_DIR="$HOME/raneto"
  fi
fi

# Method 4: Check common installation paths
if [ -z "$INSTALL_DIR" ] || [ ! -d "$INSTALL_DIR" ]; then
  for path in /opt/raneto /usr/local/raneto /srv/raneto; do
    if [ -d "$path" ]; then
      INSTALL_DIR="$path"
      break
    fi
  done
fi

# If still not found, check for package manager installations
if [ -z "$INSTALL_DIR" ] || [ ! -d "$INSTALL_DIR" ]; then
  if command -v dpkg &>/dev/null; then
    INSTALL_DIR=$(dpkg -L raneto 2>/dev/null | grep -E "package\.json|server\.js" | head -1 | xargs dirname)
  elif command -v rpm &>/dev/null; then
    INSTALL_DIR=$(rpm -ql raneto 2>/dev/null | grep -E "package\.json|server\.js" | head -1 | xargs dirname)
  fi
fi

# Set a default if nothing found
if [ -z "$INSTALL_DIR" ] || [ ! -d "$INSTALL_DIR" ]; then
  INSTALL_DIR="$HOME/raneto"
fi

# Find the actual binary path
BINARY_PATH=""

# Check for executable entry points in order of preference
if [ -x "$INSTALL_DIR/server.js" ] || [ -f "$INSTALL_DIR/server.js" ]; then
  BINARY_PATH="$INSTALL_DIR/server.js"
elif [ -f "$INSTALL_DIR/app/index.js" ]; then
  BINARY_PATH="$INSTALL_DIR/app/index.js"
elif [ -x "$(command -v raneto 2>/dev/null)" ]; then
  BINARY_PATH="$(command -v raneto)"
else
  BINARY_PATH="$INSTALL_DIR"
fi

# Extract metadata from package.json dynamically
BINARY_NAME=""
DISPLAY_NAME=""
RANETO_VERSION=""

if [ -f "$INSTALL_DIR/package.json" ]; then
  # Extract name field
  BINARY_NAME=$(grep '"name"' "$INSTALL_DIR/package.json" | head -1 | sed 's/.*"name": "//' | sed 's/".*//' | sed 's/@.*\///')
  
  # Extract description (use as display name) or fallback to name
  DISPLAY_NAME=$(grep '"description"' "$INSTALL_DIR/package.json" | head -1 | sed 's/.*"description": "//' | sed 's/".*//')
  if [ -z "$DISPLAY_NAME" ]; then
    # If no description, capitalize the first letter of the name
    DISPLAY_NAME=$(echo "$BINARY_NAME" | sed 's/^./\U&/')
  fi
  
  # Extract version
  RANETO_VERSION=$(grep '"version"' "$INSTALL_DIR/package.json" | head -1 | sed 's/.*"version": "//' | sed 's/".*//')
fi

# Find icon paths - specifically looking for application icons in standard locations
declare -a ICON_PATHS=()

# Look for favicon and logo files specifically
if [ -d "$INSTALL_DIR" ]; then
  # Search for known icon files used by Raneto
  while IFS= read -r icon_file; do
    if [ -f "$icon_file" ]; then
      # Only include files that are likely application icons (favicons, logos)
      if [[ "$icon_file" =~ (favicon|logo|icon) ]]; then
        ICON_PATHS+=("$icon_file")
      fi
    fi
  done < <(find "$INSTALL_DIR" -type f \( -name "favicon*" -o -name "logo*" -o -name "*icon*.png" -o -name "*icon*.ico" \) 2>/dev/null | head -10)
fi

# Look for .desktop file
DESKTOP_ENTRY="null"

# Check package manager for desktop entry
if command -v dpkg &>/dev/null; then
  DESKTOP_FILE=$(dpkg -L raneto 2>/dev/null | grep "\.desktop$" | head -1)
  if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
    DESKTOP_ENTRY="\"$DESKTOP_FILE\""
  fi
elif command -v rpm &>/dev/null; then
  DESKTOP_FILE=$(rpm -ql raneto 2>/dev/null | grep "\.desktop$" | head -1)
  if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
    DESKTOP_ENTRY="\"$DESKTOP_FILE\""
  fi
fi

# Fallback: search in standard desktop locations
if [ "$DESKTOP_ENTRY" = "null" ] && [ -d "/usr/share/applications" ]; then
  DESKTOP_FILE=$(find /usr/share/applications -name "raneto.desktop" -type f 2>/dev/null | head -1)
  if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
    DESKTOP_ENTRY="\"$DESKTOP_FILE\""
  fi
fi

# Convert icon paths array to JSON
ICON_PATHS_JSON="["
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
  for i in "${!ICON_PATHS[@]}"; do
    if [ $i -gt 0 ]; then
      ICON_PATHS_JSON="${ICON_PATHS_JSON},"
    fi
    # Escape backslashes and quotes in paths for JSON
    escaped_path="${ICON_PATHS[$i]//\\/\\\\}"
    escaped_path="${escaped_path//\"/\\\"}"
    ICON_PATHS_JSON="${ICON_PATHS_JSON}\"${escaped_path}\""
  done
fi
ICON_PATHS_JSON="${ICON_PATHS_JSON}]"

# Output JSON with proper formatting
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY,
  "icon_paths": $ICON_PATHS_JSON,
  "version": "$RANETO_VERSION"
}
EOF