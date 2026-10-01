#!/bin/bash

# Extract metadata for GitHub Actions Runner - Dynamic discovery with artifact inspection

# Try to find the runner binary dynamically
find_runner_binary() {
    # Check in PATH
    if command -v github-actions-runner &> /dev/null; then
        command -v github-actions-runner
        return 0
    fi
    
    # Check common installation locations
    for path in ~/.github-actions-runner /opt/github-actions-runner /usr/local/lib/github-actions-runner; do
        if [ -f "$path/run.sh" ]; then
            echo "$path/run.sh"
            return 0
        fi
    done
    
    # Fallback to home directory search
    if [ -f "$HOME/.github-actions-runner/run.sh" ]; then
        echo "$HOME/.github-actions-runner/run.sh"
        return 0
    fi
    
    # Last resort: find in entire home directory (limited depth)
    find "$HOME" -maxdepth 3 -name "run.sh" -path "*/github-actions*" -type f 2>/dev/null | head -1
}

# Get version from binary
get_runner_version_from_binary() {
    local binary="$1"
    
    # Try to get version from the binary directly
    if [ -f "$binary" ]; then
        "$binary" --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1
    fi
}

# Get version from .deps.json file in the runner directory
get_version_from_deps_json() {
    local runner_dir="$1"
    
    if [ -f "$runner_dir/bin/Runner.Listener.deps.json" ]; then
        grep -oP '"Runner\.Listener/\K[0-9]+\.[0-9]+\.[0-9]+' "$runner_dir/bin/Runner.Listener.deps.json" | head -1
    fi
}

# Get version from package manager
get_package_version() {
    # Try dpkg first
    if command -v dpkg &>/dev/null; then
        dpkg -l 2>/dev/null | grep -i 'github.*actions.*runner' | awk '{print $3}' | head -1
    fi
    
    # Try rpm
    if command -v rpm &>/dev/null; then
        rpm -qa 2>/dev/null | grep -i 'github.*actions.*runner' | sed 's/.*-\([0-9.]*\).*/\1/' | head -1
    fi
}

# Get display name from .desktop file
get_display_name_from_desktop() {
    for desktop_file in /usr/share/applications/github-actions-runner.desktop \
                        /usr/local/share/applications/github-actions-runner.desktop \
                        "$HOME/.local/share/applications/github-actions-runner.desktop"; do
        if [ -f "$desktop_file" ]; then
            grep "^Name=" "$desktop_file" 2>/dev/null | cut -d'=' -f2 | head -1
        fi
    done
}

# Get display name from package metadata
get_display_name_from_package() {
    # Try dpkg
    if command -v dpkg &>/dev/null; then
        local pkg_name=$(dpkg -l 2>/dev/null | grep -i 'github.*actions.*runner' | awk '{print $2}' | head -1)
        if [ -n "$pkg_name" ]; then
            dpkg -s "$pkg_name" 2>/dev/null | grep "Description:" | cut -d':' -f2 | xargs | head -1
        fi
    fi
    
    # Try rpm
    if command -v rpm &>/dev/null; then
        local pkg_name=$(rpm -qa 2>/dev/null | grep -i 'github.*actions.*runner' | head -1)
        if [ -n "$pkg_name" ]; then
            rpm -qi "$pkg_name" 2>/dev/null | grep "Summary" | cut -d':' -f2 | xargs | head -1
        fi
    fi
}

# Get display name by inspecting binary/filename
get_display_name_from_binary() {
    local binary="$1"
    local binary_name=$(basename "$binary")
    
    # Check if binary name contains github-actions or similar
    if echo "$binary_name" | grep -qi 'github.*action'; then
        echo "GitHub Actions Runner"
        return 0
    fi
    
    # Check symlink target
    if [ -L "$binary" ]; then
        local target=$(readlink -f "$binary" 2>/dev/null)
        local target_name=$(basename "$target")
        if echo "$target_name" | grep -qi 'github.*action'; then
            echo "GitHub Actions Runner"
            return 0
        fi
        
        # Check parent directory of target
        local target_dir=$(dirname "$target")
        local target_parent=$(basename "$target_dir")
        if echo "$target_parent" | grep -qi 'github.*action'; then
            echo "GitHub Actions Runner"
            return 0
        fi
    fi
    
    # Try to extract from help output
    if [ -f "$binary" ]; then
        local help_output=$("$binary" --help 2>/dev/null | head -30)
        # Look for descriptive text
        if echo "$help_output" | grep -qi 'github'; then
            echo "GitHub Actions Runner"
            return 0
        fi
    fi
}

# Find desktop entry
find_desktop_entry() {
    for desktop_file in /usr/share/applications/github-actions-runner.desktop \
                        /usr/local/share/applications/github-actions-runner.desktop \
                        "$HOME/.local/share/applications/github-actions-runner.desktop"; do
        if [ -f "$desktop_file" ]; then
            echo "$desktop_file"
            return 0
        fi
    done
    echo "null"
}

# Find icon files
find_icon_paths() {
    local icons=()
    
    # Check standard icon locations
    for icon_dir in /usr/share/icons /usr/local/share/icons ~/.local/share/icons; do
        if [ -d "$icon_dir" ]; then
            while IFS= read -r icon_file; do
                if [ -f "$icon_file" ]; then
                    icons+=("$icon_file")
                fi
            done < <(find "$icon_dir" -type f \( -name "*github*" -o -name "*actions*" \) 2>/dev/null | head -5)
        fi
    done
    
    # Look for runner directory icons
    local runner_dir=$(dirname "$(find_runner_binary)")
    if [ -d "$runner_dir" ]; then
        for icon_ext in png svg ico; do
            if [ -f "$runner_dir/icon.$icon_ext" ]; then
                icons+=("$runner_dir/icon.$icon_ext")
            fi
        done
    fi
    
    # Try to find from .desktop file
    if [ ${#icons[@]} -eq 0 ]; then
        local desktop=$(find_desktop_entry)
        if [ "$desktop" != "null" ] && [ -f "$desktop" ]; then
            local icon_name=$(grep "^Icon=" "$desktop" 2>/dev/null | cut -d'=' -f2 | head -1)
            if [ -n "$icon_name" ]; then
                while IFS= read -r icon_file; do
                    if [ -f "$icon_file" ]; then
                        icons+=("$icon_file")
                    fi
                done < <(find /usr/share/icons ~/.local/share/icons -name "$icon_name*" 2>/dev/null | head -3)
            fi
        fi
    fi
    
    # If still no icons, use generic system icon
    if [ ${#icons[@]} -eq 0 ] && [ -f "/usr/share/pixmaps/debian-logo.png" ]; then
        icons+=("/usr/share/pixmaps/debian-logo.png")
    fi
    
    # Output as JSON array
    if [ ${#icons[@]} -eq 0 ]; then
        echo "[]"
    else
        local json_array="["
        for i in "${!icons[@]}"; do
            json_array+="\"${icons[$i]}\""
            if [ $i -lt $((${#icons[@]} - 1)) ]; then
                json_array+=","
            fi
        done
        json_array+="]"
        echo "$json_array"
    fi
}

# Main execution
BINARY_PATH=$(find_runner_binary)
if [ -z "$BINARY_PATH" ]; then
    echo "Error: GitHub Actions Runner binary not found" >&2
    exit 1
fi

BINARY_NAME=$(basename "$BINARY_PATH")
RUNNER_DIR=$(dirname "$BINARY_PATH")

# Get version - try multiple sources in order
VERSION=$(get_runner_version_from_binary "$BINARY_PATH")
[ -z "$VERSION" ] && VERSION=$(get_version_from_deps_json "$RUNNER_DIR")
[ -z "$VERSION" ] && VERSION=$(get_package_version)

# Get display name - try multiple sources
DISPLAY_NAME=$(get_display_name_from_desktop)
[ -z "$DISPLAY_NAME" ] && DISPLAY_NAME=$(get_display_name_from_package)
[ -z "$DISPLAY_NAME" ] && DISPLAY_NAME=$(get_display_name_from_binary "$BINARY_PATH")

DESKTOP_ENTRY=$(find_desktop_entry)
ICON_PATHS=$(find_icon_paths)

# Output JSON
cat <<EOF
{
  "binary_path": "$BINARY_PATH",
  "binary_name": "$BINARY_NAME",
  "display_name": "$DISPLAY_NAME",
  "desktop_entry": $DESKTOP_ENTRY,
  "icon_paths": $ICON_PATHS,
  "version": "$VERSION"
}
EOF