#!/bin/bash

# Extract metadata for Jenkins installation dynamically

# Find Jenkins wrapper/executable
JENKINS_BIN=$(which jenkins 2>/dev/null || which java 2>/dev/null || echo "")

# Get the actual binary name from the executable
BINARY_NAME=$(basename "$JENKINS_BIN")

# Try to find Jenkins installation via package managers
JENKINS_WAR=""
JENKINS_PATHS=()

# Query dpkg for Jenkins package files (Debian-based)
if command -v dpkg -L &>/dev/null 2>&1; then
    JENKINS_PATHS+=($(dpkg -L jenkins 2>/dev/null | grep -E '\.war$' | head -1))
fi

# Query rpm for Jenkins package files (RedHat-based)
if command -v rpm &>/dev/null 2>&1; then
    JENKINS_PATHS+=($(rpm -ql jenkins 2>/dev/null | grep -E '\.war$' | head -1))
fi

# Check common installation locations dynamically
COMMON_PATHS=(
    "/opt/jenkins/jenkins.war"
    "/usr/share/jenkins/jenkins.war"
    "/var/lib/jenkins/jenkins.war"
    "/usr/local/share/jenkins/jenkins.war"
    "/opt/jenkins-latest/jenkins.war"
)

# Add paths from package manager results
for path in "${JENKINS_PATHS[@]}"; do
    if [ -n "$path" ] && [ -f "$path" ]; then
        COMMON_PATHS=("$path" "${COMMON_PATHS[@]}")
    fi
done

# Find the first existing WAR file
for path in "${COMMON_PATHS[@]}"; do
    if [ -f "$path" ]; then
        JENKINS_WAR="$path"
        break
    fi
done

# If not found in standard locations, use find as fallback
if [ -z "$JENKINS_WAR" ]; then
    JENKINS_WAR=$(find /opt /usr /var -name "jenkins.war" -type f 2>/dev/null | head -1)
fi

# Extract Jenkins version from WAR's META-INF/MANIFEST.MF
JENKINS_VERSION=""
if [ -n "$JENKINS_WAR" ] && [ -f "$JENKINS_WAR" ]; then
    JENKINS_VERSION=$(unzip -p "$JENKINS_WAR" META-INF/MANIFEST.MF 2>/dev/null | grep "Implementation-Version" | sed 's/^.*: //' | tr -d '\r')
fi

# If version still not found, try other methods
if [ -z "$JENKINS_VERSION" ]; then
    # Try running jenkins --version
    if [ -x "$JENKINS_BIN" ]; then
        JENKINS_VERSION=$("$JENKINS_BIN" --version 2>/dev/null || echo "")
    fi
fi

# If still no version, try to get from package manager
if [ -z "$JENKINS_VERSION" ]; then
    # Try dpkg
    if command -v dpkg-query &>/dev/null 2>&1; then
        JENKINS_VERSION=$(dpkg-query -W -f='${Version}' jenkins 2>/dev/null | head -1)
    fi
    # Try rpm
    if [ -z "$JENKINS_VERSION" ] && command -v rpm &>/dev/null 2>&1; then
        JENKINS_VERSION=$(rpm -q --queryformat='%{VERSION}' jenkins 2>/dev/null | head -1)
    fi
fi

# If still no version, extract from WAR filename
if [ -z "$JENKINS_VERSION" ] && [ -n "$JENKINS_WAR" ] && [ -f "$JENKINS_WAR" ]; then
    JENKINS_VERSION=$(basename "$JENKINS_WAR" | sed 's/jenkins-//' | sed 's/\.war//')
fi

# Last resort - check if we can get it from build info in the WAR
if [ -z "$JENKINS_VERSION" ] && [ -n "$JENKINS_WAR" ] && [ -f "$JENKINS_WAR" ]; then
    JENKINS_VERSION=$(unzip -l "$JENKINS_WAR" 2>/dev/null | grep -oP 'jenkins-\d+\.\d+\.\d+' | head -1 | sed 's/jenkins-//')
fi

# Find desktop entry dynamically using locate, find, or dpkg
DESKTOP_FILE=""
DISPLAY_NAME=""

# Try locate first (fastest)
if command -v locate &>/dev/null 2>&1; then
    DESKTOP_FILE=$(locate jenkins.desktop 2>/dev/null | head -1)
fi

# Try dpkg if locate didn't find it
if [ -z "$DESKTOP_FILE" ] && command -v dpkg -L &>/dev/null 2>&1; then
    DESKTOP_FILE=$(dpkg -L jenkins 2>/dev/null | grep "\.desktop$" | head -1)
fi

# Try rpm if dpkg didn't find it
if [ -z "$DESKTOP_FILE" ] && command -v rpm &>/dev/null 2>&1; then
    DESKTOP_FILE=$(rpm -ql jenkins 2>/dev/null | grep "\.desktop$" | head -1)
fi

# Try find as final fallback
if [ -z "$DESKTOP_FILE" ]; then
    DESKTOP_FILE=$(find /usr -name "jenkins.desktop" -type f 2>/dev/null | head -1)
fi

# Extract display name from desktop file if found
if [ -f "$DESKTOP_FILE" ]; then
    DISPLAY_NAME=$(grep -oP '(?<=^Name=).*' "$DESKTOP_FILE" | head -1)
fi

# If no desktop file or display_name found, try dpkg description
if [ -z "$DISPLAY_NAME" ]; then
    if command -v dpkg-query &>/dev/null 2>&1; then
        DISPLAY_NAME=$(dpkg-query -W -f='${Description}' jenkins 2>/dev/null | head -1)
    fi
fi

# Try rpm description if dpkg didn't have it
if [ -z "$DISPLAY_NAME" ] && command -v rpm &>/dev/null 2>&1; then
    DISPLAY_NAME=$(rpm -q --queryformat='%{DESCRIPTION}' jenkins 2>/dev/null | head -1)
fi

# If still no display name, derive from binary name
if [ -z "$DISPLAY_NAME" ]; then
    # Capitalize first letter and use binary name
    DISPLAY_NAME="$(tr '[:lower:]' '[:upper:]' <<< ${BINARY_NAME:0:1})${BINARY_NAME:1}"
fi

# Find icon files from various sources
ICON_PATHS=()

# Try to find icons from package manager
if command -v dpkg -L &>/dev/null 2>&1; then
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done < <(dpkg -L jenkins 2>/dev/null | grep -iE '\.(png|svg|ico)$')
fi

# Try rpm package icons
if command -v rpm &>/dev/null 2>&1; then
    while IFS= read -r icon_file; do
        if [ -f "$icon_file" ]; then
            ICON_PATHS+=("$icon_file")
        fi
    done < <(rpm -ql jenkins 2>/dev/null | grep -iE '\.(png|svg|ico)$')
fi

# Check common icon directories if package manager didn't find icons
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    for icon_dir in /usr/share/icons /usr/share/pixmaps /usr/local/share/icons; do
        if [ -d "$icon_dir" ]; then
            # Find any jenkins-related icons
            while IFS= read -r icon_file; do
                if [ -f "$icon_file" ]; then
                    ICON_PATHS+=("$icon_file")
                fi
            done < <(find "$icon_dir" -iname "*jenkins*" -type f 2>/dev/null)
        fi
    done
fi

# Check if there's an icon in the Jenkins WAR as last resort
if [ ${#ICON_PATHS[@]} -eq 0 ] && [ -n "$JENKINS_WAR" ] && [ -f "$JENKINS_WAR" ]; then
    # List all PNG/SVG files in the WAR
    while IFS= read -r war_icon; do
        if [[ "$war_icon" == *.png ]] || [[ "$war_icon" == *.svg ]] || [[ "$war_icon" == *.ico ]]; then
            # Extract and save temporarily
            TEMP_DIR=$(mktemp -d)
            unzip -p "$JENKINS_WAR" "$war_icon" > "$TEMP_DIR/jenkins_icon" 2>/dev/null
            if [ -s "$TEMP_DIR/jenkins_icon" ]; then
                # Get the actual file extension
                EXT="${war_icon##*.}"
                ICON_PATH="$TEMP_DIR/jenkins_icon.$EXT"
                mv "$TEMP_DIR/jenkins_icon" "$ICON_PATH"
                ICON_PATHS+=("$ICON_PATH")
            fi
            rmdir "$TEMP_DIR" 2>/dev/null || true
            # Only extract one icon
            break
        fi
    done < <(unzip -l "$JENKINS_WAR" 2>/dev/null | grep -iE '\.(png|svg|ico)$' | awk '{print $NF}')
fi

# Remove duplicates from icon paths
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u))

# Format icon paths array for JSON
ICON_JSON="["
for i in "${!ICON_PATHS[@]}"; do
    if [ $i -gt 0 ]; then
        ICON_JSON="$ICON_JSON,"
    fi
    # Escape backslashes and quotes in path
    escaped_path=$(echo "${ICON_PATHS[$i]}" | sed 's/\\/\\\\/g' | sed 's/"/\\"/g')
    ICON_JSON="$ICON_JSON\"$escaped_path\""
done
ICON_JSON="$ICON_JSON]"

# Handle null values for JSON
DESKTOP_JSON="null"
if [ -n "$DESKTOP_FILE" ]; then
    DESKTOP_JSON="\"$DESKTOP_FILE\""
fi

VERSION_JSON="null"
if [ -n "$JENKINS_VERSION" ]; then
    VERSION_JSON="\"$JENKINS_VERSION\""
fi

# Escape display name for JSON
DISPLAY_NAME_JSON=$(echo "$DISPLAY_NAME" | sed 's/"/\\"/g')

# Output JSON metadata - ONLY JSON to stdout
cat <<JSON
{
  "binary_path": "$JENKINS_BIN",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME_JSON",
  "desktop_entry": $DESKTOP_JSON,
  "icon_paths": $ICON_JSON,
  "version": $VERSION_JSON
}
JSON
