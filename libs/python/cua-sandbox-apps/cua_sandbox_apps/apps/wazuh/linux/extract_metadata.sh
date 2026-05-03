#!/bin/bash

# Extract Wazuh Agent metadata dynamically
# Handles permission issues by using sudo when needed

# Initialize variables (no defaults)
BINARY_PATH=""
BINARY_NAME=""
DISPLAY_NAME=""
DESKTOP_ENTRY=""
VERSION=""
declare -a ICON_PATHS

# Step 1: Verify the package is installed and extract basic metadata
if command -v dpkg &> /dev/null; then
    if ! dpkg -s wazuh-agent &>/dev/null 2>&1; then
        echo "Error: wazuh-agent package not installed" >&2
        exit 1
    fi
    
    # Extract display name and version from dpkg
    DISPLAY_NAME=$(dpkg -s wazuh-agent 2>/dev/null | grep "^Description:" | head -1 | sed 's/Description: //')
    VERSION=$(dpkg -s wazuh-agent 2>/dev/null | grep "^Version:" | head -1 | awk '{print $2}')
    
elif command -v rpm &> /dev/null; then
    if ! rpm -q wazuh-agent &>/dev/null 2>&1; then
        echo "Error: wazuh-agent package not installed" >&2
        exit 1
    fi
    
    PKG_INFO=$(rpm -q --info wazuh-agent 2>/dev/null)
    DISPLAY_NAME=$(echo "$PKG_INFO" | grep "^Name " | head -1 | cut -d: -f2 | xargs)
    VERSION=$(rpm -q --queryformat='%{VERSION}' wazuh-agent 2>/dev/null)
else
    echo "Error: neither dpkg nor rpm found" >&2
    exit 1
fi

# Verify we got display name and version
if [ -z "$DISPLAY_NAME" ] || [ -z "$VERSION" ]; then
    echo "Error: failed to extract display name or version" >&2
    exit 1
fi

# Step 2: Find binaries that actually exist on the filesystem
# Get list of files in the package
if command -v dpkg &> /dev/null; then
    PKG_FILES=$(dpkg -L wazuh-agent 2>/dev/null)
elif command -v rpm &> /dev/null; then
    PKG_FILES=$(rpm -ql wazuh-agent 2>/dev/null)
fi

# Look for executable binaries - check they exist (with sudo if needed)
for potential_binary in $(echo "$PKG_FILES" | grep "/bin/wazuh" | grep -v "active-response"); do
    # Check if file exists - try without sudo first, then with sudo
    if [ -f "$potential_binary" ] 2>/dev/null || sudo [ -f "$potential_binary" ] 2>/dev/null; then
        # Prioritize wazuh-control
        if [[ "$potential_binary" == *"wazuh-control"* ]]; then
            BINARY_PATH="$potential_binary"
            break
        elif [ -z "$BINARY_PATH" ]; then
            BINARY_PATH="$potential_binary"
        fi
    fi
done

# Verify binary path exists (check with sudo if necessary)
if [ -z "$BINARY_PATH" ]; then
    echo "Error: wazuh binaries not found on filesystem" >&2
    exit 1
fi

if ! ([ -f "$BINARY_PATH" ] 2>/dev/null || sudo [ -f "$BINARY_PATH" ] 2>/dev/null); then
    echo "Error: binary path does not exist or is not readable" >&2
    exit 1
fi

BINARY_NAME=$(basename "$BINARY_PATH")

# Step 3: Search for .desktop files
for desktop_dir in /usr/share/applications ~/.local/share/applications /usr/local/share/applications; do
    if [ -d "$desktop_dir" ] && [ -z "$DESKTOP_ENTRY" ]; then
        desktop_file=$(find "$desktop_dir" \( -name "*wazuh*" -o -name "*ossec*" \) 2>/dev/null | head -1)
        if [ -f "$desktop_file" ]; then
            DESKTOP_ENTRY="$desktop_file"
            
            # Extract icon from .desktop file if present
            icon_line=$(grep "^Icon=" "$DESKTOP_ENTRY" | head -1 | cut -d= -f2)
            if [ -n "$icon_line" ]; then
                if [ -f "$icon_line" ]; then
                    ICON_PATHS+=("$icon_line")
                else
                    # Try to find in icon theme directories
                    for icon_dir in /usr/share/icons/*/; do
                        icon=$(find "$icon_dir" -name "${icon_line}*" -type f 2>/dev/null | head -1)
                        if [ -f "$icon" ]; then
                            ICON_PATHS+=("$icon")
                            break
                        fi
                    done
                fi
            fi
        fi
    fi
done

# Step 4: Search for security-related icons as fallback
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ] && [ ${#ICON_PATHS[@]} -lt 5 ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done < <(find /usr/share/icons /usr/share/pixmaps -type f \( -name "*security*" -o -name "*shield*" \) 2>/dev/null | head -10)
fi

# Build icon JSON array
ICON_JSON=""
for icon_path in "${ICON_PATHS[@]}"; do
    if [ -z "$ICON_JSON" ]; then
        ICON_JSON="\"$icon_path\""
    else
        ICON_JSON="$ICON_JSON, \"$icon_path\""
    fi
done

# Build JSON output with discovered values only
if [ -z "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY_JSON="null"
else
    DESKTOP_ENTRY_JSON="\"$DESKTOP_ENTRY\""
fi

cat << EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY_JSON,
  "icon_paths": [$ICON_JSON],
  "version": "$VERSION"
}
EOF