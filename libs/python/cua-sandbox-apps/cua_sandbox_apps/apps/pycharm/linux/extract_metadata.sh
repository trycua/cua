#!/bin/bash

# Extract PyCharm metadata script
# Dynamically discovers PyCharm installation

python3 << 'PYTHON_EOF'
import json
import subprocess
import os
import glob
import re

metadata = {
    "binary_path": None,
    "binary_name": None,
    "display_name": "PyCharm Community Edition",
    "desktop_entry": None,
    "icon_paths": [],
    "version": None
}

# Try to find binary using which
try:
    result = subprocess.run(["which", "pycharm"], capture_output=True, text=True)
    if result.returncode == 0:
        metadata["binary_path"] = result.stdout.strip()
        metadata["binary_name"] = os.path.basename(metadata["binary_path"])
except:
    pass

# If not found, try pycharm-community
if not metadata["binary_path"]:
    try:
        result = subprocess.run(["which", "pycharm-community"], capture_output=True, text=True)
        if result.returncode == 0:
            metadata["binary_path"] = result.stdout.strip()
            metadata["binary_name"] = os.path.basename(metadata["binary_path"])
    except:
        pass

# If still not found, search for pycharm.sh in common locations
if not metadata["binary_path"]:
    possible_paths = [
        "/opt/pycharm/bin/pycharm.sh",
        "/usr/local/bin/pycharm",
        "/snap/pycharm-community/current/bin/pycharm",
        "/snap/bin/pycharm-community",
    ]
    for path in possible_paths:
        if os.path.exists(path) and os.access(path, os.X_OK):
            metadata["binary_path"] = path
            metadata["binary_name"] = os.path.basename(path)
            break

# If still not found, search in /opt
if not metadata["binary_path"]:
    for pycharm_dir in glob.glob("/opt/pycharm*/bin/pycharm*"):
        if os.access(pycharm_dir, os.X_OK):
            metadata["binary_path"] = pycharm_dir
            metadata["binary_name"] = os.path.basename(pycharm_dir)
            break

# Follow symlink to get the real path for directory traversal
real_binary_path = metadata["binary_path"]
if metadata["binary_path"] and os.path.islink(metadata["binary_path"]):
    real_binary_path = os.path.realpath(metadata["binary_path"])

# Search for .desktop file
desktop_files = glob.glob("/usr/share/applications/*pycharm*.desktop")
if desktop_files:
    metadata["desktop_entry"] = desktop_files[0]
    
    # Parse desktop file to extract information
    try:
        with open(metadata["desktop_entry"], 'r') as f:
            content = f.read()
            
            # Extract display name
            name_match = re.search(r'Name=(.*)', content)
            if name_match:
                metadata["display_name"] = name_match.group(1).strip()
            
            # Extract icon path
            icon_match = re.search(r'Icon=(.*)', content)
            if icon_match:
                icon_name = icon_match.group(1).strip()
                
                # If it's already a full path
                if icon_name.startswith('/'):
                    if os.path.exists(icon_name):
                        metadata["icon_paths"].append(icon_name)
                else:
                    # Search for icon in standard locations
                    search_patterns = [
                        f"/usr/share/icons/hicolor/*/apps/{icon_name}.png",
                        f"/usr/share/icons/hicolor/*/apps/{icon_name}.svg",
                        f"/usr/share/pixmaps/{icon_name}.png",
                        f"/usr/share/pixmaps/{icon_name}.svg",
                        f"/snap/pycharm-community/current/lib/{icon_name}.png",
                    ]
                    for pattern in search_patterns:
                        found = glob.glob(pattern)
                        if found:
                            metadata["icon_paths"].extend(found)
    except:
        pass

# Get version from the binary
if metadata["binary_path"]:
    try:
        # Try running with --version flag
        result = subprocess.run([metadata["binary_path"], "--version"], 
                              capture_output=True, text=True, timeout=5)
        if result.stdout or result.stderr:
            output = (result.stdout + result.stderr).strip()
            # Extract version number
            version_match = re.search(r'(\d+\.\d+[\.\.\d]*)', output)
            if version_match:
                metadata["version"] = version_match.group(1)
    except:
        pass

# Try to extract version from product-info.json
if not metadata["version"] and real_binary_path:
    try:
        install_dir = os.path.dirname(os.path.dirname(real_binary_path))
        product_info = os.path.join(install_dir, "product-info.json")
        if os.path.exists(product_info):
            with open(product_info, 'r') as f:
                try:
                    import json as json_lib
                    info = json_lib.load(f)
                    if "version" in info:
                        metadata["version"] = info["version"]
                except:
                    pass
    except:
        pass

# Try to extract from build.txt
if not metadata["version"] and real_binary_path:
    try:
        install_dir = os.path.dirname(os.path.dirname(real_binary_path))
        build_file = os.path.join(install_dir, "build.txt")
        if os.path.exists(build_file):
            with open(build_file, 'r') as f:
                content = f.read().strip()
                version_match = re.search(r'(\d+\.\d+[\.\.\d]*)', content)
                if version_match:
                    metadata["version"] = version_match.group(1)
    except:
        pass

# Search for icons in installation directory using os.walk
if real_binary_path:
    try:
        install_dir = os.path.dirname(os.path.dirname(real_binary_path))
        
        # Search for icon files using os.walk
        for root, dirs, files in os.walk(install_dir):
            for file in files:
                if ('pycharm' in file.lower()) and file.endswith(('.png', '.svg', '.ico')):
                    full_path = os.path.join(root, file)
                    metadata["icon_paths"].append(full_path)
            # Limit search depth to avoid huge directories
            if root.count(os.sep) - install_dir.count(os.sep) >= 3:
                dirs.clear()
    except:
        pass

# Remove duplicates and sort
metadata["icon_paths"] = sorted(list(set([p for p in metadata["icon_paths"] if p and os.path.exists(p)])))

# Fallback version
if not metadata["version"]:
    metadata["version"] = "unknown"

# Output JSON
print(json.dumps(metadata))
PYTHON_EOF