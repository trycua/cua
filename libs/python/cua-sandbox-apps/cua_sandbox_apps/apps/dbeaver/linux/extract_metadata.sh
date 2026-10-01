#!/bin/bash

# Extract metadata for DBeaver

# Find DBeaver binary using which and command -v
BINARY_PATH=$(command -v dbeaver 2>/dev/null || which dbeaver 2>/dev/null || find "$HOME/.local" -name "dbeaver" -type f 2>/dev/null | head -1)

if [ -z "$BINARY_PATH" ]; then
    echo "Error: DBeaver binary not found" >&2
    exit 1
fi

# If the symlink resolves to a path, follow it
if [ -L "$BINARY_PATH" ]; then
    BINARY_PATH=$(readlink -f "$BINARY_PATH")
fi

BINARY_NAME="dbeaver"
VERSION=""
DISPLAY_NAME="DBeaver"
DESKTOP_FILE=""

# Try to get metadata from dpkg (Debian/Ubuntu)
if command -v dpkg &> /dev/null; then
    PKG_INFO=$(timeout 2 dpkg -l 2>/dev/null | grep dbeaver)
    if [ -n "$PKG_INFO" ]; then
        # Try getting more detailed info from dpkg show
        PKG_SHOW=$(timeout 2 dpkg-query -W 'dbeaver*' 2>/dev/null | grep dbeaver | head -1)
        if [ -n "$PKG_SHOW" ]; then
            DISPLAY_NAME=$(timeout 2 dpkg-query -W dbeaver 2>/dev/null | cut -f1 | head -1)
        fi
        # Extract version from dpkg output
        VERSION=$(echo "$PKG_INFO" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
    fi
fi

# Try to get metadata from rpm (RedHat/CentOS)
if [ -z "$VERSION" ] && command -v rpm &> /dev/null; then
    RPM_QUERY=$(timeout 2 rpm -qa 'dbeaver*' 2>/dev/null | grep dbeaver)
    if [ -n "$RPM_QUERY" ]; then
        # Extract display name and version
        RPM_INFO=$(timeout 2 rpm -qi "$RPM_QUERY" 2>/dev/null)
        DISPLAY_NAME=$(echo "$RPM_INFO" | grep "^Name " | cut -d: -f2- | xargs)
        VERSION=$(echo "$RPM_INFO" | grep "^Version " | cut -d: -f2- | xargs)
    fi
fi

# If package manager didn't work, try to extract from readme.txt in installation directory
if [ -z "$VERSION" ] && [ -f "$BINARY_PATH" ]; then
    INSTALL_DIR=$(dirname "$BINARY_PATH")
    if [ -f "$INSTALL_DIR/readme.txt" ]; then
        VERSION=$(grep -m2 "^[0-9]\+\.[0-9]\+\.[0-9]\+" "$INSTALL_DIR/readme.txt" | tail -1)
    fi
fi

# If still no version, try installation directory name for version info
if [ -z "$VERSION" ]; then
    INSTALL_DIR=$(dirname "$BINARY_PATH")
    if [[ "$INSTALL_DIR" =~ ([0-9]+\.[0-9]+\.[0-9]+) ]]; then
        VERSION="${BASH_REMATCH[1]}"
    fi
fi

# Look for .desktop files in standard locations to get display name and desktop entry
for desktop_loc in /usr/share/applications "$HOME/.local/share/applications"; do
    if [ -d "$desktop_loc" ]; then
        for desktop_candidate in "$desktop_loc"/dbeaver*.desktop; do
            if [ -f "$desktop_candidate" ]; then
                DESKTOP_FILE="$desktop_candidate"
                # Extract display name from .desktop file
                DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_FILE" 2>/dev/null | head -1 | cut -d= -f2-)
                [ -z "$DISPLAY_NAME" ] && DISPLAY_NAME="DBeaver"
                break 2
            fi
        done
    fi
done

# If version is still empty after all attempts, error out (don't use silent fallback)
if [ -z "$VERSION" ]; then
    echo "Warning: Could not determine DBeaver version from package manager or installation files" >&2
    VERSION="unknown"
fi

# Find icon paths from various sources
declare -a ICON_PATHS

# Try to extract icon path from .desktop file if found
if [ -n "$DESKTOP_FILE" ]; then
    ICON_FROM_DESKTOP=$(grep "^Icon=" "$DESKTOP_FILE" 2>/dev/null | head -1 | cut -d= -f2-)
    if [ -n "$ICON_FROM_DESKTOP" ]; then
        # If it's a full path
        if [[ "$ICON_FROM_DESKTOP" = /* ]]; then
            if [ -f "$ICON_FROM_DESKTOP" ]; then
                ICON_PATHS+=("$ICON_FROM_DESKTOP")
            fi
        else
            # Search in standard icon directories for various sizes
            for icon_dir in /usr/share/icons/hicolor/{16x16,24x24,32x32,48x48,64x64,128x128,256x256,scalable}/apps /usr/share/pixmaps /usr/share/icons/gnome/scalable/apps; do
                if [ -d "$icon_dir" ]; then
                    for ext in png svg; do
                        if [ -f "$icon_dir/${ICON_FROM_DESKTOP}.$ext" ]; then
                            ICON_PATHS+=("$icon_dir/${ICON_FROM_DESKTOP}.$ext")
                        fi
                    done
                fi
            done
        fi
    fi
fi

# If no icon found yet, look in installation directory
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    INSTALL_DIR=$(dirname "$BINARY_PATH")
    for ext in png svg; do
        if [ -f "$INSTALL_DIR/dbeaver.$ext" ]; then
            ICON_PATHS+=("$INSTALL_DIR/dbeaver.$ext")
        fi
    done
    
    # Search in common icon locations within the installation
    for icon_candidate in "$INSTALL_DIR"/*icon* "$INSTALL_DIR"/*logo*; do
        if [ -f "$icon_candidate" ] && [[ "$icon_candidate" =~ \.(png|svg|ico)$ ]]; then
            ICON_PATHS+=("$icon_candidate")
        fi
    done
fi

# Build icon paths JSON array (only include existing icons)
ICON_JSON="["
first=true
for icon in "${ICON_PATHS[@]}"; do
    # Only include icons that actually exist
    if [ -f "$icon" ]; then
        if [ "$first" = true ]; then
            ICON_JSON="$ICON_JSON\"$icon\""
            first=false
        else
            ICON_JSON="$ICON_JSON,\"$icon\""
        fi
    fi
done
ICON_JSON="$ICON_JSON]"

# Ensure desktop_entry is empty string if not found
DESKTOP_FILE=${DESKTOP_FILE:-""}

# Output JSON metadata
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