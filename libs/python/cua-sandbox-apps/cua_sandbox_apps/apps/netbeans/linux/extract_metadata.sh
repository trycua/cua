#!/bin/bash

# Extract NetBeans metadata with robust discovery

# Find the binary using which
BINARY_PATH=$(which netbeans 2>/dev/null)
if [ -z "$BINARY_PATH" ]; then
    BINARY_PATH="/opt/netbeans/netbeans/bin/netbeans"
fi

# Validate binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo "{\"error\": \"netbeans binary not found\"}" >&2
    exit 1
fi

# Resolve symlink to actual binary
if [ -L "$BINARY_PATH" ]; then
    BINARY_PATH=$(readlink -f "$BINARY_PATH")
fi

# Re-validate after resolving symlink
if [ ! -f "$BINARY_PATH" ]; then
    echo "{\"error\": \"netbeans binary not found after resolving symlink\"}" >&2
    exit 1
fi

# Extract binary_name from the binary path
BINARY_NAME=$(basename "$BINARY_PATH")

# Get the NetBeans home directory (bin/netbeans -> bin -> netbeans)
NETBEANS_HOME=$(dirname "$BINARY_PATH")
NETBEANS_HOME=$(dirname "$NETBEANS_HOME")

# Validate NetBeans home directory exists
if [ ! -d "$NETBEANS_HOME" ]; then
    echo "{\"error\": \"NetBeans home directory not found\"}" >&2
    exit 1
fi

# Extract version from jar manifest file (most reliable)
VERSION=""
CORE_JAR="$NETBEANS_HOME/platform/modules/org-netbeans-core.jar"
if [ -f "$CORE_JAR" ]; then
    VERSION=$(unzip -p "$CORE_JAR" META-INF/MANIFEST.MF 2>/dev/null | grep 'OpenIDE-Module-Implementation-Version:' | grep -oE '[0-9]+' | head -1)
fi

# Fallback to netbeans.conf if jar extraction failed
if [ -z "$VERSION" ] && [ -f "$NETBEANS_HOME/etc/netbeans.conf" ]; then
    VERSION=$(grep 'netbeans_default_userdir' "$NETBEANS_HOME/etc/netbeans.conf" | grep -oE '[0-9]+' | tail -1)
fi

# Try package managers for version if available
if [ -z "$VERSION" ]; then
    if command -v dpkg &> /dev/null && dpkg -l 2>/dev/null | grep -q netbeans; then
        VERSION=$(dpkg -l 2>/dev/null | grep netbeans | grep -oE 'netbeans[_-][0-9]+' | grep -oE '[0-9]+' | head -1)
    fi
fi

# Try rpm if available
if [ -z "$VERSION" ] && command -v rpm &> /dev/null; then
    VERSION=$(rpm -q netbeans 2>/dev/null | grep -oE '[0-9]+' | head -1)
fi

# Look for .desktop file in standard locations
DESKTOP_FILE=$(find /usr/share/applications -type f -name "*netbeans*" 2>/dev/null | head -1)

# Initialize display name and icon paths
DISPLAY_NAME=""
ICON_PATHS=()

# Try to extract from .desktop file first
if [ -n "$DESKTOP_FILE" ] && [ -f "$DESKTOP_FILE" ]; then
    # Extract Name field for display_name
    DESKTOP_NAME=$(grep '^Name=' "$DESKTOP_FILE" 2>/dev/null | head -1 | cut -d'=' -f2-)
    if [ -n "$DESKTOP_NAME" ]; then
        DISPLAY_NAME="$DESKTOP_NAME"
    fi
    
    # Extract Icon field for icon path
    DESKTOP_ICON=$(grep '^Icon=' "$DESKTOP_FILE" 2>/dev/null | head -1 | cut -d'=' -f2-)
    if [ -n "$DESKTOP_ICON" ] && [ -f "$DESKTOP_ICON" ]; then
        ICON_PATHS+=("$DESKTOP_ICON")
    fi
fi

# Use default display name if not found in .desktop file
if [ -z "$DISPLAY_NAME" ]; then
    if [ -n "$VERSION" ]; then
        DISPLAY_NAME="Apache NetBeans IDE $VERSION"
    else
        DISPLAY_NAME="Apache NetBeans IDE"
    fi
fi

# Look for jar files containing icon resources
if [ -d "$NETBEANS_HOME" ]; then
    while IFS= read -r jar_file; do
        if [ -f "$jar_file" ] && unzip -l "$jar_file" 2>/dev/null | grep -qE '\.(png|svg|ico|gif)$'; then
            ICON_PATHS+=("$jar_file")
            break
        fi
    done < <(find "$NETBEANS_HOME" -type f -name "org-netbeans-core.jar" 2>/dev/null)
fi

# Look for loose icon files in standard NetBeans locations
for search_dir in "$NETBEANS_HOME/etc" "$NETBEANS_HOME/nb" "$NETBEANS_HOME/ide" "$NETBEANS_HOME/platform"; do
    if [ -d "$search_dir" ]; then
        while IFS= read -r icon_file; do
            if [ -f "$icon_file" ]; then
                ICON_PATHS+=("$icon_file")
            fi
        done < <(find "$search_dir" -type f \( -name "*.png" -o -name "*.svg" -o -name "*.ico" -o -name "*.gif" \) 2>/dev/null)
    fi
done

# Query package managers for installed files if available
if command -v dpkg &> /dev/null; then
    if dpkg -l 2>/dev/null | grep -q netbeans; then
        while IFS= read -r pkg_file; do
            if [ -f "$pkg_file" ]; then
                if [[ "$pkg_file" == *.jar ]]; then
                    if unzip -l "$pkg_file" 2>/dev/null | grep -qE '\.(png|svg|ico|gif)$'; then
                        ICON_PATHS+=("$pkg_file")
                    fi
                else
                    ICON_PATHS+=("$pkg_file")
                fi
            fi
        done < <(dpkg -L netbeans 2>/dev/null | grep -E '\.(png|svg|ico|gif|jar)$')
    fi
fi

# Build JSON array for icon paths, removing duplicates and validating existence
ICON_JSON="["
FIRST=1
declare -A SEEN_ICONS
for icon in "${ICON_PATHS[@]}"; do
    if [ -n "$icon" ] && [ -z "${SEEN_ICONS[$icon]}" ]; then
        # Validate path exists
        if [ -e "$icon" ]; then
            if [ $FIRST -eq 1 ]; then
                ICON_JSON="$ICON_JSON\"$icon\""
                FIRST=0
            else
                ICON_JSON="$ICON_JSON,\"$icon\""
            fi
            SEEN_ICONS[$icon]=1
        fi
    fi
done
ICON_JSON="$ICON_JSON]"

# Ensure desktop_entry has proper value
if [ -z "$DESKTOP_FILE" ]; then
    DESKTOP_FILE=""
fi

# Output JSON metadata
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