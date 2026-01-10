import logging
from typing import Dict

logger = logging.getLogger("desktopenv.metrics.edge")


def check_edge_font_size(font_size, rule):
    """
    Check if the font size is as expected.
    """

    default_font_size = font_size["default_font_size"]
    if rule["type"] == "value":
        return 1.0 if default_font_size == rule["value"] else 0.0
    elif rule["type"] == "range":
        return 1.0 if rule["min"] <= default_font_size < rule["max"] else 0.0
    else:
        raise TypeError(f"{rule['type']} not support yet!")


def is_url_shortcut_on_desktop(shortcuts: Dict[str, str], rule):
    """
    Check if the url shortcut is on the desktop.
    """
    if rule["type"] == "url":
        for shortcut_path, shortcut_content in shortcuts.items():
            if rule["url_content"] in shortcut_content:
                return 1.0
        return 0.0
    else:
        raise TypeError(f"{rule['type']} not support yet!")
