#!/bin/bash

# Extract PostgreSQL metadata

# Find main binary
binary_path=$(which psql)
binary_name=$(basename "$binary_path")

# Get version - extract version number from psql --version
version=$(psql --version 2>/dev/null | grep -oP '\) \K[0-9]+\.[0-9]+')

# Find desktop entry - search dynamically in /usr/share/applications
desktop_entry=""
display_name="PostgreSQL"

# Search for any .desktop file that contains postgres, postgresql, or psql
if [ -d "/usr/share/applications" ]; then
    # Use find with grep to search for postgres-related desktop files
    while IFS= read -r desktop_file; do
        if [ -f "$desktop_file" ]; then
            desktop_entry="$desktop_file"
            # Extract display name from the desktop file
            extracted_name=$(grep "^Name=" "$desktop_file" 2>/dev/null | head -1 | cut -d= -f2)
            if [ -n "$extracted_name" ]; then
                display_name="$extracted_name"
                break
            fi
        fi
    done < <(find /usr/share/applications -name "*.desktop" -exec grep -l "postgres\|postgresql\|psql\|database" {} \; 2>/dev/null)
fi

# If no desktop file found, try to extract package description as fallback
if [ -z "$desktop_entry" ]; then
    pkg_desc=$(dpkg -s postgresql 2>/dev/null | grep "^Description:" | head -1 | cut -d: -f2-)
    if [ -n "$pkg_desc" ]; then
        display_name="PostgreSQL$pkg_desc"
    fi
fi

# Find icon paths - look for PostgreSQL icons
icon_paths=()

# Check standard icon directories
for dir in /usr/share/icons /usr/share/pixmaps; do
    if [ -d "$dir" ]; then
        while IFS= read -r icon_file; do
            if [ -f "$icon_file" ]; then
                icon_paths+=("$icon_file")
            fi
        done < <(find "$dir" -type f \( -name "*postgres*" -o -name "*sql*" \) 2>/dev/null | head -10)
    fi
done

# If no specific icons found, try looking for the default PostgreSQL icon
if [ ${#icon_paths[@]} -eq 0 ]; then
    # Check common locations
    test -f "/usr/share/pixmaps/postgresql.xpm" && icon_paths+=("/usr/share/pixmaps/postgresql.xpm")
    test -f "/usr/share/pixmaps/postgresql.png" && icon_paths+=("/usr/share/pixmaps/postgresql.png")
fi

# Also try to extract icon from desktop file if it exists
if [ -n "$desktop_entry" ] && [ -f "$desktop_entry" ]; then
    desktop_icon=$(grep "^Icon=" "$desktop_entry" 2>/dev/null | head -1 | cut -d= -f2)
    if [ -n "$desktop_icon" ]; then
        # Try to locate the icon file
        for dir in /usr/share/icons /usr/share/pixmaps; do
            if [ -d "$dir" ]; then
                found_icon=$(find "$dir" -name "$desktop_icon*" 2>/dev/null | head -1)
                if [ -f "$found_icon" ]; then
                    # Check if already in list
                    if [[ ! " ${icon_paths[@]} " =~ " ${found_icon} " ]]; then
                        icon_paths+=("$found_icon")
                    fi
                fi
            fi
        done
    fi
fi

# Output as JSON
printf '{\n'
printf '  "binary_path": "%s",\n' "$binary_path"
printf '  "binary_name": "%s",\n' "$binary_name"
printf '  "display_name": "%s",\n' "$display_name"
printf '  "desktop_entry": "%s",\n' "$desktop_entry"
printf '  "icon_paths": [\n'

# Add icon paths to JSON
first=true
for icon in "${icon_paths[@]}"; do
    if [ "$first" = true ]; then
        printf '    "%s"\n' "$icon"
        first=false
    else
        printf '    ,"%s"\n' "$icon"
    fi
done

printf '  ],\n'
printf '  "version": "%s"\n' "$version"
printf '}\n'