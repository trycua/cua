#!/bin/bash

# Extract MySQL metadata in JSON format

# Function to safely escape JSON strings
json_escape() {
    local s="$1"
    s="${s//\\/\\\\}"  # Escape backslash
    s="${s//\"/\\\"}"  # Escape double quote
    s="${s//$'\t'/\\t}"  # Escape tab
    s="${s//$'\n'/\\n}"  # Escape newline
    s=$(echo "$s" | tr -d '\r')  # Remove carriage returns
    echo "$s"
}

# Discover MySQL packages dynamically
MYSQL_PACKAGES=()
if command -v dpkg &> /dev/null; then
    # Query dpkg for all installed MySQL packages
    while IFS= read -r pkg; do
        if [ -n "$pkg" ]; then
            MYSQL_PACKAGES+=("$pkg")
        fi
    done < <(dpkg -l 2>/dev/null | grep '^ii' | grep -i 'mysql' | awk '{print $2}')
elif command -v rpm &> /dev/null; then
    # Query rpm for MySQL packages
    while IFS= read -r pkg; do
        if [ -n "$pkg" ]; then
            MYSQL_PACKAGES+=("$pkg")
        fi
    done < <(rpm -qa 2>/dev/null | grep -i 'mysql')
fi

# Find the MySQL binary - query package files from discovered packages
BINARY_PATH=""
for pkg in "${MYSQL_PACKAGES[@]}"; do
    if command -v dpkg &> /dev/null; then
        BINARY_PATH=$(dpkg -L "$pkg" 2>/dev/null | grep -E 'bin/mysqld$|sbin/mysqld$' | head -1)
        if [ -n "$BINARY_PATH" ]; then
            break
        fi
    elif command -v rpm &> /dev/null; then
        BINARY_PATH=$(rpm -ql "$pkg" 2>/dev/null | grep -E 'bin/mysqld$|sbin/mysqld$' | head -1)
        if [ -n "$BINARY_PATH" ]; then
            break
        fi
    fi
done

# Fallback to which if dpkg didn't find it
if [ -z "$BINARY_PATH" ]; then
    BINARY_PATH=$(which mysqld 2>/dev/null)
fi

# Extract binary name from path
BINARY_NAME=$(basename "$BINARY_PATH" 2>/dev/null || echo "mysqld")

# Get display name from package description using dpkg or rpm
DISPLAY_NAME=""
for pkg in "${MYSQL_PACKAGES[@]}"; do
    if command -v dpkg &> /dev/null; then
        PKG_DESC=$(dpkg -s "$pkg" 2>/dev/null | grep '^Description:' | head -1 | sed 's/^Description: //')
    elif command -v rpm &> /dev/null; then
        PKG_DESC=$(rpm -qi "$pkg" 2>/dev/null | grep '^Summary' | sed 's/^Summary.*: //')
    fi
    
    if [ -n "$PKG_DESC" ]; then
        # Extract first two words (typically "MySQL database" or "MySQL server")
        DISPLAY_NAME=$(echo "$PKG_DESC" | awk '{for(i=1;i<=2;i++) printf "%s ", $i}' | sed 's/ $//')
        if [ -n "$DISPLAY_NAME" ]; then
            break
        fi
    fi
done

# Find desktop entry files by searching package contents first
DESKTOP_ENTRY=""
for pkg in "${MYSQL_PACKAGES[@]}"; do
    if command -v dpkg &> /dev/null; then
        DESKTOP_ENTRY=$(dpkg -L "$pkg" 2>/dev/null | grep '\.desktop$' | head -1)
    elif command -v rpm &> /dev/null; then
        DESKTOP_ENTRY=$(rpm -ql "$pkg" 2>/dev/null | grep '\.desktop$' | head -1)
    fi
    if [ -n "$DESKTOP_ENTRY" ]; then
        break
    fi
done

# If not found in package, search standard locations (limited depth to avoid slowness)
if [ -z "$DESKTOP_ENTRY" ]; then
    for dir in /usr/share/applications /usr/local/share/applications; do
        if [ -d "$dir" ]; then
            DESKTOP_ENTRY=$(find "$dir" -maxdepth 1 -name '*mysql*' -type f 2>/dev/null | head -1)
            if [ -n "$DESKTOP_ENTRY" ]; then
                break
            fi
        fi
    done
fi

# Find icon paths - search through package files and standard directories (limited)
ICON_PATHS=()

# First, get icon paths from package files
for pkg in "${MYSQL_PACKAGES[@]}"; do
    if command -v dpkg &> /dev/null; then
        while IFS= read -r icon; do
            icon=$(echo "$icon" | tr -d '\r' | tr -d '\n')  # Remove trailing CR/LF
            if [ -n "$icon" ] && [[ "$icon" =~ \.(png|svg|ico)$ ]] && [[ ! "$icon" =~ test|doc|debug ]]; then
                ICON_PATHS+=("$icon")
            fi
        done < <(dpkg -L "$pkg" 2>/dev/null | grep -E '\.(png|svg|ico)$')
    elif command -v rpm &> /dev/null; then
        while IFS= read -r icon; do
            icon=$(echo "$icon" | tr -d '\r' | tr -d '\n')
            if [ -n "$icon" ] && [[ "$icon" =~ \.(png|svg|ico)$ ]] && [[ ! "$icon" =~ test|doc|debug ]]; then
                ICON_PATHS+=("$icon")
            fi
        done < <(rpm -ql "$pkg" 2>/dev/null | grep -E '\.(png|svg|ico)$')
    fi
done

# If no icons found in package, search standard locations (with maxdepth 4 for nested icon themes)
if [ ${#ICON_PATHS[@]} -eq 0 ]; then
    # Search for database-related icons
    for pattern in '*database*' '*server*' '*sql*' '*storage*'; do
        while IFS= read -r icon; do
            icon=$(echo "$icon" | tr -d '\r' | tr -d '\n')
            if [ -n "$icon" ] && [[ "$icon" =~ \.(png|svg|ico)$ ]]; then
                ICON_PATHS+=("$icon")
            fi
        done < <(find /usr/share/icons /usr/share/pixmaps /usr/local/share/icons -maxdepth 4 -name "$pattern" -type f 2>/dev/null | head -5)
    done
fi

# Remove duplicates, sort, and limit results
ICON_PATHS=($(printf '%s\n' "${ICON_PATHS[@]}" | sort -u | head -10))

# Get version - try multiple sources
VERSION=""

# Try mysqld --version first if binary exists
if [ -x "$BINARY_PATH" ]; then
    VERSION=$("$BINARY_PATH" --version 2>/dev/null | awk -F'Ver ' '{print $2}' | awk '{print $1}' | tr -d '\r')
fi

# If version is empty, try mysql-config --version
if [ -z "$VERSION" ] && command -v mysql-config &> /dev/null; then
    VERSION=$(mysql-config --version 2>/dev/null | tr -d '\r')
fi

# Try mysql client --version
if [ -z "$VERSION" ] && command -v mysql &> /dev/null; then
    VERSION=$(mysql --version 2>/dev/null | awk '{print $NF}' | tr -d '\r')
fi

# Try getting version from package manager
if [ -z "$VERSION" ]; then
    for pkg in "${MYSQL_PACKAGES[@]}"; do
        if command -v dpkg &> /dev/null; then
            VERSION=$(dpkg -s "$pkg" 2>/dev/null | grep '^Version:' | awk '{print $2}' | tr -d '\r')
        elif command -v rpm &> /dev/null; then
            VERSION=$(rpm -q "$pkg" 2>/dev/null | sed 's/^[^-]*-//')
        fi
        if [ -n "$VERSION" ]; then
            break
        fi
    done
fi

# Build JSON output with proper array formatting
ICON_ARRAY="["
first=true
for icon in "${ICON_PATHS[@]}"; do
    icon=$(echo "$icon" | tr -d '\r' | tr -d '\n')  # Clean whitespace
    if [ -n "$icon" ]; then
        if [ "$first" = true ]; then
            ICON_ARRAY="${ICON_ARRAY}\"$(json_escape \"$icon\")\""
            first=false
        else
            ICON_ARRAY="${ICON_ARRAY},\"$(json_escape \"$icon\")\""
        fi
    fi
done
ICON_ARRAY="${ICON_ARRAY}]"

# Output JSON - only output what we successfully found
cat <<EOF
{
  \"binary_path\": \"$(json_escape \"$BINARY_PATH\")\",
  \"binary_name\": \"$(json_escape \"$BINARY_NAME\")\",
  \"display_name\": \"$(json_escape \"$DISPLAY_NAME\")\",
  \"desktop_entry\": \"$(json_escape \"$DESKTOP_ENTRY\")\",
  \"icon_paths\": $ICON_ARRAY,
  \"version\": \"$(json_escape \"$VERSION\")\"
}
EOF