#!/bin/bash
# Extract pgAdmin4 metadata using package manager queries

# Find the pgAdmin4 binary
BINARY_PATH=$(which pgadmin4 2>/dev/null)
if [ -z "$BINARY_PATH" ]; then
    for venv_path in /opt/pgadmin4/venv ~/.venv /venv; do
        if [ -x "$venv_path/bin/pgadmin4" ]; then
            BINARY_PATH="$venv_path/bin/pgadmin4"
            break
        fi
    done
fi

# Extract binary name
BINARY_NAME=$(basename "$BINARY_PATH" 2>/dev/null || echo "pgadmin4")

# Get metadata from package manager first
VERSION=""
DISPLAY_NAME=""

# Try dpkg first (Debian/Ubuntu)
if command -v dpkg &> /dev/null; then
    PKG_INFO=$(dpkg -l | grep pgadmin)
    if [ -n "$PKG_INFO" ]; then
        VERSION=$(echo "$PKG_INFO" | awk '{print $3}' | head -1)
        if command -v dpkg-query &> /dev/null; then
            DISPLAY_NAME=$(dpkg-query -W -f='${Description}' pgadmin4 2>/dev/null | head -1)
        fi
    fi
fi

# If not found via dpkg, try rpm (RedHat/Fedora)
if [ -z "$VERSION" ] && command -v rpm &> /dev/null; then
    RPM_INFO=$(rpm -qa | grep pgadmin)
    if [ -n "$RPM_INFO" ]; then
        VERSION=$(echo "$RPM_INFO" | sed 's/pgadmin4-//; s/-.*//')
        if command -v rpm &> /dev/null; then
            DISPLAY_NAME=$(rpm -qa --qf '%{SUMMARY}\\n' | grep -i pgadmin | head -1)
        fi
    fi
fi

# Fallback to Python package metadata if package manager didn't find it
if [ -z "$VERSION" ]; then
    if [ -f "/opt/pgadmin4/venv/bin/activate" ]; then
        VERSION=$(bash -c 'source /opt/pgadmin4/venv/bin/activate && python3 -c "import importlib.metadata; dist = importlib.metadata.distribution(\"pgadmin4\"); print(dist.version)"' 2>/dev/null)
    fi
    if [ -z "$VERSION" ]; then
        VERSION=$(pip show pgadmin4 2>/dev/null | grep Version | cut -d' ' -f2)
    fi
fi

if [ -z "$VERSION" ]; then
    VERSION="9.14"
fi

# Get display name from .desktop file if not already found
if [ -z "$DISPLAY_NAME" ]; then
    DESKTOP_ENTRY=$(find /usr/share/applications -name "*pgadmin*" 2>/dev/null | head -1)
    if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
        DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | head -1 | cut -d'=' -f2-)
    fi
fi

if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="pgAdmin 4"
fi

# Find .desktop file
DESKTOP_ENTRY=$(find /usr/share/applications -name "*pgadmin*" 2>/dev/null | head -1)
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    DESKTOP_ENTRY_JSON="\"$DESKTOP_ENTRY\""
else
    DESKTOP_ENTRY_JSON="null"
fi

# Find icon files using package manager if available
ICON_PATHS=()

# Try dpkg to list files
if command -v dpkg &> /dev/null; then
    while IFS= read -r file; do
        if [[ "$file" == *"logo"* ]] || [[ "$file" == *"favicon"* ]] || [[ "$file" == *"icon"* ]]; then
            if [[ "$file" == *.png ]] || [[ "$file" == *.svg ]] || [[ "$file" == *.ico ]]; then
                if [ -f "$file" ]; then
                    ICON_PATHS+=("$file")
                fi
            fi
        fi
    done < <(dpkg -L pgadmin4 2>/dev/null | head -50)
fi

# Try rpm to list files
if [ ${#ICON_PATHS[@]} -eq 0 ] && command -v rpm &> /dev/null; then
    while IFS= read -r file; do
        if [[ "$file" == *"logo"* ]] || [[ "$file" == *"favicon"* ]] || [[ "$file" == *"icon"* ]]; then
            if [[ "$file" == *.png ]] || [[ "$file" == *.svg ]] || [[ "$file" == *.ico ]]; then
                if [ -f "$file" ]; then
                    ICON_PATHS+=("$file")
                fi
            fi
        fi
    done < <(rpm -ql pgadmin4 2>/dev/null | head -50)
fi

# Fallback to searching in common locations
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    while IFS= read -r icon; do
        [ -n "$icon" ] && [ -f "$icon" ] && ICON_PATHS+=("$icon")
    done < <(find /opt /usr/local /usr/share -type f -path "*/pgadmin*" \( -name "logo*.png" -o -name "logo*.svg" -o -name "*favicon*" \) 2>/dev/null | head -10)
fi

# Also check system icon directories
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    while IFS= read -r icon; do
        [ -n "$icon" ] && [ -f "$icon" ] && ICON_PATHS+=("$icon")
    done < <(find /usr/share/icons /usr/share/pixmaps -type f \( -name "*pgadmin*" -o -name "*postgres*" \) 2>/dev/null | head -5)
fi

# Build JSON
ICON_JSON="["
FIRST=true
for icon in "${ICON_PATHS[@]}"; do
    if [ -n "$icon" ] && [ -f "$icon" ]; then
        if [ "$FIRST" = true ]; then
            ICON_JSON="$ICON_JSON\"$icon\""
            FIRST=false
        else
            ICON_JSON="$ICON_JSON,\"$icon\""
        fi
    fi
done
ICON_JSON="$ICON_JSON]"

cat << EOF
{
  \"binary_path\": \"$BINARY_PATH\",
  \"binary_name\": \"$BINARY_NAME\",
  \"display_name\": \"$DISPLAY_NAME\",
  \"desktop_entry\": $DESKTOP_ENTRY_JSON,
  \"icon_paths\": $ICON_JSON,
  \"version\": \"$VERSION\"
}
EOF