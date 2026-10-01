"""Per-platform role table for native accessibility candidates (RFC #4268).

Cua Driver reports each element's raw platform role: macOS AX names
(``AXButton``), Windows UIA ``control_type_name`` values (``Button``), or Linux
AT-SPI role names (``push button``). It does not normalize them across
platforms, so jev-use owns this closed mapping from raw roles to a small set of
role classes. Unknown roles are excluded, never guessed.

Lookups go through ``normalized_role``, a port of Driver's private
``normalized_role`` in ``cua-driver-core/src/expectation.rs``: keep ASCII
alphanumerics, lowercase, strip an ``ax`` prefix, and map ``pushbutton`` to
``button`` and ``pagetab``/``tabitem`` to ``tab``. Because every table is keyed
by that normalized form, two raw roles that Driver's ``verify_state`` treats as
the same role (for example ``AXCheckBox``, ``CheckBox`` and ``check box``) can
never map to different role classes here. The tables stay per platform because
the same normalized name can mean different controls: Linux AT-SPI ``text`` is
an editable text control, while Windows UIA ``Text`` is static text.
"""

from __future__ import annotations

from typing import Literal, Mapping

Platform = Literal["macos", "windows", "linux"]
RoleClass = Literal[
    "button", "toggle", "checkbox", "radio", "popup", "menu_item", "link", "text_input"
]
ActionKind = Literal["press", "toggle", "select", "open_menu", "set_text", "visual_click"]

PLATFORMS: tuple[Platform, ...] = ("macos", "windows", "linux")

ROLE_CLASSES: tuple[RoleClass, ...] = (
    "button",
    "toggle",
    "checkbox",
    "radio",
    "popup",
    "menu_item",
    "link",
    "text_input",
)

# The one action kind each role class offers.
ROLE_CLASS_ACTION: Mapping[RoleClass, ActionKind] = {
    "button": "press",
    "toggle": "toggle",
    "checkbox": "toggle",
    "radio": "select",
    "popup": "open_menu",
    "menu_item": "press",
    "link": "press",
    "text_input": "set_text",
}

ACTION_KINDS: frozenset[ActionKind] = frozenset(
    {"press", "toggle", "select", "open_menu", "set_text", "visual_click"}
)

# Raw platform roles, as Driver reports them, for each role class. The
# normalized tables below are derived from these, so this is the reviewed data.
RAW_ROLES: Mapping[Platform, Mapping[RoleClass, tuple[str, ...]]] = {
    "macos": {
        "button": ("AXButton",),
        "toggle": ("AXSwitch",),
        "checkbox": ("AXCheckBox",),
        "radio": ("AXRadioButton",),
        "popup": ("AXPopUpButton", "AXComboBox", "AXMenuButton"),
        "menu_item": ("AXMenuItem", "AXMenuBarItem"),
        "link": ("AXLink",),
        "text_input": ("AXTextField", "AXTextArea", "AXSearchField", "AXSecureTextField"),
    },
    # UIA control types; the WPF and WinUI3 harnesses report the same ones.
    "windows": {
        "button": ("Button", "SplitButton"),
        "toggle": (),
        "checkbox": ("CheckBox",),
        "radio": ("RadioButton",),
        "popup": ("ComboBox",),
        "menu_item": ("MenuItem",),
        "link": ("Hyperlink",),
        "text_input": ("Edit",),
    },
    "linux": {
        "button": ("push button", "button"),
        "toggle": ("toggle button", "switch"),
        "checkbox": ("check box",),
        "radio": ("radio button",),
        "popup": ("combo box",),
        "menu_item": ("menu item", "check menu item", "radio menu item"),
        "link": ("link",),
        "text_input": ("entry", "text", "password text"),
    },
}


def normalized_role(role: str) -> str:
    """Port of Driver's ``normalized_role`` (``cua-driver-core/src/expectation.rs``)."""
    normalized = "".join(
        character.lower()
        for character in role
        if character.isascii() and character.isalnum()
    )
    if normalized.startswith("ax"):
        normalized = normalized[2:]
    if normalized == "pushbutton":
        return "button"
    if normalized in {"pagetab", "tabitem"}:
        return "tab"
    return normalized


def _normalized_table(platform: Platform) -> dict[str, RoleClass]:
    table: dict[str, RoleClass] = {}
    for role_class, raw_roles in RAW_ROLES[platform].items():
        for raw in raw_roles:
            key = normalized_role(raw)
            existing = table.get(key)
            if existing is not None and existing != role_class:
                raise ValueError(
                    f"{platform} roles normalized to {key!r} map to both {existing} and {role_class}"
                )
            table[key] = role_class
    return table


ROLE_TABLES: Mapping[Platform, Mapping[str, RoleClass]] = {
    platform: _normalized_table(platform) for platform in PLATFORMS
}


# Window-chrome containers, by normalized role. Their actionable descendants
# (Windows' title-bar System menu, Minimize, Maximize, and Close) belong to
# the window manager, not the application, so they are never candidates. On
# macOS the AppKit fixtures carry no such container, and on Linux the window
# decorations are not part of the application's AT-SPI tree.
WINDOW_CHROME_ROLES: Mapping[Platform, frozenset[str]] = {
    "macos": frozenset(),
    "windows": frozenset({normalized_role("TitleBar")}),
    "linux": frozenset(),
}


def is_window_chrome(raw_role: object, platform: Platform) -> bool:
    """Whether a raw Driver role is a window-chrome container on ``platform``."""
    return isinstance(raw_role, str) and normalized_role(raw_role) in WINDOW_CHROME_ROLES[platform]


def role_class(raw_role: object, platform: Platform) -> RoleClass | None:
    """Return the role class for a raw Driver role, or ``None`` when excluded."""
    if not isinstance(raw_role, str) or not raw_role:
        return None
    return ROLE_TABLES[platform].get(normalized_role(raw_role))
