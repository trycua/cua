#!/bin/bash

# Extract metadata for IntelliJ IDEA - dynamically parsing from desktop files and package info

# Find binary path using which first, then check package manager
BINARY_PATH=$(which idea 2>/dev/null || which ideaIC 2>/dev/null || which intellij 2>/dev/null || echo "")

# If not found in PATH, check package manager installation locations
if [ -z "$BINARY_PATH" ]; then
    # Check if installed via package manager
    if command -v dpkg &> /dev/null && dpkg -l 2>/dev/null | grep -q intellij-idea; then
        BINARY_PATH=$(dpkg -L intellij-idea 2>/dev/null | grep 'bin/idea' | head -1 || echo "")
    fi
    # Fallback to direct installation location
    if [ -z "$BINARY_PATH" ] && [ -x "/opt/intellij-idea/bin/idea.sh" ]; then
        BINARY_PATH="/opt/intellij-idea/bin/idea.sh"
    fi
fi

BINARY_NAME=$(basename "$BINARY_PATH" 2>/dev/null || echo "idea")

# Get version from the binary
VERSION=$("$BINARY_PATH" --version 2>&1 | grep -i "IntelliJ IDEA" | grep -oE "[0-9]+\.[0-9]+\.[0-9]+(\.[0-9]+)?" | head -1 || echo "")

# Find desktop entry by searching in standard locations
DESKTOP_ENTRY=""
for desktop_dir in /usr/share/applications ~/.local/share/applications /usr/local/share/applications; do
    if [ -d "$desktop_dir" ]; then
        # Look for IntelliJ IDEA related desktop files
        while IFS= read -r found_desktop; do
            [ -n "$found_desktop" ] && DESKTOP_ENTRY="$found_desktop" && break 2
        done < <(find "$desktop_dir" -maxdepth 1 -type f \( -name "*idea*.desktop" -o -name "*intellij*.desktop" \) 2>/dev/null)
    fi
done

# Parse display name from desktop entry if found
DISPLAY_NAME=""
if [ -n "$DESKTOP_ENTRY" ] && [ -f "$DESKTOP_ENTRY" ]; then
    DISPLAY_NAME=$(grep "^Name=" "$DESKTOP_ENTRY" | head -1 | sed 's/^Name=//' || echo "")
fi

# Fallback display name if not found in desktop file
if [ -z "$DISPLAY_NAME" ]; then
    DISPLAY_NAME="IntelliJ IDEA Community Edition"
fi

# Find icon paths - search in multiple locations and deduplicate
declare -a ICON_PATHS_ARRAY
declare -A ICON_SEEN

# Search for icon files in standard locations
for icon_dir in /usr/share/icons /opt/intellij-idea /opt/intellij-idea/bin /opt/intellij-idea/lib ~/.local/share/icons; do
    if [ -d "$icon_dir" ]; then
        # Look for PNG, SVG, and ICO files
        while IFS= read -r icon_file; do
            if [ -n "$icon_file" ] && [ -z "${ICON_SEEN[$icon_file]}" ]; then
                ICON_PATHS_ARRAY+=("$icon_file")
                ICON_SEEN["$icon_file"]=1
            fi
        done < <(find "$icon_dir" -type f \( -name "idea*.png" -o -name "idea*.svg" -o -name "*intellij*.png" -o -name "*intellij*.svg" \) 2>/dev/null | head -10)
    fi
done

# Build JSON output with proper formatting
printf '{\n'
printf '  "binary_path": "%s",\n' "$BINARY_PATH"
printf '  "binary_name": "%s",\n' "$BINARY_NAME"
printf '  "display_name": "%s",\n' "$DISPLAY_NAME"
printf '  "desktop_entry": "%s",\n' "$DESKTOP_ENTRY"
printf '  "icon_paths": ['
if [ ${#ICON_PATHS_ARRAY[@]} -gt 0 ]; then
    for i in "${!ICON_PATHS_ARRAY[@]}"; do
        printf '"%s"' "${ICON_PATHS_ARRAY[$i]}"
        if [ $i -lt $((${#ICON_PATHS_ARRAY[@]} - 1)) ]; then
            printf ', '
        fi
    done
fi
printf '],\n'
printf '  "version": "%s"\n' "$VERSION"
printf '}\n'