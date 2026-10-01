#!/bin/bash

# Extract Kubernetes metadata

# Find kubectl binary
KUBECTL_BINARY=$(which kubectl)
KUBECTL_NAME=$(basename "$KUBECTL_BINARY")

# Get version
KUBECTL_VERSION=$(kubectl version --client 2>/dev/null | grep -oP 'v[0-9]+\.[0-9]+\.[0-9]+' | head -1)

# Find desktop entry on Linux - check standard locations
DESKTOP_ENTRY="null"
for desktop_dir in /usr/share/applications /usr/local/share/applications ~/.local/share/applications; do
    if [ -f "$desktop_dir/kubectl.desktop" ]; then
        DESKTOP_ENTRY="\"$desktop_dir/kubectl.desktop\""
        break
    fi
done

# Extract display name using multiple strategies:
# 1. Try to find from .desktop file Name= field
# 2. Try to extract from package metadata (dpkg/rpm)
# 3. Use xdg-mime or desktop database
# 4. Fall back to binary name

DISPLAY_NAME=""

# Strategy 1: Extract from .desktop file if available
if [ "$DESKTOP_ENTRY" != "null" ]; then
    # Remove quotes from DESKTOP_ENTRY for file reading
    DESKTOP_FILE="${DESKTOP_ENTRY%\"}"
    DESKTOP_FILE="${DESKTOP_FILE#\"}"
    if [ -f "$DESKTOP_FILE" ]; then
        DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_FILE" | sed 's/^Name=//' | head -1)
    fi
fi

# Strategy 2: Extract from package metadata if available
if [ -z "$DISPLAY_NAME" ]; then
    if command -v dpkg &> /dev/null; then
        PKG_NAME=$(dpkg -S "$(which kubectl)" 2>/dev/null | cut -d: -f1 | head -1)
        if [ -n "$PKG_NAME" ]; then
            # Try to get from Summary or Description field
            PKG_DESC=$(dpkg -s "$PKG_NAME" 2>/dev/null | grep "^Summary:" | sed 's/^Summary: //')
            if [ -z "$PKG_DESC" ]; then
                PKG_DESC=$(dpkg -s "$PKG_NAME" 2>/dev/null | grep "^Description:" | sed 's/^Description: //')
            fi
            if [ -n "$PKG_DESC" ]; then
                DISPLAY_NAME="$PKG_DESC"
            fi
        fi
    elif command -v rpm &> /dev/null; then
        PKG_NAME=$(rpm -qf "$(which kubectl)" 2>/dev/null | head -1)
        if [ -n "$PKG_NAME" ]; then
            DISPLAY_NAME=$(rpm -q --queryformat="%{SUMMARY}" "$PKG_NAME" 2>/dev/null)
        fi
    fi
fi

# Strategy 3: Use xdg-mime or desktop database
if [ -z "$DISPLAY_NAME" ]; then
    if command -v update-desktop-database &> /dev/null; then
        # Desktop database tools might be available
        DISPLAY_NAME=$(grep "kubectl" /usr/share/applications/mimeapps.list 2>/dev/null | head -1)
    fi
fi

# Strategy 4: Fall back to capitalized binary name
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="Kubernetes"
fi

# Find icons - search in multiple locations
ICON_PATHS=()

# Search using dpkg -L to list package files for kubectl
if command -v dpkg &> /dev/null; then
    PKG_NAME=$(dpkg -S "$(which kubectl)" 2>/dev/null | cut -d: -f1 | head -1)
    if [ -n "$PKG_NAME" ]; then
        # Find .png, .svg, .ico files in package
        while IFS= read -r file; do
            if [[ "$file" == *.png ]] || [[ "$file" == *.svg ]] || [[ "$file" == *.ico ]]; then
                ICON_PATHS+=("$file")
            fi
        done < <(dpkg -L "$PKG_NAME" 2>/dev/null | grep -E '\.(png|svg|ico)$')
    fi
elif command -v rpm &> /dev/null; then
    PKG_NAME=$(rpm -qf "$(which kubectl)" 2>/dev/null | head -1)
    if [ -n "$PKG_NAME" ]; then
        # Find .png, .svg, .ico files in package
        while IFS= read -r file; do
            if [[ "$file" == *.png ]] || [[ "$file" == *.svg ]] || [[ "$file" == *.ico ]]; then
                ICON_PATHS+=("$file")
            fi
        done < <(rpm -ql "$PKG_NAME" 2>/dev/null | grep -E '\.(png|svg|ico)$')
    fi
fi

# Search in standard icon directories using find
if [ -d "/usr/share/icons" ]; then
    while IFS= read -r icon; do
        if [ -f "$icon" ]; then
            ICON_PATHS+=("$icon")
        fi
    done < <(find /usr/share/icons -type f \( -name "*kubernetes*" -o -name "*k8s*" -o -name "*kubectl*" \) 2>/dev/null | head -10)
fi

# Search in pixmaps using find
if [ -d "/usr/share/pixmaps" ]; then
    while IFS= read -r icon; do
        if [ -f "$icon" ]; then
            ICON_PATHS+=("$icon")
        fi
    done < <(find /usr/share/pixmaps -type f \( -name "*kubernetes*" -o -name "*k8s*" -o -name "*kubectl*" \) 2>/dev/null)
fi

# Extract icon from .desktop file if available
if [ "$DESKTOP_ENTRY" != "null" ]; then
    DESKTOP_FILE="${DESKTOP_ENTRY%\"}"
    DESKTOP_FILE="${DESKTOP_FILE#\"}"
    if [ -f "$DESKTOP_FILE" ]; then
        DESKTOP_ICON=$(grep "^Icon=" "$DESKTOP_FILE" | sed 's/^Icon=//' | head -1)
        if [ -n "$DESKTOP_ICON" ]; then
            # Try to find the icon file
            ICON_FILE=$(find /usr/share/icons -name "${DESKTOP_ICON}*" 2>/dev/null | head -1)
            if [ -f "$ICON_FILE" ]; then
                ICON_PATHS+=("$ICON_FILE")
            fi
        fi
    fi
fi

# Remove duplicates
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))

# If no icons found, use empty array
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    ICON_ARRAY="[]"
else
    # Create JSON array from icon paths
    ICON_ARRAY="["
    for i in "${!ICON_PATHS[@]}"; do
        if [ $i -gt 0 ]; then
            ICON_ARRAY="$ICON_ARRAY,"
        fi
        ICON_ARRAY="$ICON_ARRAY\"${ICON_PATHS[$i]}\""
    done
    ICON_ARRAY="$ICON_ARRAY]"
fi

# Output JSON metadata
cat << EOF
{
  "binary_path": "$KUBECTL_BINARY",
  "binary_name": "$KUBECTL_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY,
  "icon_paths": $ICON_ARRAY,
  "version": "$KUBECTL_VERSION"
}
EOF