from enum import Enum
from typing import Dict, List, Any, TypedDict, Union, Literal

# Navigation key literals
NavigationKey = Literal['pagedown', 'pageup', 'home', 'end', 'left', 'right', 'up', 'down']

# Special key literals
SpecialKey = Literal['enter', 'esc', 'tab', 'space', 'backspace', 'del']

# Modifier key literals
ModifierKey = Literal['ctrl', 'alt', 'shift', 'win', 'command', 'option']

# Function key literals
FunctionKey = Literal['f1', 'f2', 'f3', 'f4', 'f5', 'f6', 'f7', 'f8', 'f9', 'f10', 'f11', 'f12']

class Key(Enum):
    """Keyboard keys that can be used with press_key.
    
    These key names map to PyAutoGUI's expected key names.
    """
    # Navigation
    PAGE_DOWN = 'pagedown'
    PAGE_UP = 'pageup'
    HOME = 'home'
    END = 'end'
    LEFT = 'left'
    RIGHT = 'right'
    UP = 'up'
    DOWN = 'down'
    
    # Special keys
    RETURN = 'enter'
    ENTER = 'enter'
    ESCAPE = 'esc'
    ESC = 'esc'
    TAB = 'tab'
    SPACE = 'space'
    BACKSPACE = 'backspace'
    DELETE = 'del'
    
    # Modifier keys
    ALT = 'alt'
    CTRL = 'ctrl'
    SHIFT = 'shift'
    WIN = 'win'
    COMMAND = 'command'
    OPTION = 'option'
    
    # Function keys
    F1 = 'f1'
    F2 = 'f2'
    F3 = 'f3'
    F4 = 'f4'
    F5 = 'f5'
    F6 = 'f6'
    F7 = 'f7'
    F8 = 'f8'
    F9 = 'f9'
    F10 = 'f10'
    F11 = 'f11'
    F12 = 'f12'

    @classmethod
    def from_string(cls, key: str) -> 'Key | str':
        """Convert a string key name to a Key enum value.
        
        Args:
            key: String key name to convert
            
        Returns:
            Key enum value if the string matches a known key,
            otherwise returns the original string for single character keys
        """
        # Map common alternative names to enum values
        key_mapping = {
            'page_down': cls.PAGE_DOWN,
            'page down': cls.PAGE_DOWN,
            'pagedown': cls.PAGE_DOWN,
            'page_up': cls.PAGE_UP,
            'page up': cls.PAGE_UP,
            'pageup': cls.PAGE_UP,
            'return': cls.RETURN,
            'enter': cls.ENTER,
            'escape': cls.ESCAPE,
            'esc': cls.ESC,
            'delete': cls.DELETE,
            'del': cls.DELETE,
            # Modifier key mappings
            'alt': cls.ALT,
            'ctrl': cls.CTRL,
            'control': cls.CTRL,
            'shift': cls.SHIFT,
            'win': cls.WIN,
            'windows': cls.WIN,
            'super': cls.WIN,
            'command': cls.COMMAND,
            'cmd': cls.COMMAND,
            '⌘': cls.COMMAND,
            'option': cls.OPTION,
            '⌥': cls.OPTION,
        }
        
        normalized = key.lower().strip()
        return key_mapping.get(normalized, key)

# Combined key type
KeyType = Union[Key, NavigationKey, SpecialKey, ModifierKey, FunctionKey, str]

class AccessibilityWindow(TypedDict):
    """Information about a window in the accessibility tree."""
    app_name: str
    pid: int
    frontmost: bool
    has_windows: bool
    windows: List[Dict[str, Any]]

class AccessibilityTree(TypedDict):
    """Complete accessibility tree information."""
    success: bool
    frontmost_application: str
    windows: List[AccessibilityWindow] 