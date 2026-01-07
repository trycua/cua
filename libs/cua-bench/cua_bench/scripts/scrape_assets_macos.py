#!/usr/bin/env python3
"""
Scrape installed macOS applications and generate a CSS file that maps
`.icon[data-icon="{Application Name}"]` to a data URL background image.

- Scans: /Applications, /System/Applications, ~/Applications
- For each .app bundle, reads Info.plist for name and icon
- Converts .icns to PNG via `sips` (preferred) or Pillow if available
- Filters out entries with no icon or fully transparent icons
- Resizes icons to a max of 64x64
- Emits CSS to `cua_bench/www/iconsets/macos.css`

Run:
  python cua_bench/scripts/scrape_assets.py
"""
from __future__ import annotations

import base64
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import List, Optional, Tuple

# Optional progress bars
try:
    from tqdm import tqdm as _tqdm  # type: ignore
except Exception:
    _tqdm = None  # graceful fallback if tqdm isn't installed

def pbar(iterable, desc: str):
    return _tqdm(iterable, desc=desc) if _tqdm else iterable

# -------------- Config --------------
APPLICATION_DIRS = [
    Path("/Applications"),
    Path("/System/Applications"),
    Path.home() / "Applications",
]

OUTPUT_CSS = Path(__file__).parents[1] / "www" / "iconsets" / "macos.css"
OUTPUT_JSON = Path(__file__).parents[1] / "www" / "iconsets" / "macos.json"

# Additional (optional) system icons source (unused for CSS rules keyed by application name but kept for future use)
SYSTEM_ICONS_DIR = Path("/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/")

# -------------- Helpers --------------

def is_tool(name: str) -> bool:
    return shutil.which(name) is not None


def find_app_bundles(search_dirs: List[Path]) -> List[Path]:
    apps: List[Path] = []
    seen = set()
    for root in search_dirs:
        if not root.exists():
            continue
        # walk filesystem, but when we see a .app, record and don't descend into it further
        for dirpath, dirnames, filenames in os.walk(root):
            p = Path(dirpath)
            # If current directory itself is a .app, record and skip children
            if p.suffix.lower() == ".app":
                if p not in seen:
                    apps.append(p)
                    seen.add(p)
                # prevent walking into bundle contents
                dirnames[:] = []
                continue
            # Otherwise, filter immediate children that are .app so we can skip descending into them
            # (os.walk will descend unless we remove from dirnames)
            to_remove = []
            for d in dirnames:
                if d.lower().endswith('.app'):
                    app_path = p / d
                    if app_path not in seen:
                        apps.append(app_path)
                        seen.add(app_path)
                    to_remove.append(d)
            # remove .app dirs from descent
            for d in to_remove:
                try:
                    dirnames.remove(d)
                except ValueError:
                    pass
    # Sort by name for stable output
    apps.sort(key=lambda x: x.name.lower())
    return apps


def read_info_plist(app_path: Path) -> Optional[dict]:
    plist_path = app_path / "Contents" / "Info.plist"
    if not plist_path.exists():
        return None
    try:
        with plist_path.open('rb') as f:
            return plistlib.load(f)
    except Exception:
        return None


def get_app_display_name(info: dict, app_path: Path) -> str:
    # Prefer CFBundleDisplayName, then CFBundleName, otherwise fallback to bundle dir name without .app
    name = info.get("CFBundleDisplayName") or info.get("CFBundleName")
    if not name:
        name = app_path.stem
    return str(name)


def resolve_icon_candidates(info: dict, app_path: Path) -> List[Path]:
    resources = app_path / "Contents" / "Resources"
    candidates: List[Path] = []

    def add_icon_name(name: str):
        if not name:
            return
        base = Path(name)
        if base.suffix == '':
            # try common suffixes
            possible = [base.with_suffix('.icns'), base.with_suffix('.png')]
        else:
            possible = [base]
        for p in possible:
            cand = resources / p
            if cand.exists():
                candidates.append(cand)

    # Older key
    icon_file = info.get("CFBundleIconFile")
    if isinstance(icon_file, str):
        add_icon_name(icon_file)

    # Newer key
    icon_name = info.get("CFBundleIconName")
    if isinstance(icon_name, str):
        add_icon_name(icon_name)

    # CFBundleIcons structure (iOS/macCatalyst style sometimes appears)
    icons = info.get("CFBundleIcons") or info.get("CFBundleIcons~mac")
    if isinstance(icons, dict):
        primary = icons.get("CFBundlePrimaryIcon")
        if isinstance(primary, dict):
            files = primary.get("CFBundleIconFiles")
            if isinstance(files, list):
                for f in files:
                    if isinstance(f, str):
                        add_icon_name(f)

    # Fallback: pick any .icns in Resources
    if resources.exists():
        for p in resources.glob("*.icns"):
            candidates.append(p)

    # Deduplicate preserving order
    seen: set[Path] = set()
    uniq: List[Path] = []
    for c in candidates:
        if c not in seen:
            uniq.append(c)
            seen.add(c)
    return uniq

def has_opaque_pixels_path(path: Path) -> bool:
    """Return True if the image has any opaque (alpha > 0) pixel.
    If Pillow is not available or the format lacks alpha, assume True (opaque).
    """
    try:
        from PIL import Image  # type: ignore
        with Image.open(path) as im:
            # JPEG and many formats without alpha are fully opaque
            if im.mode in ("RGB", "L", "P") and (im.mode != "P" or not hasattr(im, "info") or "transparency" not in im.info):
                return True
            # Convert to RGBA to inspect alpha
            im = im.convert("RGBA")
            alpha = im.getchannel("A")
            # If any pixel alpha > 0, treat as has opaque content
            extrema = alpha.getextrema()  # (min, max)
            return bool(extrema and extrema[1] > 0)
    except Exception:
        # If Pillow isn't available or fails, default to keeping the image
        return True


def resize_image_file_to_max(path: Path, max_px: int = 64) -> Path:
    """Return path to an image resized to fit within max_px box. Uses sips if available, else Pillow.
    May return the original path if resizing not needed or not possible.
    The returned path may be a temp file; caller should read and then allow cleanup.
    """
    # Try sips first
    if is_tool('sips'):
        with tempfile.TemporaryDirectory() as td:
            out_path = Path(td) / (path.stem + f"_{max_px}.png")
            try:
                # -Z maintains aspect ratio, sets the larger dimension to max_px
                subprocess.run([
                    "sips", "-s", "format", "png", str(path), "-Z", str(max_px), "--out", str(out_path)
                ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if out_path.exists():
                    # read bytes then write to a persistent temp to return
                    data = out_path.read_bytes()
                    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
                        tmp.write(data)
                        return Path(tmp.name)
            except Exception:
                pass
    # Pillow fallback
    try:
        from PIL import Image  # type: ignore
        with Image.open(path) as im:
            im = im.convert("RGBA")
            im.thumbnail((max_px, max_px))
            with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
                im.save(tmp.name, format='PNG')
                return Path(tmp.name)
    except Exception:
        pass
    return path


def icns_to_png_dataurl(icns_path: Path) -> Optional[str]:
    """Convert .icns to PNG and return data URL. Prefers macOS `sips` if present; else tries Pillow.
    """
    # Prefer sips for reliability on macOS
    if is_tool('sips'):
        with tempfile.TemporaryDirectory() as td:
            out_png = Path(td) / (icns_path.stem + ".png")
            try:
                # `sips -s format png in.icns --out out.png`
                subprocess.run(
                    ["sips", "-s", "format", "png", str(icns_path), "--out", str(out_png)],
                    check=True,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                if out_png.exists():
                    # Resize and filter transparency
                    resized = resize_image_file_to_max(out_png, 64)
                    if not has_opaque_pixels_path(resized):
                        try:
                            if resized != out_png:
                                Path(resized).unlink(missing_ok=True)  # type: ignore[arg-type]
                        except Exception:
                            pass
                        return None
                    data = Path(resized).read_bytes()
                    b64 = base64.b64encode(data).decode('ascii')
                    try:
                        if resized != out_png:
                            Path(resized).unlink(missing_ok=True)  # type: ignore[arg-type]
                    except Exception:
                        pass
                    return f"data:image/png;base64,{b64}"
            except Exception:
                pass

    # Fallback: Pillow (if available and supports ICNS on this system)
    try:
        from PIL import Image  # type: ignore
        im = Image.open(icns_path)
        # Pick largest available size frame
        if hasattr(im, 'n_frames') and im.n_frames > 1:
            # try last frame as largest
            im.seek(im.n_frames - 1)
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
            tmp_path = Path(tmp.name)
        try:
            im.save(tmp_path, format='PNG')
            # Resize and filter transparency
            resized = resize_image_file_to_max(tmp_path, 64)
            if not has_opaque_pixels_path(resized):
                return None
            data = Path(resized).read_bytes()
            b64 = base64.b64encode(data).decode('ascii')
            return f"data:image/png;base64,{b64}"
        finally:
            try:
                tmp_path.unlink(missing_ok=True)  # type: ignore[arg-type]
            except Exception:
                pass
    except Exception:
        pass

    return None


def any_image_to_dataurl(path: Path) -> Optional[str]:
    ext = path.suffix.lower()
    if ext == ".icns":
        return icns_to_png_dataurl(path)
    # If PNG or JPEG already
    try:
        mime = {
            ".png": "image/png",
            ".jpg": "image/jpeg",
            ".jpeg": "image/jpeg",
            ".gif": "image/gif",
            ".webp": "image/webp",
        }.get(ext)
        if mime:
            # For formats that can be transparent, skip if no opaque pixels
            candidate = path
            if ext in {".png", ".gif", ".webp"} and not has_opaque_pixels_path(path):
                return None
            # Resize to max 64
            resized = resize_image_file_to_max(candidate, 64)
            data = Path(resized).read_bytes()
            b64 = base64.b64encode(data).decode('ascii')
            return f"data:{mime};base64,{b64}"
    except Exception:
        return None
    return None


def collect_app_info(app_path: Path) -> Optional[Tuple[str, Optional[str]]]:
    info = read_info_plist(app_path)
    if not info:
        return None
    name = get_app_display_name(info, app_path)
    icon_dataurl: Optional[str] = None

    for cand in resolve_icon_candidates(info, app_path):
        icon_dataurl = any_image_to_dataurl(cand)
        if icon_dataurl:
            break

    return name, icon_dataurl


def collect_system_icons() -> List[Tuple[str, Optional[str], Path]]:
    entries: List[Tuple[str, Optional[str], Path]] = []
    if not SYSTEM_ICONS_DIR.exists():
        return entries
    # common types are .icns and some .png
    system_files = sorted(SYSTEM_ICONS_DIR.glob("*"))
    for p in pbar(system_files, "System icons"):
        if p.is_file() and p.suffix.lower() in {".icns", ".png", ".jpg", ".jpeg", ".gif", ".webp"}:
            dataurl = any_image_to_dataurl(p)
            if dataurl:
                name = p.stem
                entries.append((name, dataurl, p))
    return entries


 


def css_escape_attr(value: str) -> str:
    """Escape a string for safe use inside a CSS attribute value in double quotes."""
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ")


def main() -> int:
    apps = find_app_bundles(APPLICATION_DIRS)

    entries: List[Tuple[str, Optional[str], Path]] = []
    for app in pbar(apps, "Apps"):
        try:
            info = collect_app_info(app)
            if not info:
                continue
            name, dataurl = info
            # Only include apps that have an icon
            if dataurl:
                entries.append((name, dataurl, app))
        except Exception:
            # Be resilient: skip problematic apps
            continue

    # Sort by name
    entries.sort(key=lambda x: x[0].lower())

    # Build CSS content
    OUTPUT_CSS.parent.mkdir(parents=True, exist_ok=True)
    lines: List[str] = [
        "/* Generated by scrape_assets.py — app icons mapped to data URLs (max 64x64) */",
        "",
        "/* System Icons */",
        "",
    ]
    # Collect and emit System Icons first
    system_entries = collect_system_icons()
    system_entries.sort(key=lambda x: x[0].lower())
    for name, dataurl, _ in pbar(system_entries, "System CSS"):
        if not dataurl:
            continue
        sel_name = css_escape_attr(name)
        lines.append(
            f".icon[data-icon=\"{sel_name}\"] {{\n  background-image: url({dataurl}) !important;\n}}"
        )
    # Spacer before application section rules
    lines.extend([
        "/* Application Icons */",
        "",
    ])
    for name, dataurl, _ in pbar(entries, "CSS rules"):
        if not dataurl:
            continue
        sel_name = css_escape_attr(name)
        lines.append(
            f".icon[data-icon=\"{sel_name}\"] {{\n  background-image: url({dataurl}) !important;\n}}"
        )

    # Write CSS
    OUTPUT_CSS.write_text("\n\n".join(lines) + "\n", encoding="utf-8")

    # Write JSON with icon name lists
    data = {
        "icons": {
            "system_icons": [name for name, _dataurl, _ in system_entries],
            "application_icons": [name for name, _dataurl, _ in entries],
        }
    }
    OUTPUT_JSON.write_text(json.dumps(data, indent=2), encoding="utf-8")

    print(
        f"Wrote {OUTPUT_CSS} with {len(entries)} app icon rules and {len(system_entries)} system icon rules.\n"
        f"Wrote {OUTPUT_JSON} with icon name lists."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
