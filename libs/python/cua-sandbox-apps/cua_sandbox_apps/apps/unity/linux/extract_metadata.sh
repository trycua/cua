#!/bin/bash
# Extract metadata for Unity Hub installation

BINARY_NAME=""
BINARY_PATH=""
DISPLAY_NAME=""
VERSION=""
DESKTOP_ENTRY=""
ICON_PATHS=()

# Step 1: Find the binary dynamically
# Try command -v first (most reliable)
BINARY_PATH=$(command -v unity-hub 2>/dev/null || true)

# If not found, try which
if [ -z "$BINARY_PATH" ]; then
    BINARY_PATH=$(which unity-hub 2>/dev/null || true)
fi

# If still not found, check common installation paths
if [ -z "$BINARY_PATH" ]; then
    for path in /usr/local/bin/unity-hub ~/.local/bin/UnityHub/squashfs-root/unityhub /opt/unity-hub/unityhub; do
        if [ -f "$path" ] || [ -L "$path" ]; then
            BINARY_PATH="$path"
            break
        fi
    done
fi

# Get binary name from the path
if [ -n "$BINARY_PATH" ]; then
    BINARY_NAME=$(basename "$BINARY_PATH")
else
    # Fallback: look for unityhub in PATH
    BINARY_PATH=$(find /usr -name "unityhub" -type f 2>/dev/null | head -1)
    if [ -n "$BINARY_PATH" ]; then
        BINARY_NAME=$(basename "$BINARY_PATH")
    fi
fi

# Step 2: Find desktop entry file
# Check standard locations for desktop files
for desk_file in "$HOME/.local/share/applications/Unity Hub.desktop" \
                "$HOME/.local/share/applications/unityhub.desktop" \
                "$HOME/.local/bin/UnityHub/squashfs-root/unityhub.desktop" \
                "/usr/share/applications/Unity Hub.desktop" \
                "/usr/share/applications/unityhub.desktop"; do
    if [ -f "$desk_file" ]; then
        DESKTOP_ENTRY="$desk_file"
        break
    fi
done

# If not found in standard locations, search for it
if [ -z "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY=$(find /usr/share/applications ~/.local/share/applications -name "*unityhub*" -type f 2>/dev/null | head -1)
fi

# Step 3: Extract metadata from desktop entry
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    # Extract display name from Name field
    DISPLAY_NAME=$(grep -m 1 "^Name=" "$DESKTOP_ENTRY" | sed 's/^Name=//' || true)
    
    # Try to extract version from X-AppImage-Version
    VERSION=$(grep -m 1 "^X-AppImage-Version=" "$DESKTOP_ENTRY" | sed 's/^X-AppImage-Version=//' || true)
    
    # Try to extract version from Comment field as fallback
    if [ -z "$VERSION" ]; then
        VERSION=$(grep -m 1 "^Comment=" "$DESKTOP_ENTRY" | sed 's/^Comment=//' || true)
    fi
fi

# Step 4: Try to get version from package manager if installed as package
if command -v dpkg &>/dev/null; then
    # Check if unity-hub is in dpkg database
    PKG_VERSION=$(dpkg -l 2>/dev/null | grep -i unity | awk '{print $3}' | head -1 || true)
    if [ -n "$PKG_VERSION" ] && [ -z "$VERSION" ]; then
        VERSION="$PKG_VERSION"
    fi
fi

# Step 5: Search for icon files
# Get the directory containing the binary to search for icons nearby
if [ -n "$BINARY_PATH" ]; then
    BINARY_DIR=$(dirname "$(readlink -f "$BINARY_PATH" 2>/dev/null || echo "$BINARY_PATH")")
    
    # Look for icons in the app installation directory (e.g., in squashfs-root/usr/share/icons)
    while IFS= read -r icon_file; do
        ICON_PATHS+=("$icon_file")
    done < <(find "$BINARY_DIR" -path "*/share/icons/*" -name "*unityhub*" -type f 2>/dev/null | head -20)
fi

# Also check system icon directories
while IFS= read -r icon_file; do
    ICON_PATHS+=("$icon_file")
done < <(find /usr/share/icons ~/.local/share/icons -name "*unityhub*" -type f 2>/dev/null | head -20)

# Remove duplicates from icon paths (using associative array)
declare -A seen
for icon in "${ICON_PATHS[@]}"; do
    if [ ! ${seen[$icon]+_} ]; then
        seen["$icon"]=1
    fi
done
ICON_PATHS=()
for icon in "${!seen[@]}"; do
    ICON_PATHS+=("$icon")
done

# Step 6: Fallback defaults
if [ -z "$DISPLAY_NAME" ]; then
    # Try to get from binary if it's an ELF executable
    if [ -f "$BINARY_PATH" ]; then
        if file "$BINARY_PATH" 2>/dev/null | grep -q "ELF"; then
            # Try strings command to find version info
            VERSION=$(strings "$BINARY_PATH" 2>/dev/null | grep -i "version" | head -1 || true)
        fi
    fi
    # Default display name
    DISPLAY_NAME="Unity Hub"
fi

# Build the icon paths JSON array manually
ICON_JSON="["
for i in "${!ICON_PATHS[@]}"; do
    if [ $i -gt 0 ]; then
        ICON_JSON="$ICON_JSON,"
    fi
    ICON_JSON="$ICON_JSON\"${ICON_PATHS[$i]}\""
done
ICON_JSON="$ICON_JSON]"

# Build desktop_entry JSON value
if [ -n "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY_JSON="\"$DESKTOP_ENTRY\""
else
    DESKTOP_ENTRY_JSON="null"
fi

# Build version JSON value
if [ -n "$VERSION" ]; then
    VERSION_JSON="\"$VERSION\""
else
    VERSION_JSON="null"
fi

# Output JSON
cat <<EOF
{
  \"binary_path\": \"$BINARY_PATH\",
  \"binary_name\": \"$BINARY_NAME\",
  \"display_name\": \"$DISPLAY_NAME\",
  \"desktop_entry\": $DESKTOP_ENTRY_JSON,
  \"icon_paths\": $ICON_JSON,
  \"version\": $VERSION_JSON
}
EOF
