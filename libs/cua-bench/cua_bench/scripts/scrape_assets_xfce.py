#!/usr/bin/env python3
"""
Script to scan applications and generate JSON + CSS files with icons
Only includes apps that have icons
"""

import base64
import glob
import io
import json
import os

from PIL import Image


def resize_icon(icon_path, max_size=64):
    """Resize icon to max 64x64 and return as base64"""
    try:
        with Image.open(icon_path) as img:
            # Convert to RGBA if needed
            if img.mode != "RGBA":
                img = img.convert("RGBA")

            # Calculate new size maintaining aspect ratio
            width, height = img.size
            if width > max_size or height > max_size:
                if width > height:
                    new_width = max_size
                    new_height = int(height * (max_size / width))
                else:
                    new_height = max_size
                    new_width = int(width * (max_size / height))

                img = img.resize((new_width, new_height))

            # Save to bytes
            buffer = io.BytesIO()
            img.save(buffer, format="PNG", optimize=True)
            buffer.seek(0)

            # Encode as base64
            encoded = base64.b64encode(buffer.read()).decode("utf-8")
            return f"data:image/png;base64,{encoded}"
    except Exception as e:
        print(f"  Error resizing {icon_path}: {e}")
        return None


def find_icon(icon_name, icon_theme="elementray-xfce-dark"):
    """Find icon file in system icon directories"""
    if not icon_name:
        return None

    # If it's already a full path
    if os.path.isabs(icon_name) and os.path.exists(icon_name):
        return icon_name

    # Remove file extension if present
    icon_base = os.path.splitext(icon_name)[0]

    # Common icon directories
    icon_dirs = [
        f"/usr/share/icons/{icon_theme}",
        "/usr/share/icons/hicolor",
        "/usr/share/icons/gnome",
        "/usr/share/icons/default",
        "/usr/share/pixmaps",
        os.path.expanduser("~/.local/share/icons"),
        os.path.expanduser("~/.icons"),
    ]

    # Common sizes to check (prefer larger ones first for better quality)
    sizes = ["256x256", "128x128", "96x96", "64x64", "48x48", "32x32", "scalable"]
    extensions = [".png", ".svg", ".xpm", ".jpg"]

    for icon_dir in icon_dirs:
        if not os.path.exists(icon_dir):
            continue

        # Try different sizes
        for size in sizes:
            size_dir = os.path.join(icon_dir, size, "apps")
            if os.path.exists(size_dir):
                for ext in extensions:
                    icon_path = os.path.join(size_dir, icon_base + ext)
                    if os.path.exists(icon_path):
                        return icon_path

        # Try direct search in icon directory
        for ext in extensions:
            for pattern in [f"**/{icon_base}{ext}", f"{icon_base}{ext}"]:
                matches = glob.glob(os.path.join(icon_dir, pattern), recursive=True)
                if matches:
                    # Prefer non-symbolic icons
                    for match in matches:
                        if "symbolic" not in match:
                            return match
                    return matches[0]

    # Last resort: search pixmaps directly
    for ext in extensions:
        pixmap_path = f"/usr/share/pixmaps/{icon_base}{ext}"
        if os.path.exists(pixmap_path):
            return pixmap_path

    return None


def parse_desktop_file(filepath):
    """Parse a .desktop file and extract relevant info"""
    info = {"name": None, "icon": None, "comment": None}

    try:
        with open(filepath, "r", encoding="utf-8", errors="ignore") as f:
            in_desktop_entry = False
            for line in f:
                line = line.strip()

                if line == "[Desktop Entry]":
                    in_desktop_entry = True
                    continue

                if line.startswith("[") and line != "[Desktop Entry]":
                    break

                if not in_desktop_entry:
                    continue

                if line.startswith("Name=") and not line.startswith("Name["):
                    info["name"] = line.split("=", 1)[1].strip()
                elif line.startswith("Icon="):
                    info["icon"] = line.split("=", 1)[1].strip()
                elif line.startswith("Comment=") and not line.startswith("Comment["):
                    info["comment"] = line.split("=", 1)[1].strip()
    except Exception:
        pass

    return info


def get_all_applications():
    """Get all applications with their icons"""
    apps = []
    search_paths = [
        "/usr/share/applications",
        "/usr/local/share/applications",
        os.path.expanduser("~/.local/share/applications"),
        "/var/lib/snapd/desktop/applications",
        "/var/lib/flatpak/exports/share/applications",
        os.path.expanduser("~/.local/share/flatpak/exports/share/applications"),
    ]

    print("Scanning for applications...")

    for path in search_paths:
        if os.path.exists(path):
            desktop_files = glob.glob(os.path.join(path, "*.desktop"))
            for desktop_file in desktop_files:
                info = parse_desktop_file(desktop_file)
                if info["name"]:
                    apps.append(info)

    return apps


def generate_css(apps_with_icons):
    """Generate CSS file with icon data URLs"""
    css_content = (
        "/* Generated by scrape_assets_xfce.py — app icons mapped to data URLs (max 64x64) */\n\n"
    )
    css_content += "/* Application Icons */\n\n"

    for app in apps_with_icons:
        # Sanitize name for CSS class
        safe_name = app["name"].replace(" ", "-").replace("/", "-").replace("\\", "-")
        safe_name = "".join(c for c in safe_name if c.isalnum() or c == "-")

        css_content += f'.icon[data-icon="{app["name"]}"] {{\n'
        css_content += f'  background-image: url({app["icon_data"]});\n'
        css_content += "}\n\n"

    return css_content


def generate_json(apps_with_icons):
    """Generate JSON file with application list"""
    app_names = [app["name"] for app in apps_with_icons]

    json_data = {"icons": {"application_icons": sorted(app_names)}}

    return json.dumps(json_data, indent=2, ensure_ascii=False)


def main():
    print("=" * 70)
    print("APPLICATION SCANNER - JSON & CSS GENERATOR")
    print("=" * 70)
    print()

    # Get all applications
    apps = get_all_applications()

    # Remove duplicates by name
    unique_apps = {}
    for app in apps:
        if app["name"] not in unique_apps:
            unique_apps[app["name"]] = app

    apps_list = list(unique_apps.values())
    print(f"Found {len(apps_list)} unique applications")
    print()

    print(apps)
    print(apps_list)

    # Filter apps with icons and process them
    print("Processing icons (this may take a moment)...")
    apps_with_icons = []

    for i, app in enumerate(apps_list, 1):
        if i % 10 == 0:
            print(f"  Processing {i}/{len(apps_list)}...")

        if app["icon"]:
            icon_path = find_icon(app["icon"])
            if icon_path:
                # Skip SVG files as PIL doesn't handle them well
                if icon_path.endswith(".svg"):
                    # Try to find a PNG version instead
                    icon_name = os.path.splitext(os.path.basename(icon_path))[0]
                    png_path = find_icon(icon_name)
                    if png_path and not png_path.endswith(".svg"):
                        icon_path = png_path
                    else:
                        continue

                icon_data = resize_icon(icon_path)
                if icon_data:
                    apps_with_icons.append(
                        {"name": app["name"], "icon_data": icon_data, "comment": app["comment"]}
                    )

    print(f"\nFound {len(apps_with_icons)} applications with valid icons")
    print()

    # Sort alphabetically
    apps_with_icons = sorted(apps_with_icons, key=lambda x: x["name"].lower())

    # Generate CSS
    print("Generating xfce.css...")
    css_content = generate_css(apps_with_icons)
    with open("xfce.css", "w", encoding="utf-8") as f:
        f.write(css_content)

    # Generate JSON
    print("Generating xfce.json...")
    json_content = generate_json(apps_with_icons)
    with open("xfce.json", "w", encoding="utf-8") as f:
        f.write(json_content)

    # Generate HTML for preview
    print("Generating results.html...")
    html_content = f"""<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Applications with Icons</title>
    <link rel="stylesheet" href="xfce.css">
    <style>
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}

        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            padding: 20px;
            min-height: 100vh;
        }}

        .container {{
            max-width: 1400px;
            margin: 0 auto;
            background: white;
            border-radius: 20px;
            box-shadow: 0 20px 60px rgba(0,0,0,0.3);
            padding: 40px;
        }}

        h1 {{
            text-align: center;
            color: #333;
            margin-bottom: 10px;
            font-size: 2.5em;
        }}

        .subtitle {{
            text-align: center;
            color: #666;
            margin-bottom: 40px;
            font-size: 1.1em;
        }}

        .stats {{
            text-align: center;
            margin-bottom: 30px;
            padding: 15px;
            background: #f0f0f0;
            border-radius: 10px;
            color: #555;
            font-weight: 600;
        }}

        .grid {{
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
            gap: 20px;
            margin-top: 30px;
        }}

        .app-card {{
            background: #f8f9fa;
            border-radius: 12px;
            padding: 20px;
            text-align: center;
            transition: all 0.3s ease;
            cursor: pointer;
            border: 2px solid transparent;
        }}

        .app-card:hover {{
            transform: translateY(-5px);
            box-shadow: 0 10px 25px rgba(0,0,0,0.15);
            border-color: #667eea;
            background: #fff;
        }}

        .icon {{
            width: 64px;
            height: 64px;
            margin: 0 auto 15px;
            background-size: contain;
            background-position: center;
            background-repeat: no-repeat;
        }}

        .app-name {{
            font-weight: 600;
            color: #333;
            font-size: 0.95em;
            margin-bottom: 5px;
            word-wrap: break-word;
        }}

        .app-comment {{
            font-size: 0.8em;
            color: #777;
            margin-top: 8px;
            line-height: 1.3;
            display: -webkit-box;
            -webkit-line-clamp: 2;
            -webkit-box-orient: vertical;
            overflow: hidden;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>🚀 Applications with Icons</h1>
        <p class="subtitle">All applications with valid icons found on your system</p>
        <div class="stats">
            Total Applications with Icons: {len(apps_with_icons)}
        </div>
        <div class="grid">
"""

    for app in apps_with_icons:
        html_content += '            <div class="app-card">\n'
        html_content += f'                <div class="icon" data-icon="{app["name"]}"></div>\n'
        html_content += f'                <div class="app-name">{app["name"]}</div>\n'
        if app["comment"]:
            html_content += f'                <div class="app-comment">{app["comment"]}</div>\n'
        html_content += "            </div>\n"

    html_content += """        </div>
    </div>
</body>
</html>"""

    with open("results.html", "w", encoding="utf-8") as f:
        f.write(html_content)

    print()
    print("=" * 70)
    print("✓ Files generated successfully:")
    print(f"  - xfce.css ({len(apps_with_icons)} icon rules)")
    print(f"  - xfce.json (list of {len(apps_with_icons)} applications)")
    print("  - results.html (preview page)")
    print()
    print("Open results.html in your browser to preview!")
    print("=" * 70)


if __name__ == "__main__":
    main()
