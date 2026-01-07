from __future__ import annotations

from dataclasses import dataclass
from typing import List, Literal, Union, Optional

# --- Screen size options for desktop environments ---
StandardScreenSize = Union[
    # Standard Desktop Resolutions
    tuple[Literal[1920], Literal[1080]],  # Full HD (current default)
    tuple[Literal[1366], Literal[768]],  # HD (laptop standard)
    tuple[Literal[2560], Literal[1440]],  # 2K/QHD
    tuple[Literal[3840], Literal[2160]],  # 4K/UHD
    tuple[Literal[1280], Literal[720]],  # HD Ready
    tuple[Literal[1600], Literal[900]],  # HD+
    tuple[Literal[1920], Literal[1200]],  # WUXGA
    tuple[Literal[2560], Literal[1600]],  # WQXGA
    tuple[Literal[3440], Literal[1440]],  # Ultrawide QHD
    tuple[Literal[5120], Literal[1440]],  # Super Ultrawide
    # Mobile/Tablet Resolutions
    tuple[Literal[1024], Literal[768]],  # iPad (portrait)
    tuple[Literal[768], Literal[1024]],  # iPad (landscape)
    tuple[Literal[360], Literal[640]],  # Mobile portrait
    tuple[Literal[640], Literal[360]],  # Mobile landscape
    # Legacy Resolutions
    tuple[Literal[1024], Literal[600]],  # Netbook
    tuple[Literal[800], Literal[600]],  # SVGA
    tuple[Literal[640], Literal[480]],  # VGA
    # Additional Common Resolutions
    tuple[Literal[1440], Literal[900]],  # Custom laptop
    tuple[Literal[1680], Literal[1050]],  # WSXGA+
    tuple[Literal[1920], Literal[1440]],  # Custom 4:3 ratio
    tuple[Literal[2560], Literal[1080]],  # Ultrawide Full HD
    tuple[Literal[3440], Literal[1440]],  # Ultrawide QHD
    tuple[Literal[3840], Literal[1080]],  # Super Ultrawide Full HD
]


# --- Snapshot types ---
@dataclass
class WindowSnapshot:
    window_type: Literal["webview", "process", "desktop"]
    pid: Optional[str] = None
    url: Optional[str] = None
    html: Optional[str] = None
    title: str = ""
    x: int = 0
    y: int = 0
    width: int = 0
    height: int = 0
    active: bool = False
    minimized: bool = False


@dataclass
class Snapshot:
    windows: List[WindowSnapshot]

# --- Action types ---

# Mouse Actions
@dataclass
class ClickAction:
    x: int
    y: int

@dataclass
class RightClickAction:
    x: int
    y: int

@dataclass
class DoubleClickAction:
    x: int
    y: int

@dataclass
class MiddleClickAction:
    x: int
    y: int

@dataclass
class DragAction:
    from_x: int
    from_y: int
    to_x: int
    to_y: int
    duration: float = 1.0

@dataclass
class MoveToAction:
    x: int
    y: int
    duration: float = 0.0

@dataclass
class ScrollAction:
    direction: Literal["up", "down"] = "up"
    amount: int = 100

# Keyboard Actions
@dataclass
class TypeAction:
    text: str

@dataclass
class KeyAction:
    key: str

@dataclass
class HotkeyAction:
    keys: List[str]

# Control Actions
@dataclass
class DoneAction:
    pass

@dataclass
class WaitAction:
    seconds: float = 1.0

Action = Union[
    ClickAction,
    RightClickAction,
    DoubleClickAction,
    MiddleClickAction,
    DragAction,
    MoveToAction,
    ScrollAction,
    TypeAction,
    KeyAction,
    HotkeyAction,
    DoneAction,
    WaitAction,
]

__all__ = [
    "ClickAction",
    "RightClickAction",
    "DoubleClickAction",
    "MiddleClickAction",
    "DragAction",
    "MoveToAction",
    "ScrollAction",
    "TypeAction",
    "KeyAction",
    "HotkeyAction",
    "DoneAction",
    "WaitAction",
    "Action",
    "WindowSnapshot",
    "Snapshot",
]
