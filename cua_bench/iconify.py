"""
Iconify icon processing module for cua_bench.

This module provides functionality to process HTML containing iconify-icon elements
and replace them with inline SVG content fetched from the Iconify API.

Key features:
- Processes <iconify-icon icon="prefix:name"> elements
- Supports custom icons.json for icon resolution
- Option to ignore icon set prefixes for randomization
- Caches SVG content for performance
- Preserves element attributes (width, height, class, etc.)
"""

import re
import json
from pathlib import Path
from typing import Optional, List, Dict
from urllib.request import urlopen
from urllib.error import URLError, HTTPError


# Global SVG cache for performance
_SVG_CACHE: Dict[str, str] = {}


def process_icons(
    html: str, 
    icons_json: Optional[str] = None, 
    ignore_iconset: bool = False
) -> str:
    """
    Process HTML containing iconify-icon elements and replace them with inline SVGs.
    
    Args:
        html: HTML content containing iconify-icon elements
        icons_json: Path to custom icons.json file. If None, uses default iconsets/icons.json
        ignore_iconset: If True, ignores the iconset prefix and searches for icon name only.
                       Useful for shuffling/randomizing icon sets. For example:
                       - eva:people-outline becomes */people-outline
                       - mingcute:ad-circle-line becomes */ad-circle-line
    
    Returns:
        HTML with iconify-icon elements replaced by inline SVG content
        
    Examples:
        >>> html = '<iconify-icon icon="eva:people-outline"></iconify-icon>'
        >>> process_icons(html)
        '<svg>...</svg>'
        
        >>> # With ignore_iconset=True for randomization
        >>> process_icons(html, ignore_iconset=True)  # May use different iconset
    """
    if not html:
        return html
    
    # Load icons index
    icons_list = _load_icons_index(icons_json)
    
    # Find all iconify-icon elements
    pattern = r'<iconify-icon\s+([^>]*?)(?:\s*/\s*>|>\s*</iconify-icon>)'
    
    def replace_iconify_element(match):
        attrs_str = match.group(1)
        
        # Parse attributes
        attrs = _parse_attributes(attrs_str)
        icon_name = attrs.get('icon', '')
        
        if not icon_name:
            return ''  # Remove element if no icon specified
        
        # Resolve icon key
        resolved_key = _resolve_icon_key(icon_name, icons_list, ignore_iconset)
        if not resolved_key:
            return ''  # Remove if cannot resolve
        
        # Fetch SVG
        svg_content = _fetch_icon_svg(resolved_key)
        if not svg_content:
            return ''  # Remove if cannot fetch
        
        # Apply attributes to SVG
        svg_content = _apply_attributes_to_svg(svg_content, attrs)
        
        return svg_content
    
    # Replace all iconify-icon elements
    result = re.sub(pattern, replace_iconify_element, html, flags=re.IGNORECASE)
    return result


def _load_icons_index(icons_json: Optional[str] = None) -> List[str]:
    """Load the icons index list from icons.json file."""
    if icons_json is None:
        # Use default path
        icons_path = Path(__file__).parent / 'www' / 'iconsets' / 'icons.json'
    else:
        icons_path = Path(icons_json)
    
    try:
        with open(icons_path, 'r', encoding='utf-8') as f:
            data = json.load(f)
            if isinstance(data, list):
                return data
    except Exception:
        pass
    return []


def _parse_attributes(attrs_str: str) -> Dict[str, str]:
    """Parse HTML attributes string into a dictionary."""
    attrs = {}
    
    # Pattern to match attribute="value" or attribute='value' or attribute=value
    pattern = r'(\w+)=(?:"([^"]*)"|\'([^\']*)\'|([^\s>]+))'
    
    for match in re.finditer(pattern, attrs_str):
        attr_name = match.group(1)
        attr_value = match.group(2) or match.group(3) or match.group(4) or ''
        attrs[attr_name] = attr_value
    
    return attrs


def _resolve_icon_key(
    requested_key: str, 
    icons_list: List[str], 
    ignore_iconset: bool = False
) -> str:
    """
    Resolve icon key to exact icon available in icons_list.
    
    Args:
        requested_key: Icon key like "eva:people-outline" or "mingcute:ad-circle-line"
        icons_list: List of available icons from icons.json
        ignore_iconset: If True, ignore the prefix and match by name only
    
    Returns:
        Resolved icon key or empty string if not found
    """
    if not requested_key:
        return ""
    
    # Convert colon notation to slash notation
    if ':' in requested_key:
        requested_key = requested_key.replace(':', '/')
    
    if ignore_iconset:
        # Extract just the icon name (part after / or :)
        if '/' in requested_key:
            name = requested_key.split('/', 1)[1]
        else:
            name = requested_key
        
        # Find first icon in list that ends with this name
        for entry in icons_list:
            if isinstance(entry, str) and entry.endswith('/' + name):
                return entry
        return ""
    else:
        # Try exact match first
        if requested_key in icons_list:
            return requested_key
        
        # Fallback: match by name part only
        if '/' in requested_key:
            name = requested_key.split('/', 1)[1]
        else:
            name = requested_key
        
        for entry in icons_list:
            if isinstance(entry, str) and entry.endswith('/' + name):
                return entry
        return ""


def _fetch_icon_svg(icon_key: str) -> str:
    """Fetch SVG from Iconify API for icon_key like 'mdi/play'. Cache results."""
    if not icon_key:
        return ""
    
    if icon_key in _SVG_CACHE:
        return _SVG_CACHE[icon_key]
    
    url = f"https://api.iconify.design/{icon_key}.svg"
    try:
        with urlopen(url, timeout=10) as resp:
            svg = resp.read().decode('utf-8')
            _SVG_CACHE[icon_key] = svg
            return svg
    except (URLError, HTTPError):
        return ""


def _apply_attributes_to_svg(svg: str, attrs: Dict[str, str]) -> str:
    """Apply iconify-icon attributes to the SVG element."""
    if not svg:
        return svg
    
    # Find the opening <svg> tag
    svg_match = re.search(r'<svg([^>]*?)>', svg)
    if not svg_match:
        return svg
    
    existing_attrs = svg_match.group(1)
    
    # Build new attributes
    new_attrs = []
    
    # Handle width and height
    if 'width' in attrs:
        new_attrs.append(f'width="{attrs["width"]}"')
    if 'height' in attrs:
        new_attrs.append(f'height="{attrs["height"]}"')
    
    # Handle class attribute
    if 'class' in attrs:
        # Check if SVG already has a class
        existing_class_match = re.search(r'class="([^"]*)"', existing_attrs)
        if existing_class_match:
            # Merge classes
            existing_class = existing_class_match.group(1)
            merged_class = f"{existing_class} {attrs['class']}".strip()
            new_attrs.append(f'class="{merged_class}"')
        else:
            new_attrs.append(f'class="{attrs["class"]}"')
    
    # Handle style attribute
    if 'style' in attrs:
        # Check if SVG already has a style
        existing_style_match = re.search(r'style="([^"]*)"', existing_attrs)
        if existing_style_match:
            # Merge styles
            existing_style = existing_style_match.group(1)
            merged_style = f"{existing_style}; {attrs['style']}".strip()
            new_attrs.append(f'style="{merged_style}"')
        else:
            new_attrs.append(f'style="{attrs["style"]}"')
    
    # Handle other attributes (excluding 'icon' which is iconify-specific)
    for attr_name, attr_value in attrs.items():
        if attr_name not in ['icon', 'width', 'height', 'class', 'style']:
            new_attrs.append(f'{attr_name}="{attr_value}"')
    
    # Reconstruct SVG with new attributes
    if new_attrs:
        # Remove existing width, height, class, style from original attrs
        cleaned_attrs = existing_attrs
        for attr_to_remove in ['width', 'height', 'class', 'style']:
            cleaned_attrs = re.sub(rf'\s*{attr_to_remove}="[^"]*"', '', cleaned_attrs)
        
        # Combine cleaned existing attrs with new attrs
        all_attrs = cleaned_attrs.strip()
        if all_attrs and new_attrs:
            all_attrs += ' ' + ' '.join(new_attrs)
        elif new_attrs:
            all_attrs = ' '.join(new_attrs)
        
        new_svg_tag = f'<svg {all_attrs}>' if all_attrs else '<svg>'
        return svg.replace(svg_match.group(0), new_svg_tag)
    
    return svg


def clear_cache():
    """Clear the SVG cache. Useful for testing or memory management."""
    global _SVG_CACHE
    _SVG_CACHE.clear()


def get_cache_size() -> int:
    """Get the number of cached SVG entries."""
    return len(_SVG_CACHE)
