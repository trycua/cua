#!/bin/bash

# Extract metadata for Apache Cassandra

# Try to find cassandra binary using package managers first, then fallback to PATH
BINARY_PATH=""
BINARY_NAME=""

# Method 1: Query dpkg for installed cassandra package binaries
if command -v dpkg &>/dev/null && dpkg -l 2>/dev/null | grep -q cassandra; then
    # Find all executables provided by cassandra package in bin directories
    BINARY_PATH=$(dpkg -L cassandra 2>/dev/null | grep -E '/bin/cassandra$' | head -1)
    if [ -n "$BINARY_PATH" ] && [ -x "$BINARY_PATH" ]; then
        BINARY_NAME=$(basename "$BINARY_PATH")
    fi
fi

# Method 2: Query rpm for installed cassandra package binaries
if [ -z "$BINARY_PATH" ] && command -v rpm &>/dev/null && rpm -q cassandra &>/dev/null 2>&1; then
    BINARY_PATH=$(rpm -ql cassandra 2>/dev/null | grep -E '/bin/cassandra$' | head -1)
    if [ -n "$BINARY_PATH" ] && [ -x "$BINARY_PATH" ]; then
        BINARY_NAME=$(basename "$BINARY_PATH")
    fi
fi

# Method 3: Check if cassandra binary is in PATH
if [ -z "$BINARY_PATH" ]; then
    if BINARY_PATH=$(command -v cassandra 2>/dev/null); then
        BINARY_NAME=$(basename "$BINARY_PATH")
    fi
fi

# Method 4: Check common installation paths
if [ -z "$BINARY_PATH" ]; then
    for bin_path in /opt/cassandra/bin/cassandra /usr/bin/cassandra /usr/local/bin/cassandra; do
        if [ -f "$bin_path" ] && [ -x "$bin_path" ]; then
            BINARY_PATH="$bin_path"
            BINARY_NAME=$(basename "$BINARY_PATH")
            break
        fi
    done
fi

# Get version from package manager or binary
VERSION=""

# Try dpkg first (Debian-based systems)
if command -v dpkg &>/dev/null && dpkg -l 2>/dev/null | grep -q cassandra; then
    VERSION=$(dpkg -s cassandra 2>/dev/null | grep "^Version:" | cut -d: -f2 | xargs)
fi

# Try rpm (RedHat-based systems)
if [ -z "$VERSION" ] && command -v rpm &>/dev/null && rpm -q cassandra &>/dev/null 2>&1; then
    VERSION=$(rpm -q cassandra 2>/dev/null | sed 's/^cassandra-//' | sed 's/-.*//')
fi

# Fall back to binary version string - try multiple paths if direct call fails
if [ -z "$VERSION" ]; then
    # Try the discovered binary path first
    if [ -n "$BINARY_PATH" ] && [ -x "$BINARY_PATH" ]; then
        VERSION=$("$BINARY_PATH" -v 2>/dev/null)
    fi
    
    # If that failed, try direct path to /opt/cassandra/bin/cassandra
    if [ -z "$VERSION" ] && [ -x "/opt/cassandra/bin/cassandra" ]; then
        VERSION=$(/opt/cassandra/bin/cassandra -v 2>/dev/null)
    fi
fi

# Look for .desktop files in standard locations
DESKTOP_ENTRY="null"
if [ -f "/usr/share/applications/cassandra.desktop" ]; then
    DESKTOP_ENTRY="/usr/share/applications/cassandra.desktop"
elif find /usr/share/applications -name "*cassandra*" -type f 2>/dev/null | grep -q .; then
    DESKTOP_ENTRY=$(find /usr/share/applications -name "*cassandra*" -type f 2>/dev/null | head -1)
fi

# Extract display name from .desktop file (primary source)
DISPLAY_NAME=""
if [ "$DESKTOP_ENTRY" != "null" ] && [ -f "$DESKTOP_ENTRY" ]; then
    # Try to extract Name field from desktop file
    DISPLAY_NAME=$(grep -E "^Name=" "$DESKTOP_ENTRY" | head -1 | cut -d= -f2 | xargs)
fi

# If still empty, try to extract from package metadata (dpkg)
if [ -z "$DISPLAY_NAME" ]; then
    if command -v dpkg &>/dev/null && dpkg -l 2>/dev/null | grep -q cassandra; then
        PKG_NAME=$(dpkg -s cassandra 2>/dev/null | grep "^Package:" | cut -d: -f2 | xargs)
        DESC=$(dpkg -s cassandra 2>/dev/null | grep "^Description:" | cut -d: -f2 | xargs)
        if [ -n "$PKG_NAME" ]; then
            DISPLAY_NAME=$(echo "$PKG_NAME" | sed 's/^./\U&/' | sed 's/-/ /g')
            if [ -n "$DESC" ]; then
                DISPLAY_NAME="$DISPLAY_NAME - $(echo $DESC | head -c 50)"
            fi
        fi
    fi
fi

# If still empty, try rpm description
if [ -z "$DISPLAY_NAME" ] && command -v rpm &>/dev/null && rpm -q cassandra &>/dev/null 2>&1; then
    PKG_NAME=$(rpm -q --qf "%{NAME}" cassandra 2>/dev/null)
    DESC=$(rpm -q --qf "%{SUMMARY}" cassandra 2>/dev/null)
    if [ -n "$PKG_NAME" ]; then
        DISPLAY_NAME=$(echo "$PKG_NAME" | sed 's/^./\U&/' | sed 's/-/ /g')
        if [ -n "$DESC" ]; then
            DISPLAY_NAME="$DISPLAY_NAME - $(echo $DESC | head -c 50)"
        fi
    fi
fi

# If still empty, derive from binary name (fallback)
if [ -z "$DISPLAY_NAME" ] && [ -n "$BINARY_NAME" ]; then
    # Capitalize the binary name
    DISPLAY_NAME=$(echo "$BINARY_NAME" | sed 's/^./\U&/;s/-/ /g')
fi

# Find icon files from standard directories
ICON_PATHS=()

# Standard icon locations
for icon_dir in /usr/share/icons /usr/share/pixmaps /opt/cassandra/share/pixmaps ~/.local/share/icons; do
    if [ -d "$icon_dir" ]; then
        while IFS= read -r icon_file; do
            [ -f "$icon_file" ] && ICON_PATHS+=("$icon_file")
        done < <(find "$icon_dir" -iname "*cassandra*" -type f \( -iname "*.png" -o -iname "*.svg" -o -iname "*.ico" \) 2>/dev/null | head -5)
    fi
done

# Try to extract icon from .desktop file if found
if [ "$DESKTOP_ENTRY" != "null" ] && [ -f "$DESKTOP_ENTRY" ]; then
    ICON_NAME=$(grep -E "^Icon=" "$DESKTOP_ENTRY" | head -1 | cut -d= -f2 | xargs)
    if [ -n "$ICON_NAME" ]; then
        # Search for this icon in standard locations
        for icon_path in $(find /usr/share/icons /usr/share/pixmaps -name "$ICON_NAME*" -type f 2>/dev/null | head -3); do
            [ -f "$icon_path" ] && ICON_PATHS+=("$icon_path")
        done
    fi
fi

# Remove duplicates and sort
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))

# Format icon paths as JSON array
ICON_JSON="["
for i in "${!ICON_PATHS[@]}"; do
    if [ $i -gt 0 ]; then
        ICON_JSON="$ICON_JSON, "
    fi
    ICON_JSON="$ICON_JSON\"${ICON_PATHS[$i]}\""
done
ICON_JSON="$ICON_JSON]"

# Format desktop entry as JSON string or null
if [ "$DESKTOP_ENTRY" = "null" ]; then
    DESKTOP_JSON="null"
else
    DESKTOP_JSON="\"$DESKTOP_ENTRY\""
fi

# Format display name as JSON string
DISPLAY_JSON="\"$DISPLAY_NAME\""

# Format version as JSON string
VERSION_JSON="\"$VERSION\""

# Format binary path as JSON string
if [ -z "$BINARY_PATH" ]; then
    BINARY_PATH_JSON="null"
else
    BINARY_PATH_JSON="\"$BINARY_PATH\""
fi

# Format binary name as JSON string
if [ -z "$BINARY_NAME" ]; then
    BINARY_NAME_JSON="null"
else
    BINARY_NAME_JSON="\"$BINARY_NAME\""
fi

# Create JSON output
cat <<EOF
{
  "binary_path": $BINARY_PATH_JSON,
  "binary_name": $BINARY_NAME_JSON,
  "display_name": $DISPLAY_JSON,
  "desktop_entry": $DESKTOP_JSON,
  "icon_paths": $ICON_JSON,
  "version": $VERSION_JSON
}
EOF