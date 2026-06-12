#!/bin/bash

# Extract metadata for Eclipse IDE - robust version

# First, try to find Eclipse binary through multiple methods
BINARY_PATH=""

# Method 1: Check standard installation directories
for install_dir in "$HOME/.local/opt/eclipse-ide" "$HOME/.local/eclipse" "/opt/eclipse" "/usr/local/eclipse" "/usr/lib/eclipse"; do
    if [ -f "$install_dir/eclipse/eclipse" ]; then
        BINARY_PATH="$install_dir/eclipse/eclipse"
        break
    elif [ -f "$install_dir/eclipse" ]; then
        BINARY_PATH="$install_dir/eclipse"
        break
    fi
done

# Method 2: Use 'which' command
if [ -z "$BINARY_PATH" ] && command -v eclipse &> /dev/null; then
    BINARY_PATH=$(command -v eclipse)
fi

# Method 3: Check package manager installations
if [ -z "$BINARY_PATH" ]; then
    if command -v dpkg &> /dev/null; then
        INSTALLED_BIN=$(dpkg -L eclipse 2>/dev/null | grep -E "bin/eclipse$|/eclipse$" | head -1)
        [ -n "$INSTALLED_BIN" ] && BINARY_PATH="$INSTALLED_BIN"
    elif command -v rpm &> /dev/null; then
        INSTALLED_BIN=$(rpm -ql eclipse 2>/dev/null | grep -E "bin/eclipse$|/eclipse$" | head -1)
        [ -n "$INSTALLED_BIN" ] && BINARY_PATH="$INSTALLED_BIN"
    fi
fi

# Get binary name
BINARY_NAME=$(basename "$BINARY_PATH")

# Determine installation root directory from binary path
INSTALL_ROOT=""
if [ -n "$BINARY_PATH" ]; then
    # If path ends with /eclipse/eclipse, get the parent directory (eclipse root)
    if [[ "$BINARY_PATH" == */eclipse/eclipse ]]; then
        INSTALL_ROOT=$(dirname "$BINARY_PATH")
    else
        # Otherwise assume it's in eclipse root
        INSTALL_ROOT=$(dirname "$BINARY_PATH")
    fi
fi

# Initialize display name with fallback
DISPLAY_NAME="Eclipse IDE"

# Try to get display name from eclipse.ini
if [ -n "$INSTALL_ROOT" ] && [ -f "$INSTALL_ROOT/eclipse.ini" ]; then
    if grep -q "org.eclipse.epp.package.java.product" "$INSTALL_ROOT/eclipse.ini"; then
        DISPLAY_NAME="Eclipse IDE for Java Developers"
    fi
fi

# Try to get display name from .desktop files first (more authoritative)
DESKTOP_ENTRY=""
if [ -d /usr/share/applications ]; then
    for desktop_file in /usr/share/applications/eclipse*.desktop /usr/share/applications/org.eclipse*.desktop; do
        if [ -f "$desktop_file" ]; then
            DESKTOP_ENTRY="$desktop_file"
            NAME_FROM_DESKTOP=$(grep "^Name=" "$desktop_file" 2>/dev/null | head -1 | cut -d= -f2)
            if [ -n "$NAME_FROM_DESKTOP" ]; then
                DISPLAY_NAME="$NAME_FROM_DESKTOP"
            fi
            break
        fi
    done
fi

# Get version - try multiple methods
VERSION=""

# Method 1: Parse from plugin directory name (most reliable)
if [ -n "$INSTALL_ROOT" ] && [ -d "$INSTALL_ROOT/plugins" ]; then
    VERSION=$(ls -d "$INSTALL_ROOT"/plugins/org.eclipse.platform_* 2>/dev/null | head -1 | sed 's/.*_\([0-9.]*\)\..*/\1/')
fi

# Method 2: Parse from config.ini
if [ -z "$VERSION" ] && [ -n "$INSTALL_ROOT" ] && [ -f "$INSTALL_ROOT/configuration/config.ini" ]; then
    VERSION=$(grep "eclipse.buildId=" "$INSTALL_ROOT/configuration/config.ini" 2>/dev/null | cut -d= -f2 | head -1)
fi

# Method 3: Check package manager for version
if [ -z "$VERSION" ]; then
    if command -v dpkg &> /dev/null && dpkg -l 2>/dev/null | grep -q eclipse; then
        VERSION=$(dpkg -l 2>/dev/null | grep eclipse | awk '{print $3}' | head -1)
    elif command -v rpm &> /dev/null; then
        VERSION=$(rpm -q eclipse 2>/dev/null | sed 's/eclipse-//' || echo "")
    fi
fi

# Ensure VERSION is not empty
if [ -z "$VERSION" ]; then
    VERSION="unknown"
fi

# Find icon files - multiple search strategies
ICON_PATHS=()

# Strategy 1: Look for icons in standard application directories
if [ -d /usr/share/applications ]; then
    for desktop_file in /usr/share/applications/eclipse*.desktop /usr/share/applications/org.eclipse*.desktop; do
        if [ -f "$desktop_file" ]; then
            ICON_SPEC=$(grep "^Icon=" "$desktop_file" 2>/dev/null | cut -d= -f2 | head -1)
            if [ -n "$ICON_SPEC" ]; then
                if [[ "$ICON_SPEC" = /* ]]; then
                    # Absolute path
                    [ -f "$ICON_SPEC" ] && ICON_PATHS+=("$ICON_SPEC")
                else
                    # Search in pixmaps
                    for ext in png svg ico; do
                        if [ -f "/usr/share/pixmaps/${ICON_SPEC}.${ext}" ]; then
                            ICON_PATHS+=("/usr/share/pixmaps/${ICON_SPEC}.${ext}")
                            break
                        fi
                    done
                    # Also check without extension if found
                    if [ -f "/usr/share/pixmaps/$ICON_SPEC" ]; then
                        ICON_PATHS+=("/usr/share/pixmaps/$ICON_SPEC")
                    fi
                fi
            fi
            break
        fi
    done
fi

# Strategy 2: Look in installation directory for splash/icons
if [ -n "$INSTALL_ROOT" ] && [ -d "$INSTALL_ROOT/plugins" ]; then
    # Find splash files
    while IFS= read -r icon_file; do
        [ -f "$icon_file" ] && ICON_PATHS+=("$icon_file")
    done < <(find "$INSTALL_ROOT/plugins" -name "splash.*" -type f 2>/dev/null | head -2)
    
    # Find standard icon files
    while IFS= read -r icon_file; do
        [ -f "$icon_file" ] && ICON_PATHS+=("$icon_file")
    done < <(find "$INSTALL_ROOT/plugins" -type f \( -name "*.png" -o -name "*.svg" \) -path "*icon*" 2>/dev/null | head -3)
fi

# Strategy 3: Check package manager for icon locations
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    if command -v dpkg &> /dev/null && dpkg -L eclipse 2>/dev/null | grep -q "icon|pixmap"; then
        while IFS= read -r icon_file; do
            [ -f "$icon_file" ] && ICON_PATHS+=("$icon_file")
        done < <(dpkg -L eclipse 2>/dev/null | grep -E "\.png$|\.svg$|\.ico$" | head -5)
    fi
fi

# Remove duplicates and verify files exist
declare -A unique_icons
for icon in "${ICON_PATHS[@]}"; do
    if [ -f "$icon" ]; then
        unique_icons["$icon"]=1
    fi
done

# Build JSON icon array
ICON_JSON=""
for icon in "${!unique_icons[@]}"; do
    if [ -n "$ICON_JSON" ]; then
        ICON_JSON="${ICON_JSON},"
    fi
    ICON_JSON="${ICON_JSON}\"${icon}\""
done

# Output JSON metadata
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": "$DESKTOP_ENTRY",
  "icon_paths": [$ICON_JSON],
  "version": "$VERSION"
}
EOF