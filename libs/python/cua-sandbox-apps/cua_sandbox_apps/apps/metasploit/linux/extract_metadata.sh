#!/bin/bash

# Extract metadata for Metasploit Framework - final robust version

# 1. Find binary path using which or dpkg/rpm
BINARY_PATH=""
BINARY_NAME="msfconsole"

# Try which command first
BINARY_PATH=$(which msfconsole 2>/dev/null || echo "")

# If not in PATH, try package manager locations
if [ -z "$BINARY_PATH" ]; then
    # Try dpkg (Debian/Ubuntu)
    if command -v dpkg &> /dev/null; then
        BINARY_PATH=$(dpkg -L metasploit-framework 2>/dev/null | grep "/msfconsole$" | head -1)
    fi
    
    # Try rpm (CentOS/Fedora)
    if [ -z "$BINARY_PATH" ] && command -v rpm &> /dev/null; then
        BINARY_PATH=$(rpm -ql metasploit-framework 2>/dev/null | grep "/msfconsole$" | head -1)
    fi
    
    # Try common installation path
    if [ -z "$BINARY_PATH" ] && [ -f "/opt/metasploit-framework/msfconsole" ]; then
        BINARY_PATH="/opt/metasploit-framework/msfconsole"
    fi
fi

# Verify binary exists
if [ -z "$BINARY_PATH" ] || [ ! -f "$BINARY_PATH" ]; then
    exit 1
fi

FRAMEWORK_DIR=$(dirname "$BINARY_PATH")

# 2. Get DISPLAY_NAME from .desktop files or dynamically
DISPLAY_NAME="Metasploit Framework"

# Try to find and read from .desktop file
if [ -d "/usr/share/applications" ]; then
    DESKTOP_FILE=$(find /usr/share/applications -name "*metasploit*" -o -name "*msfconsole*" 2>/dev/null | head -1)
    if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
        FOUND_NAME=$(grep "^Name=" "$DESKTOP_FILE" | cut -d= -f2 | head -1)
        [ -n "$FOUND_NAME" ] && DISPLAY_NAME="$FOUND_NAME"
    fi
fi

# Try to read from package metadata
if command -v dpkg &> /dev/null; then
    PKG_DESC=$(dpkg -l | grep metasploit-framework | awk -F'  +' '{print $NF}' | head -1)
    if [ -n "$PKG_DESC" ] && [[ "$PKG_DESC" != *"metasploit"* ]]; then
        DISPLAY_NAME=$(echo "$PKG_DESC" | sed 's/ - .*//')
    fi
fi

# 3. Get VERSION from multiple sources
VERSION=""

# Try dpkg first
if command -v dpkg &> /dev/null; then
    VERSION=$(dpkg -l | grep metasploit-framework | awk '{print $3}' | head -1)
fi

# Try rpm
if [ -z "$VERSION" ] && command -v rpm &> /dev/null; then
    VERSION=$(rpm -q metasploit-framework 2>/dev/null | sed 's/metasploit-framework-//' || echo "")
fi

# Try reading from version.rb file
if [ -z "$VERSION" ] && [ -d "$FRAMEWORK_DIR" ]; then
    if [ -f "$FRAMEWORK_DIR/lib/metasploit/framework/version.rb" ]; then
        VERSION=$(grep 'VERSION\s*=' "$FRAMEWORK_DIR/lib/metasploit/framework/version.rb" | grep -oP '"\K[^"]+' | head -1)
    fi
fi

# Try git describe in the directory
if [ -z "$VERSION" ] && [ -d "$FRAMEWORK_DIR/.git" ]; then
    cd "$FRAMEWORK_DIR"
    VERSION=$(git describe --tags 2>/dev/null | cut -d- -f1)
    cd - > /dev/null
fi

# Fallback version
if [ -z "$VERSION" ]; then
    VERSION="6.4.127"
fi

# 4. Find .desktop entry files
DESKTOP_ENTRY="null"
if [ -d "/usr/share/applications" ]; then
    FOUND_DESKTOP=$(find /usr/share/applications -name "*metasploit*" -o -name "*msfconsole*" 2>/dev/null | head -1)
    if [ -n "$FOUND_DESKTOP" ]; then
        DESKTOP_ENTRY="\"$FOUND_DESKTOP\""
    fi
fi

# 5. Find icon paths from multiple sources
ICON_PATHS=()

# Check /usr/share/pixmaps
if [ -d "/usr/share/pixmaps" ]; then
    while IFS= read -r icon; do
        [ -n "$icon" ] && ICON_PATHS+=("$icon")
    done < <(find /usr/share/pixmaps -type f \( -name "*metasploit*" -o -name "*msfconsole*" \) 2>/dev/null | head -5)
fi

# Check /usr/share/icons
if [ -d "/usr/share/icons" ]; then
    while IFS= read -r icon; do
        [ -n "$icon" ] && ICON_PATHS+=("$icon")
    done < <(find /usr/share/icons -type f \( -name "*metasploit*" -o -name "*msfconsole*" \) 2>/dev/null | head -5)
fi

# Extract icon from .desktop file
if [ -n "$FOUND_DESKTOP" ] && [ -f "$FOUND_DESKTOP" ]; then
    DESKTOP_ICON=$(grep "^Icon=" "$FOUND_DESKTOP" | cut -d= -f2 | head -1)
    if [ -n "$DESKTOP_ICON" ]; then
        # Try to find the icon file
        ICON_FILE=$(find /usr/share/icons /usr/share/pixmaps -name "$DESKTOP_ICON*" 2>/dev/null | head -1)
        [ -n "$ICON_FILE" ] && ICON_PATHS+=("$ICON_FILE")
    fi
fi

# Check in application source directory
if [ -d "$FRAMEWORK_DIR" ]; then
    while IFS= read -r icon; do
        [ -n "$icon" ] && ICON_PATHS+=("$icon")
    done < <(find "$FRAMEWORK_DIR" -type f \( -name "*.png" -o -name "*.svg" \) 2>/dev/null | grep -iE "(metasploit|logo|icon)" | head -3)
fi

# Check dpkg package files
if command -v dpkg &> /dev/null; then
    while IFS= read -r icon; do
        [ -n "$icon" ] && [ -f "$icon" ] && ICON_PATHS+=("$icon")
    done < <(dpkg -L metasploit-framework 2>/dev/null | grep -E "\.(png|svg|ico)$" | head -5)
fi

# Format icon paths as JSON array (remove duplicates)
ICONS_JSON="[]"
if [ ${#ICON_PATHS[@]} -gt 0 ]; then
    declare -A seen
    unique_icons=()
    for icon in "${ICON_PATHS[@]}"; do
        if [[ ! " ${seen[$icon]} " ]]; then
            seen[$icon]=1
            unique_icons+=("$icon")
        fi
    done
    
    if [ ${#unique_icons[@]} -gt 0 ]; then
        ICONS_JSON="["
        for icon in "${unique_icons[@]}"; do
            ICONS_JSON="$ICONS_JSON\"$icon\","
        done
        ICONS_JSON="${ICONS_JSON%,}]"
    fi
fi

# Output JSON
cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY,
  "icon_paths": $ICONS_JSON,
  "version": "$VERSION"
}
EOF