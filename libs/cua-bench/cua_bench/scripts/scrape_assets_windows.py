#!/usr/bin/env python3
"""
Scrape installed Windows applications and generate an HTML file (results.html)
listing each application's name with its icon.

- Scans Windows registry uninstall keys in HKLM/HKCU (both 32-bit and 64-bit views)
- Extracts DisplayName and attempts to resolve DisplayIcon
- Supports .ico directly; for .exe entries, tries best-effort fallbacks
- Converts icons to max 64x64 PNG data URLs when possible
- Emits HTML to `cua_bench/scripts/results.html`

Run:
  python cua_bench/scripts/scrape_assets_windows.py
"""
from __future__ import annotations

import base64
import html
import json
import os
import re
import sys
from pathlib import Path
from typing import Dict, List, Optional, Tuple

# Optional progress bars
try:
    from tqdm import tqdm as _tqdm  # type: ignore
except Exception:
    _tqdm = None


def pbar(iterable, desc: str):
    return _tqdm(iterable, desc=desc) if _tqdm else iterable


# Windows-only imports guarded
if os.name != "nt":
    print("This script is intended to run on Windows.")
    sys.exit(1)

try:
    import winreg  # type: ignore
except Exception:
    print("winreg is required on Windows Python.")
    sys.exit(1)

import icoextract  # type: ignore
import win32com.client  # type: ignore

# Output
OUTPUT_HTML = Path(__file__).parent / "results.html"
OUTPUT_CSS = Path(__file__).parents[1] / "www" / "iconsets" / "win11.css"
OUTPUT_JSON = Path(__file__).parents[1] / "www" / "iconsets" / "win11.json"

ICO_MAX_SIZE = 64
START_MENU_DIR = Path(r"C:\ProgramData\Microsoft\Windows\Start Menu")

# --- Image helpers ---


def _has_opaque_pixels_bytes(data: bytes) -> bool:
    try:
        from io import BytesIO

        from PIL import Image  # type: ignore

        with Image.open(BytesIO(data)) as im:
            if im.mode in ("RGB", "L", "P") and (
                im.mode != "P" or not hasattr(im, "info") or "transparency" not in im.info
            ):
                return True
            im = im.convert("RGBA")
            alpha = im.getchannel("A")
            extrema = alpha.getextrema()
            return bool(extrema and extrema[1] > 0)
    except Exception:
        return True


def _resize_image_bytes_to_png_dataurl(data: bytes, max_px: int = ICO_MAX_SIZE) -> Optional[str]:
    try:
        from io import BytesIO

        from PIL import Image  # type: ignore

        with Image.open(BytesIO(data)) as im:
            im = im.convert("RGBA")
            im.thumbnail((max_px, max_px))
            out = BytesIO()
            im.save(out, format="PNG")
            b64 = base64.b64encode(out.getvalue()).decode("ascii")
            return f"data:image/png;base64,{b64}"
    except Exception:
        return None


def ico_file_to_dataurl(path: Path) -> Optional[str]:
    try:
        data = path.read_bytes()
    except Exception:
        return None
    # Filter fully transparent
    if not _has_opaque_pixels_bytes(data):
        return None
    # Try Pillow resize -> PNG data URL; else fall back to embedding ICO
    png_url = _resize_image_bytes_to_png_dataurl(data, ICO_MAX_SIZE)
    if png_url:
        return png_url
    b64 = base64.b64encode(data).decode("ascii")
    return f"data:image/x-icon;base64,{b64}"


# --- Registry scanning ---
REG_PATHS = [
    (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    (winreg.HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
]

VALUE_DISPLAY_NAME = "DisplayName"
VALUE_DISPLAY_ICON = "DisplayIcon"
VALUE_INSTALL_LOCATION = "InstallLocation"


def _open_key(root, subkey):
    # Try both 64-bit and 32-bit views where applicable
    access_flags = [0]
    if hasattr(winreg, "KEY_WOW64_64KEY"):
        access_flags.append(winreg.KEY_WOW64_64KEY)
    if hasattr(winreg, "KEY_WOW64_32KEY"):
        access_flags.append(winreg.KEY_WOW64_32KEY)

    for flag in access_flags:
        try:
            return winreg.OpenKey(root, subkey, 0, winreg.KEY_READ | flag)
        except Exception:
            continue
    return None


def _enum_subkeys(key) -> List[str]:
    names: List[str] = []
    i = 0
    while True:
        try:
            name = winreg.EnumKey(key, i)
            names.append(name)
            i += 1
        except OSError:
            break
    return names


def _get_value(key, name: str) -> Optional[str]:
    try:
        val, _ = winreg.QueryValueEx(key, name)
        if isinstance(val, str):
            return val.strip() or None
        return None
    except Exception:
        return None


ICON_REF_RE = re.compile(r"^([^,]+)(?:,(\-?\d+))?$")


def _normalize_display_icon(display_icon: str) -> Tuple[Optional[Path], Optional[int]]:
    # Expand environment variables and strip quotes
    s = os.path.expandvars(display_icon).strip().strip('"')
    m = ICON_REF_RE.match(s)
    if not m:
        return None, None
    path_str, idx_str = m.groups()
    if not path_str:
        return None, None
    # Remove arguments after .exe path (e.g., app.exe" -foo)
    # If there are extra args, keep only up to .exe/.ico path
    path_only = path_str
    # In case of stray args, split on .exe or .ico and add extension back
    lowered = path_only.lower()
    if ".exe" in lowered:
        head = lowered.split(".exe", 1)[0]
        path_only = path_only[: len(head) + 4]
    elif ".dll" in lowered:
        head = lowered.split(".dll", 1)[0]
        path_only = path_only[: len(head) + 4]
    elif ".ico" in lowered:
        head = lowered.split(".ico", 1)[0]
        path_only = path_only[: len(head) + 4]
    p = Path(path_only)
    idx = int(idx_str) if idx_str else None
    return (p if p.exists() else None), idx


def _extract_png_dataurl_from_exe(exe_path: Path, icon_index: Optional[int]) -> Optional[str]:
    """Use icoextract to export an icon from an executable, then convert to PNG data URL."""
    if not exe_path.exists():
        return None
    # Export ICO to a temporary file using icoextract
    try:
        ie = icoextract.IconExtractor(str(exe_path))
    except Exception:
        return None
    import tempfile

    with tempfile.NamedTemporaryFile(suffix=".ico", delete=False) as tmp:
        tmp_ico = Path(tmp.name)
    if icon_index is not None:
        if icon_index < 0:
            ie.export_icon(str(tmp_ico), resource_id=-icon_index)
        else:
            ie.export_icon(str(tmp_ico), num=icon_index)
    else:
        # Default to the first/group icon
        ie.export_icon(str(tmp_ico))
    ico_bytes = tmp_ico.read_bytes()
    tmp_ico.unlink(missing_ok=True)
    # Validate and convert to PNG data URL (resized to <= 64)
    if not _has_opaque_pixels_bytes(ico_bytes):
        return None
    return _resize_image_bytes_to_png_dataurl(ico_bytes, ICO_MAX_SIZE)


def _find_nearby_ico(exe_or_dir: Path) -> Optional[Path]:
    candidates: List[Path] = []
    if exe_or_dir.is_file():
        d = exe_or_dir.parent
        stem = exe_or_dir.stem
        candidates.append(d / f"{stem}.ico")
        candidates.append(d / "icon.ico")
        candidates.append(d / "app.ico")
        # any .ico in the directory
        candidates.extend(sorted(d.glob("*.ico")))
    else:
        d = exe_or_dir
        candidates.extend(sorted(d.glob("*.ico")))
    for c in candidates:
        if c.exists():
            return c
    return None


def _resolve_icon_for_lnk(lnk_path: Path) -> Optional[str]:
    shell = win32com.client.Dispatch("WScript.Shell")
    sc = shell.CreateShortcut(str(lnk_path))
    # IconLocation can be "path,index"
    icon_loc = sc.IconLocation or ""
    dataurl: Optional[str] = None
    if icon_loc:
        p, idx = _normalize_display_icon(icon_loc)
        if p and p.suffix.lower() == ".ico" and p.exists():
            dataurl = ico_file_to_dataurl(p)
        elif p and p.suffix.lower() in {".exe", ".dll"} and p.exists():
            dataurl = _extract_png_dataurl_from_exe(p, idx)
            if not dataurl:
                cand = _find_nearby_ico(p)
                if cand:
                    dataurl = ico_file_to_dataurl(cand)
        if dataurl:
            return dataurl
    # Fallback to TargetPath
    target = sc.TargetPath or ""
    if target:
        tp = Path(target)
        if tp.exists():
            if tp.suffix.lower() == ".ico":
                return ico_file_to_dataurl(tp)
            if tp.suffix.lower() in {".exe", ".dll"}:
                dataurl = _extract_png_dataurl_from_exe(tp, None)
                if dataurl:
                    return dataurl
                cand = _find_nearby_ico(tp)
                if cand:
                    return ico_file_to_dataurl(cand)
    return None


def collect_start_menu_shortcuts(root: Path) -> List[Tuple[str, Optional[str], Path]]:
    if not root.exists():
        return []
    entries: List[Tuple[str, Optional[str], Path]] = []
    for lnk in root.rglob("*.lnk"):
        name = lnk.stem
        dataurl = _resolve_icon_for_lnk(lnk)
        entries.append((name, dataurl, lnk))
    # Sort by name
    entries.sort(key=lambda x: x[0].lower())
    return entries


def _collect_installed_apps() -> List[Dict[str, Optional[str]]]:
    results: Dict[str, Dict[str, Optional[str]]] = {}

    for root, subkey in REG_PATHS:
        key = _open_key(root, subkey)
        if not key:
            continue
        try:
            for name in _enum_subkeys(key):
                try:
                    sk = winreg.OpenKey(key, name)
                except Exception:
                    continue
                disp_name = _get_value(sk, VALUE_DISPLAY_NAME)
                if not disp_name:
                    continue
                disp_icon = _get_value(sk, VALUE_DISPLAY_ICON)
                install_loc = _get_value(sk, VALUE_INSTALL_LOCATION)

                # Prefer first occurrence; avoid duplicates across roots
                if disp_name not in results:
                    results[disp_name] = {
                        "DisplayName": disp_name,
                        "DisplayIcon": disp_icon,
                        "InstallLocation": install_loc,
                    }
        finally:
            try:
                key.Close()
            except Exception:
                pass

    # Return sorted by name
    items = list(results.values())
    items.sort(key=lambda x: (x.get("DisplayName") or "").lower())
    return items


def _resolve_icon_for_app(entry: Dict[str, Optional[str]]) -> Optional[str]:
    """Return a data URL for the icon if possible; else None.
    We try, in order:
      - DisplayIcon -> if .ico, embed
      - DisplayIcon -> if .exe, search for nearby .ico
      - InstallLocation -> search for .ico
    """
    disp_icon = entry.get("DisplayIcon") or ""
    install_loc = entry.get("InstallLocation") or ""

    # 1) Try DisplayIcon
    if disp_icon:
        p, idx = _normalize_display_icon(disp_icon)
        if p and p.suffix.lower() == ".ico" and p.exists():
            return ico_file_to_dataurl(p)
        if p and p.suffix.lower() in {".exe", ".dll"} and p.exists():
            # Attempt direct extraction from .exe first
            dataurl = _extract_png_dataurl_from_exe(p, idx)
            if dataurl:
                return dataurl
            # Fallback to nearby .ico
            cand = _find_nearby_ico(p)
            if cand:
                return ico_file_to_dataurl(cand)

    # 2) Try InstallLocation for .ico
    if install_loc:
        p = Path(os.path.expandvars(install_loc))
        if p.exists():
            cand = _find_nearby_ico(p)
            if cand:
                return ico_file_to_dataurl(cand)

    return None


def css_escape_attr(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ")


NAME_SUFFIX_RE = re.compile(r"\s+\((?:x64|User)\)$", re.IGNORECASE)


def normalize_app_name(name: str) -> str:
    """Normalize an application name for display/keys.
    - Remove trailing variants like " (x64)" or " (User)".
    - Collapse whitespace at ends.
    """
    if not name:
        return name
    cleaned = NAME_SUFFIX_RE.sub("", name).strip()
    return cleaned


def main() -> int:
    apps = _collect_installed_apps()

    entries: List[Tuple[str, Optional[str]]] = []
    for app in pbar(apps, "Apps"):
        name = app.get("DisplayName") or ""
        if not name:
            continue
        name = normalize_app_name(name)
        dataurl = _resolve_icon_for_app(app)
        entries.append((name, dataurl))

    # Also collect Start Menu shortcuts (.lnk) recursively
    lnk_entries = collect_start_menu_shortcuts(START_MENU_DIR)
    for name, dataurl, _ in pbar(lnk_entries, "StartMenu .lnk"):
        entries.append((normalize_app_name(name), dataurl))

    # Apply filters: skip names containing certain tokens, skip empty icons, and dedupe by icon content
    TOKENS = ("service", "redistributable", "runtime", "driver", "installer", "uninstall", "helper")
    seen_icons: set[str] = set()
    filtered_entries: List[Tuple[str, str]] = []
    for name, dataurl in entries:
        if not dataurl:
            continue
        lname = name.lower()
        if any(tok in lname for tok in TOKENS):
            continue
        if dataurl in seen_icons:
            continue
        seen_icons.add(dataurl)
        filtered_entries.append((name, dataurl))

    # Build HTML
    OUTPUT_HTML.parent.mkdir(parents=True, exist_ok=True)
    # Also ensure iconsets directory exists for CSS/JSON
    OUTPUT_CSS.parent.mkdir(parents=True, exist_ok=True)

    # Basic styles
    style = (
        "body{font-family:system-ui,-apple-system,Segoe UI,Roboto,Arial,sans-serif;padding:16px;}"
        ".grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:12px;}"
        ".card{display:flex;align-items:center;gap:10px;padding:8px 10px;border:1px solid #e0e0e0;border-radius:8px;background:#fff;box-shadow:0 1px 2px rgba(0,0,0,0.04);}"
        ".icon{width:32px;height:32px;background:#f5f5f5;border-radius:6px;display:flex;align-items:center;justify-content:center;overflow:hidden;}"
        ".icon img{max-width:100%;max-height:100%;display:block;}"
        ".name{font-size:14px;line-height:1.2;word-break:break-word;}"
        ".noicon{width:32px;height:32px;border-radius:6px;background:#f0f0f0;color:#999;display:flex;align-items:center;justify-content:center;font-size:11px;}"
    )

    rows: List[str] = []
    for name, dataurl in filtered_entries:
        safe = html.escape(name)
        icon_html = f'<div class="icon"><img src="{dataurl}" alt="{safe}"/></div>'
        rows.append(f'<div class="card">{icon_html}<div class="name">{safe}</div></div>')

    html_doc = f"""
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Installed Apps (Windows)</title>
<style>{style}</style>
</head>
<body>
<h1>Installed Apps (Windows)</h1>
<p>Total items (after filters & dedupe): {len(filtered_entries)}</p>
<div class="grid">
{os.linesep.join(rows)}
</div>
</body>
</html>
"""

    OUTPUT_HTML.write_text(html_doc, encoding="utf-8")
    print(f"Wrote {OUTPUT_HTML} with {len(filtered_entries)} items after filters & dedupe.")
    missing_icons = sum(1 for _n, d in entries if not d)
    print(f"Original items without icons (before filters): {missing_icons}")

    # Build CSS with data URLs
    css_lines: List[str] = [
        "/* Generated by scrape_assets_windows.py — app icons mapped to data URLs (max 64x64) */",
        "",
    ]
    for name, dataurl in filtered_entries:
        sel_name = css_escape_attr(name)
        css_lines.append(
            f'.icon[data-icon="{sel_name}"] {{\n  background-image: url({dataurl}) !important;\n}}'
        )
    OUTPUT_CSS.write_text("\n\n".join(css_lines) + "\n", encoding="utf-8")
    print(f"Wrote {OUTPUT_CSS} with {len(filtered_entries)} icon rules.")

    # Build JSON schema {"icons": {"application_icons": [...]}}
    data = {
        "icons": {
            "application_icons": [name for name, _ in filtered_entries],
        }
    }
    OUTPUT_JSON.write_text(json.dumps(data, indent=2), encoding="utf-8")
    print(f"Wrote {OUTPUT_JSON} with icon name list.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
