import asyncio
import base64
import copy
import json
import logging
import re
import time
from ctypes import POINTER, byref, c_void_p
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

import AppKit
import Foundation
import objc
from AppKit import NSWorkspace  # type: ignore
from ApplicationServices import AXUIElementCopyAttributeValue  # type: ignore
from ApplicationServices import AXUIElementCopyAttributeValues  # type: ignore
from ApplicationServices import AXUIElementCreateApplication  # type: ignore
from ApplicationServices import AXUIElementCreateSystemWide  # type: ignore
from ApplicationServices import AXUIElementGetTypeID  # type: ignore
from ApplicationServices import AXValueGetType  # type: ignore
from ApplicationServices import AXValueGetValue  # type: ignore
from ApplicationServices import kAXChildrenAttribute  # type: ignore
from ApplicationServices import kAXDescriptionAttribute  # type: ignore
from ApplicationServices import kAXEnabledAttribute  # type: ignore
from ApplicationServices import kAXErrorSuccess  # type: ignore
from ApplicationServices import kAXFocusedApplicationAttribute  # type: ignore
from ApplicationServices import kAXFocusedUIElementAttribute  # type: ignore
from ApplicationServices import kAXFocusedWindowAttribute  # type: ignore
from ApplicationServices import kAXMainWindowAttribute  # type: ignore
from ApplicationServices import kAXPositionAttribute  # type: ignore
from ApplicationServices import kAXRoleAttribute  # type: ignore
from ApplicationServices import kAXRoleDescriptionAttribute  # type: ignore
from ApplicationServices import kAXSelectedTextAttribute  # type: ignore
from ApplicationServices import kAXSelectedTextRangeAttribute  # type: ignore
from ApplicationServices import kAXSizeAttribute  # type: ignore
from ApplicationServices import kAXTitleAttribute  # type: ignore
from ApplicationServices import kAXValueAttribute  # type: ignore
from ApplicationServices import kAXValueCFRangeType  # type: ignore
from ApplicationServices import kAXValueCGPointType  # type: ignore
from ApplicationServices import kAXValueCGSizeType  # type: ignore
from ApplicationServices import kAXVisibleChildrenAttribute  # type: ignore
from ApplicationServices import kAXWindowsAttribute  # type: ignore
from pynput.keyboard import Controller as KeyboardController
from pynput.keyboard import Key
from pynput.mouse import Button
from pynput.mouse import Controller as MouseController
from Quartz.CoreGraphics import *  # type: ignore
from Quartz.CoreGraphics import CGPoint, CGSize  # type: ignore

from .base import BaseAccessibilityHandler, BaseAutomationHandler

logger = logging.getLogger(__name__)

# Trigger accessibility permissions prompt on macOS
try:
    # Source - https://stackoverflow.com/a/17134
    # Posted by Andreas
    # Retrieved 2025-12-03, License - CC BY-SA 4.0
    # Attempt to create and post a mouse event to trigger the permissions prompt
    # This will cause macOS to show "Python would like to control this computer using accessibility features"
    current_pos = CGEventGetLocation(CGEventCreate(None))
    p = CGPoint()
    p.x = current_pos.x
    p.y = current_pos.y

    me = CGEventCreateMouseEvent(None, kCGEventMouseMoved, p, 0)
    if me:
        CGEventPost(kCGHIDEventTap, me)
        CFRelease(me)
except Exception as e:
    logger.debug(f"Failed to trigger accessibility permissions prompt: {e}")

# Trigger screen recording prompt on macOS
try:
    from PIL import ImageGrab, Image

    ImageGrab.grab()
except Exception as e:
    logger.debug(f"Failed to trigger screenshot permissions prompt: {e}")


# Constants for accessibility API
kAXErrorSuccess = 0
kAXRoleAttribute = "AXRole"
kAXTitleAttribute = "AXTitle"
kAXValueAttribute = "AXValue"
kAXWindowsAttribute = "AXWindows"
kAXFocusedAttribute = "AXFocused"
kAXPositionAttribute = "AXPosition"
kAXSizeAttribute = "AXSize"
kAXChildrenAttribute = "AXChildren"
kAXMenuBarAttribute = "AXMenuBar"
kAXMenuBarItemAttribute = "AXMenuBarItem"

# Constants for window properties
kCGWindowLayer = "kCGWindowLayer"  # Z-order information (lower values are higher in the stack)
kCGWindowAlpha = "kCGWindowAlpha"  # Window opacity

# Constants for application activation options
NSApplicationActivationOptions = {
    "regular": 0,  # Default activation
    "bringing_all_windows_forward": 1 << 0,  # NSApplicationActivateAllWindows
    "ignoring_other_apps": 1 << 1,  # NSApplicationActivateIgnoringOtherApps
}


def CFAttributeToPyObject(attrValue):
    """Convert Core Foundation attribute values to Python objects.

    Args:
        attrValue: Core Foundation attribute value to convert

    Returns:
        Converted Python object or None if conversion fails
    """

    def list_helper(list_value):
        """Helper function to convert CF arrays to Python lists.

        Args:
            list_value: Core Foundation array to convert

        Returns:
            Python list containing converted items
        """
        list_builder = []
        for item in list_value:
            list_builder.append(CFAttributeToPyObject(item))
        return list_builder

    def number_helper(number_value):
        """Helper function to convert CF numbers to Python numbers.

        Args:
            number_value: Core Foundation number to convert

        Returns:
            Python int or float, or None if conversion fails
        """
        success, int_value = Foundation.CFNumberGetValue(  # type: ignore
            number_value, Foundation.kCFNumberIntType, None  # type: ignore
        )
        if success:
            return int(int_value)

        success, float_value = Foundation.CFNumberGetValue(  # type: ignore
            number_value, Foundation.kCFNumberDoubleType, None  # type: ignore
        )
        if success:
            return float(float_value)
        return None

    def axuielement_helper(element_value):
        """Helper function to handle AX UI elements.

        Args:
            element_value: Accessibility UI element to process

        Returns:
            The element value unchanged
        """
        return element_value

    cf_attr_type = Foundation.CFGetTypeID(attrValue)  # type: ignore
    cf_type_mapping = {
        Foundation.CFStringGetTypeID(): str,  # type: ignore
        Foundation.CFBooleanGetTypeID(): bool,  # type: ignore
        Foundation.CFArrayGetTypeID(): list_helper,  # type: ignore
        Foundation.CFNumberGetTypeID(): number_helper,  # type: ignore
        AXUIElementGetTypeID(): axuielement_helper,  # type: ignore
    }
    try:
        return cf_type_mapping[cf_attr_type](attrValue)
    except KeyError:
        # did not get a supported CF type. Move on to AX type
        pass

    ax_attr_type = AXValueGetType(attrValue)
    ax_type_map = {
        kAXValueCGSizeType: Foundation.NSSizeFromString,  # type: ignore
        kAXValueCGPointType: Foundation.NSPointFromString,  # type: ignore
        kAXValueCFRangeType: Foundation.NSRangeFromString,  # type: ignore
    }
    try:
        search_result = re.search("{.*}", attrValue.description())
        if search_result:
            extracted_str = search_result.group()
            return tuple(ax_type_map[ax_attr_type](extracted_str))
        return None
    except KeyError:
        return None


def element_attribute(element, attribute):
    """Get an attribute value from an accessibility element.

    Args:
        element: The accessibility element
        attribute: The attribute name to retrieve

    Returns:
        The attribute value or None if not found
    """
    if attribute == kAXChildrenAttribute:
        err, value = AXUIElementCopyAttributeValues(element, attribute, 0, 999, None)
        if err == kAXErrorSuccess:
            if isinstance(value, Foundation.NSArray):  # type: ignore
                return CFAttributeToPyObject(value)
            else:
                return value
    err, value = AXUIElementCopyAttributeValue(element, attribute, None)
    if err == kAXErrorSuccess:
        if isinstance(value, Foundation.NSArray):  # type: ignore
            return CFAttributeToPyObject(value)
        else:
            return value
    return None


def element_value(element, type):
    """Extract a typed value from an accessibility element.

    Args:
        element: The accessibility element containing the value
        type: The expected value type

    Returns:
        The extracted value or None if extraction fails
    """
    err, value = AXValueGetValue(element, type, None)
    if err == True:
        return value
    return None


class UIElement:
    """Represents a UI element in the accessibility tree with position, size, and hierarchy information."""

    def __init__(self, element, offset_x=0, offset_y=0, max_depth=None, parents_visible_bbox=None):
        """Initialize a UIElement from an accessibility element.

        Args:
            element: The accessibility element to wrap
            offset_x: X offset for position calculations
            offset_y: Y offset for position calculations
            max_depth: Maximum depth to traverse for children
            parents_visible_bbox: Parent's visible bounding box for clipping
        """
        self.ax_element = element
        self.content_identifier = ""
        self.identifier = ""
        self.name = ""
        self.children = []
        self.description = ""
        self.role_description = ""
        self.value = None
        self.max_depth = max_depth

        # Set role
        self.role = element_attribute(element, kAXRoleAttribute)
        if self.role is None:
            self.role = "No role"

        # Set name
        self.name = element_attribute(element, kAXTitleAttribute)
        if self.name is not None:
            # Convert tuple to string if needed
            if isinstance(self.name, tuple):
                self.name = str(self.name[0]) if self.name else ""
            self.name = self.name.replace(" ", "_")

        # Set enabled
        self.enabled = element_attribute(element, kAXEnabledAttribute)
        if self.enabled is None:
            self.enabled = False

        # Set position and size
        position = element_attribute(element, kAXPositionAttribute)
        size = element_attribute(element, kAXSizeAttribute)
        start_position = element_value(position, kAXValueCGPointType)

        if self.role == "AXWindow" and start_position is not None:
            offset_x = start_position.x
            offset_y = start_position.y

        self.absolute_position = copy.copy(start_position)
        self.position = start_position
        if self.position is not None:
            self.position.x -= max(0, offset_x)
            self.position.y -= max(0, offset_y)
        self.size = element_value(size, kAXValueCGSizeType)

        self._set_bboxes(parents_visible_bbox)

        # Set component center
        if start_position is None or self.size is None:
            print("Position is None")
            return
        self.center = (
            start_position.x + offset_x + self.size.width / 2,
            start_position.y + offset_y + self.size.height / 2,
        )

        self.description = element_attribute(element, kAXDescriptionAttribute)
        self.role_description = element_attribute(element, kAXRoleDescriptionAttribute)
        attribute_value = element_attribute(element, kAXValueAttribute)

        # Set value
        self.value = attribute_value
        if attribute_value is not None:
            if isinstance(attribute_value, Foundation.NSArray):  # type: ignore
                self.value = []
                for value in attribute_value:
                    self.value.append(value)
            # Check if it's an accessibility element by checking its type ID
            elif Foundation.CFGetTypeID(attribute_value) == AXUIElementGetTypeID():  # type: ignore
                self.value = UIElement(attribute_value, offset_x, offset_y)

        # Set children
        if self.max_depth is None or self.max_depth > 0:
            self.children = self._get_children(element, start_position, offset_x, offset_y)
        else:
            self.children = []

        self.calculate_hashes()

    def _set_bboxes(self, parents_visible_bbox):
        """Set bounding box and visible bounding box for the element.

        Args:
            parents_visible_bbox: Parent's visible bounding box for intersection calculation
        """
        if not self.absolute_position or not self.size:
            self.bbox = None
            self.visible_bbox = None
            return
        self.bbox = [
            int(self.absolute_position.x),
            int(self.absolute_position.y),
            int(self.absolute_position.x + self.size.width),
            int(self.absolute_position.y + self.size.height),
        ]
        if parents_visible_bbox:
            # check if not intersected
            if (
                self.bbox[0] > parents_visible_bbox[2]
                or self.bbox[1] > parents_visible_bbox[3]
                or self.bbox[2] < parents_visible_bbox[0]
                or self.bbox[3] < parents_visible_bbox[1]
            ):
                self.visible_bbox = None
            else:
                self.visible_bbox = [
                    int(max(self.bbox[0], parents_visible_bbox[0])),
                    int(max(self.bbox[1], parents_visible_bbox[1])),
                    int(min(self.bbox[2], parents_visible_bbox[2])),
                    int(min(self.bbox[3], parents_visible_bbox[3])),
                ]
        else:
            self.visible_bbox = self.bbox

    def _get_children(self, element, start_position, offset_x, offset_y):
        """Get child elements from the accessibility element.

        Args:
            element: The parent accessibility element
            start_position: Starting position for offset calculations
            offset_x: X offset for child positioning
            offset_y: Y offset for child positioning

        Returns:
            List of UIElement children
        """
        children = element_attribute(element, kAXChildrenAttribute)
        visible_children = element_attribute(element, kAXVisibleChildrenAttribute)
        found_children = []
        if children is not None:
            found_children.extend(children)
        else:
            if visible_children is not None:
                found_children.extend(visible_children)

        result = []
        if self.max_depth is None or self.max_depth > 0:
            for child in found_children:
                child = UIElement(
                    child,
                    offset_x,
                    offset_y,
                    self.max_depth - 1 if self.max_depth is not None else None,
                    self.visible_bbox,
                )
                result.append(child)
        return result

    def calculate_hashes(self):
        """Calculate unique identifiers for the element and its content."""
        self.identifier = self.component_hash()
        self.content_identifier = self.children_content_hash(self.children)

    def component_hash(self):
        """Generate a hash identifier for this component based on its properties.

        Returns:
            MD5 hash string of component properties
        """
        if self.position is None or self.size is None:
            return ""
        position_string = f"{self.position.x:.0f};{self.position.y:.0f}"
        size_string = f"{self.size.width:.0f};{self.size.height:.0f}"
        enabled_string = str(self.enabled)
        # Ensure role is a string
        role_string = ""
        if self.role is not None:
            role_string = str(self.role[0]) if isinstance(self.role, tuple) else str(self.role)
        return self.hash_from_string(position_string + size_string + enabled_string + role_string)

    def hash_from_string(self, string):
        """Generate MD5 hash from a string.

        Args:
            string: Input string to hash

        Returns:
            MD5 hash hexdigest or empty string if input is None/empty
        """
        if string is None or string == "":
            return ""
        from hashlib import md5

        return md5(string.encode()).hexdigest()

    def children_content_hash(self, children):
        """Generate a hash representing the content and structure of child elements.

        Args:
            children: List of child UIElement objects

        Returns:
            Combined hash of children content and structure
        """
        if len(children) == 0:
            return ""
        all_content_hashes = []
        all_hashes = []
        for child in children:
            all_content_hashes.append(child.content_identifier)
            all_hashes.append(child.identifier)
        all_content_hashes.sort()
        if len(all_content_hashes) == 0:
            return ""
        content_hash = self.hash_from_string("".join(all_content_hashes))
        content_structure_hash = self.hash_from_string("".join(all_hashes))
        return self.hash_from_string(content_hash.join(content_structure_hash))

    def to_dict(self):
        """Convert the UIElement to a dictionary representation.

        Returns:
            Dictionary containing all element properties and children
        """

        def children_to_dict(children):
            """Convert list of children to dictionary format.

            Args:
                children: List of UIElement children to convert

            Returns:
                List of dictionaries representing the children
            """
            result = []
            for child in children:
                result.append(child.to_dict())
            return result

        value = self.value
        if isinstance(value, UIElement):
            value = json.dumps(value.to_dict(), indent=4)
        elif isinstance(value, AppKit.NSDate):  # type: ignore
            value = str(value)

        if self.absolute_position is not None:
            absolute_position = f"{self.absolute_position.x:.2f};{self.absolute_position.y:.2f}"
        else:
            absolute_position = ""

        if self.position is not None:
            position = f"{self.position.x:.2f};{self.position.y:.2f}"
        else:
            position = ""

        if self.size is not None:
            size = f"{self.size.width:.0f};{self.size.height:.0f}"
        else:
            size = ""

        return {
            "id": self.identifier,
            "name": self.name,
            "role": self.role,
            "description": self.description,
            "role_description": self.role_description,
            "value": value,
            "absolute_position": absolute_position,
            "position": position,
            "size": size,
            "enabled": self.enabled,
            "bbox": self.bbox,
            "visible_bbox": self.visible_bbox,
            "children": children_to_dict(self.children),
        }


from pathlib import Path

import Quartz
from AppKit import NSRunningApplication, NSWorkspace


def get_all_windows_zorder():
    """Get all windows in the system with their z-order information.

    Returns:
        List of window dictionaries sorted by z-index, containing window properties
        like id, name, pid, owner, bounds, layer, and opacity
    """
    window_list = Quartz.CGWindowListCopyWindowInfo(
        Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID
    )
    z_order = {
        window["kCGWindowNumber"]: z_index for z_index, window in enumerate(window_list[::-1])
    }
    window_list_all = Quartz.CGWindowListCopyWindowInfo(
        Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID
    )
    windows = []
    for window in window_list_all:
        window_id = window.get("kCGWindowNumber", 0)
        window_name = window.get("kCGWindowName", "")
        window_pid = window.get("kCGWindowOwnerPID", 0)
        window_bounds = window.get("kCGWindowBounds", {})
        window_owner = window.get("kCGWindowOwnerName", "")
        window_is_on_screen = window.get("kCGWindowIsOnscreen", False)
        layer = window.get("kCGWindowLayer", 0)
        opacity = window.get("kCGWindowAlpha", 1.0)
        z_index = z_order.get(window_id, -1)
        if window_name == "Dock" and window_owner == "Dock":
            role = "dock"
        elif window_name == "Menubar" and window_owner == "Window Server":
            role = "menubar"
        elif window_owner in ["Window Server", "Dock"]:
            role = "desktop"
        else:
            role = "app"
        if window_bounds:
            windows.append(
                {
                    "id": window_id,
                    "name": window_name or "Unnamed Window",
                    "pid": window_pid,
                    "owner": window_owner,
                    "role": role,
                    "is_on_screen": window_is_on_screen,
                    "bounds": {
                        "x": window_bounds.get("X", 0),
                        "y": window_bounds.get("Y", 0),
                        "width": window_bounds.get("Width", 0),
                        "height": window_bounds.get("Height", 0),
                    },
                    "layer": layer,
                    "z_index": z_index,
                    "opacity": opacity,
                }
            )
    windows = sorted(windows, key=lambda x: x["z_index"])
    return windows


def get_app_info(app):
    """Extract information from an NSRunningApplication object.

    Args:
        app: NSRunningApplication instance

    Returns:
        Dictionary containing app name, bundle ID, PID, and status flags
    """
    return {
        "name": app.localizedName(),
        "bundle_id": app.bundleIdentifier(),
        "pid": app.processIdentifier(),
        "active": app.isActive(),
        "hidden": app.isHidden(),
        "terminated": app.isTerminated(),
    }


def get_menubar_items(active_app_pid=None):
    """Get menubar items for the active application.

    Args:
        active_app_pid: Process ID of the active application, or None to use frontmost app

    Returns:
        List of menubar item dictionaries with title, bounds, index, and app_pid
    """
    menubar_items = []
    if active_app_pid is None:
        frontmost_app = NSWorkspace.sharedWorkspace().frontmostApplication()
        if frontmost_app:
            active_app_pid = frontmost_app.processIdentifier()
        else:
            return menubar_items
    app_element = AXUIElementCreateApplication(active_app_pid)
    if app_element is None:
        return menubar_items
    menubar = element_attribute(app_element, kAXMenuBarAttribute)
    if menubar is None:
        return menubar_items
    children = element_attribute(menubar, kAXChildrenAttribute)
    if children is None:
        return menubar_items
    for i, item in enumerate(children):
        title = element_attribute(item, kAXTitleAttribute) or "Untitled"
        bounds = {"x": 0, "y": 0, "width": 0, "height": 0}
        position_value = element_attribute(item, kAXPositionAttribute)
        if position_value:
            position_value = element_value(position_value, kAXValueCGPointType)
            bounds["x"] = getattr(position_value, "x", 0)
            bounds["y"] = getattr(position_value, "y", 0)
        size_value = element_attribute(item, kAXSizeAttribute)
        if size_value:
            size_value = element_value(size_value, kAXValueCGSizeType)
            bounds["width"] = getattr(size_value, "width", 0)
            bounds["height"] = getattr(size_value, "height", 0)
        menubar_items.append(
            {"title": title, "bounds": bounds, "index": i, "app_pid": active_app_pid}
        )
    return menubar_items


def get_dock_items():
    """Get all items in the macOS Dock.

    Returns:
        List of dock item dictionaries with title, description, bounds, index,
        type, role, and subrole information
    """
    dock_items = []
    dock_pid = None
    running_apps = NSWorkspace.sharedWorkspace().runningApplications()
    for app in running_apps:
        if app.localizedName() == "Dock" and app.bundleIdentifier() == "com.apple.dock":
            dock_pid = app.processIdentifier()
            break
    if dock_pid is None:
        return dock_items
    dock_element = AXUIElementCreateApplication(dock_pid)
    if dock_element is None:
        return dock_items
    dock_list = element_attribute(dock_element, kAXChildrenAttribute)
    if dock_list is None or len(dock_list) == 0:
        return dock_items
    dock_app_list = None
    for child in dock_list:
        role = element_attribute(child, kAXRoleAttribute)
        if role == "AXList":
            dock_app_list = child
            break
    if dock_app_list is None:
        return dock_items
    items = element_attribute(dock_app_list, kAXChildrenAttribute)
    if items is None:
        return dock_items
    for i, item in enumerate(items):
        title = element_attribute(item, kAXTitleAttribute) or "Untitled"
        description = element_attribute(item, kAXDescriptionAttribute) or ""
        role = element_attribute(item, kAXRoleAttribute) or ""
        subrole = element_attribute(item, "AXSubrole") or ""
        bounds = {"x": 0, "y": 0, "width": 0, "height": 0}
        position_value = element_attribute(item, kAXPositionAttribute)
        if position_value:
            position_value = element_value(position_value, kAXValueCGPointType)
            bounds["x"] = getattr(position_value, "x", 0)
            bounds["y"] = getattr(position_value, "y", 0)
        size_value = element_attribute(item, kAXSizeAttribute)
        if size_value:
            size_value = element_value(size_value, kAXValueCGSizeType)
            bounds["width"] = getattr(size_value, "width", 0)
            bounds["height"] = getattr(size_value, "height", 0)
        item_type = "unknown"
        if subrole == "AXApplicationDockItem":
            item_type = "application"
        elif subrole == "AXFolderDockItem":
            item_type = "folder"
        elif subrole == "AXDocumentDockItem":
            item_type = "document"
        elif subrole == "AXSeparatorDockItem" or role == "AXSeparator":
            item_type = "separator"
        else:
            title_str = title if isinstance(title, str) else str(title)
            if "trash" in title_str.lower():
                item_type = "trash"
        dock_items.append(
            {
                "title": title,
                "description": description,
                "bounds": bounds,
                "index": i,
                "type": item_type,
                "role": role,
                "subrole": subrole,
            }
        )
    return dock_items


class MacOSAccessibilityHandler(BaseAccessibilityHandler):
    """Handler for macOS accessibility features and UI element inspection."""

    def get_desktop_state(self):
        """Get the current state of the desktop including windows, apps, menubar, and dock.

        Returns:
            Dictionary containing applications, windows, menubar_items, and dock_items
        """
        windows = [w for w in get_all_windows_zorder() if w.get("is_on_screen")]
        running_apps = self.get_running_apps()
        applications = []
        pid_to_window_ids = {}
        # Build a mapping: pid -> list of AX window trees
        pid_to_ax_trees = {}
        for app in running_apps:
            pid = app.processIdentifier()
            try:
                app_elem = AXUIElementCreateApplication(pid)
                err, app_windows = AXUIElementCopyAttributeValue(
                    app_elem, kAXWindowsAttribute, None
                )
                trees = []
                if err == kAXErrorSuccess and app_windows:
                    for ax_win in app_windows:
                        try:
                            trees.append(UIElement(ax_win).to_dict())
                        except Exception as e:
                            trees.append({"error": str(e)})
                pid_to_ax_trees[pid] = trees
            except Exception as e:
                pid_to_ax_trees[pid] = [{"error": str(e)}]
        # Attach children by pid and index (order)
        pid_to_idx = {}
        for win in windows:
            pid = win["pid"]
            idx = pid_to_idx.get(pid, 0)
            ax_trees = pid_to_ax_trees.get(pid, [])
            win["children"] = (
                ax_trees[idx]["children"]
                if idx < len(ax_trees) and "children" in ax_trees[idx]
                else []
            )
            pid_to_idx[pid] = idx + 1
            pid_to_window_ids.setdefault(pid, []).append(win["id"])
        for app in running_apps:
            info = get_app_info(app)
            app_pid = info["pid"]
            applications.append({"info": info, "windows": pid_to_window_ids.get(app_pid, [])})
        menubar_items = get_menubar_items()
        dock_items = get_dock_items()
        return {
            "applications": applications,
            "windows": windows,
            "menubar_items": menubar_items,
            "dock_items": dock_items,
        }

    def get_application_windows(self, pid: int):
        """Get all windows for a specific application.

        Args:
            pid: Process ID of the application

        Returns:
            List of accessibility window elements or empty list if none found
        """
        try:
            app = AXUIElementCreateApplication(pid)
            err, windows = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute, None)
            if err == kAXErrorSuccess and windows:
                if isinstance(windows, Foundation.NSArray):  # type: ignore
                    return windows
            return []
        except:
            return []

    def get_all_windows(self):
        """Get all visible windows in the system.

        Returns:
            List of window dictionaries with app information and window details
        """
        try:
            windows = []
            running_apps = self.get_running_apps()

            for app in running_apps:
                try:
                    app_name = app.localizedName()
                    pid = app.processIdentifier()

                    # Skip system processes and background apps
                    if not app.activationPolicy() == 0:  # NSApplicationActivationPolicyRegular
                        continue

                    # Get application windows
                    app_windows = self.get_application_windows(pid)

                    windows.append(
                        {
                            "app_name": app_name,
                            "pid": pid,
                            "frontmost": app.isActive(),
                            "has_windows": len(app_windows) > 0,
                            "windows": app_windows,
                        }
                    )
                except:
                    continue

            return windows
        except:
            return []

    def get_running_apps(self):
        """Get all currently running applications.

        Returns:
            List of NSRunningApplication objects
        """
        # From NSWorkspace.runningApplications docs: https://developer.apple.com/documentation/appkit/nsworkspace/runningapplications
        # "Similar to the NSRunningApplication class's properties, this property will only change when the main run loop runs in a common mode"
        # So we need to run the main run loop to get the latest running applications
        Foundation.CFRunLoopRunInMode(Foundation.kCFRunLoopDefaultMode, 0.1, False)  # type: ignore
        return NSWorkspace.sharedWorkspace().runningApplications()

    def get_ax_attribute(self, element, attribute):
        """Get an accessibility attribute from an element.

        Args:
            element: The accessibility element
            attribute: The attribute name to retrieve

        Returns:
            The attribute value or None if not found
        """
        return element_attribute(element, attribute)

    def serialize_node(self, element):
        """Create a serializable dictionary representation of an accessibility element.

        Args:
            element: The accessibility element to serialize

        Returns:
            Dictionary containing element properties like role, title, value, position, and size
        """
        # Create a serializable dictionary representation of an accessibility element
        result = {}

        # Get basic attributes
        result["role"] = self.get_ax_attribute(element, kAXRoleAttribute)
        result["title"] = self.get_ax_attribute(element, kAXTitleAttribute)
        result["value"] = self.get_ax_attribute(element, kAXValueAttribute)

        # Get position and size if available
        position = self.get_ax_attribute(element, kAXPositionAttribute)
        if position:
            try:
                position_dict = {"x": position[0], "y": position[1]}
                result["position"] = position_dict
            except (IndexError, TypeError):
                pass

        size = self.get_ax_attribute(element, kAXSizeAttribute)
        if size:
            try:
                size_dict = {"width": size[0], "height": size[1]}
                result["size"] = size_dict
            except (IndexError, TypeError):
                pass

        return result

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the complete accessibility tree for the current desktop state.

        Returns:
            Dictionary containing success status and desktop state information
        """
        try:
            desktop_state = self.get_desktop_state()
            return {"success": True, **desktop_state}

        except Exception as e:
            return {"success": False, "error": str(e)}

    async def find_element(
        self, role: Optional[str] = None, title: Optional[str] = None, value: Optional[str] = None
    ) -> Dict[str, Any]:
        """Find an accessibility element matching the specified criteria.

        Args:
            role: The accessibility role to match (optional)
            title: The title to match (optional)
            value: The value to match (optional)

        Returns:
            Dictionary containing success status and the found element or error message
        """
        try:
            system = AXUIElementCreateSystemWide()

            def match_element(element):
                """Check if an element matches the search criteria.

                Args:
                    element: The accessibility element to check

                Returns:
                    True if element matches all specified criteria, False otherwise
                """
                if role and self.get_ax_attribute(element, kAXRoleAttribute) != role:
                    return False
                if title and self.get_ax_attribute(element, kAXTitleAttribute) != title:
                    return False
                if value and str(self.get_ax_attribute(element, kAXValueAttribute)) != value:
                    return False
                return True

            def search_tree(element):
                """Recursively search the accessibility tree for matching elements.

                Args:
                    element: The accessibility element to search from

                Returns:
                    Serialized element dictionary if match found, None otherwise
                """
                if match_element(element):
                    return self.serialize_node(element)

                children = self.get_ax_attribute(element, kAXChildrenAttribute)
                if children:
                    for child in children:
                        result = search_tree(child)
                        if result:
                            return result
                return None

            element = search_tree(system)
            return {"success": True, "element": element}

        except Exception as e:
            return {"success": False, "error": str(e)}


class MacOSAutomationHandler(BaseAutomationHandler):
    """Handler for macOS automation including mouse, keyboard, and screen operations."""

    # Mouse Actions
    mouse = MouseController()
    keyboard = KeyboardController()

    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Press and hold a mouse button at the specified coordinates.

        Args:
            x: X coordinate (optional, uses current position if None)
            y: Y coordinate (optional, uses current position if None)
            button: Mouse button to press ("left", "right", or "middle")

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.press(
                Button.left
                if button == "left"
                else Button.right if button == "right" else Button.middle
            )
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Release a mouse button at the specified coordinates.

        Args:
            x: X coordinate (optional, uses current position if None)
            y: Y coordinate (optional, uses current position if None)
            button: Mouse button to release ("left", "right", or "middle")

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.release(
                Button.left
                if button == "left"
                else Button.right if button == "right" else Button.middle
            )
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a left mouse click at the specified coordinates.

        Args:
            x: X coordinate (optional, uses current position if None)
            y: Y coordinate (optional, uses current position if None)

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(Button.left, 1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a right mouse click at the specified coordinates.

        Args:
            x: X coordinate (optional, uses current position if None)
            y: Y coordinate (optional, uses current position if None)

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(Button.right, 1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        """Perform a double left mouse click at the specified coordinates.

        Args:
            x: X coordinate (optional, uses current position if None)
            y: Y coordinate (optional, uses current position if None)

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(Button.left, 2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        """Move the mouse cursor to the specified coordinates.

        Args:
            x: Target X coordinate
            y: Target Y coordinate

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            self.mouse.position = (x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag from current position to target coordinates.

        Args:
            x: Target X coordinate
            y: Target Y coordinate
            button: Mouse button to use for dragging ("left", "right", or "middle")
            duration: Duration of the drag operation in seconds

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            btn = (
                Button.left
                if button == "left"
                else Button.right if button == "right" else Button.middle
            )
            # Press
            self.mouse.press(btn)
            # Move with sleep to simulate drag duration
            start = self.mouse.position
            steps = 20
            start_x, start_y = start
            dx = (x - start_x) / steps
            dy = (y - start_y) / steps
            for i in range(steps):
                self.mouse.position = (int(start_x + dx * (i + 1)), int(start_y + dy * (i + 1)))
                time.sleep(duration / steps)
            # Release
            self.mouse.release(btn)
            return {"success": True}
        except Exception as e:
            try:
                self.mouse.release(btn)
            except:
                pass
            return {"success": False, "error": str(e)}

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag the mouse along a specified path of coordinates.

        Args:
            path: List of (x, y) coordinate tuples defining the drag path
            button: Mouse button to use for dragging ("left", "right", or "middle")
            duration: Total duration of the drag operation in seconds

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            if not path or len(path) < 2:
                return {"success": False, "error": "Path must contain at least 2 points"}
            btn = (
                Button.left
                if button == "left"
                else Button.right if button == "right" else Button.middle
            )
            # Move to the first point
            self.mouse.position = path[0]
            self.mouse.press(btn)
            step_duration = duration / (len(path) - 1) if len(path) > 1 else duration
            for x, y in path[1:]:
                self.mouse.position = (x, y)
                time.sleep(step_duration)
            self.mouse.release(btn)
            return {"success": True}
        except Exception as e:
            try:
                self.mouse.release(btn)
            except:
                pass
            return {"success": False, "error": str(e)}

    # Keyboard Actions
    async def key_down(self, key: str) -> Dict[str, Any]:
        """Press and hold a keyboard key.

        Args:
            key: Key name to press

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            k = getattr(Key, key) if hasattr(Key, key) else (key if len(key) == 1 else None)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.press(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def key_up(self, key: str) -> Dict[str, Any]:
        """Release a keyboard key.

        Args:
            key: Key name to release

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            k = getattr(Key, key) if hasattr(Key, key) else (key if len(key) == 1 else None)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.release(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def type_text(self, text: str) -> Dict[str, Any]:
        """Type text using the keyboard with Unicode support.

        Args:
            text: Text string to type

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            self.keyboard.type(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def press_key(self, key: str) -> Dict[str, Any]:
        """Press and release a keyboard key.

        Args:
            key: Key name to press

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            k = getattr(Key, key) if hasattr(Key, key) else (key if len(key) == 1 else None)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.press(k)
            self.keyboard.release(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        """Press a combination of keys simultaneously.

        Args:
            keys: List of key names to press together

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            seq = []
            for k in keys:
                kk = getattr(Key, k) if hasattr(Key, k) else (k if len(k) == 1 else None)
                if kk is None:
                    return {"success": False, "error": f"Unknown key in hotkey: {k}"}
                seq.append(kk)
            for k in seq[:-1]:
                self.keyboard.press(k)
            last = seq[-1]
            self.keyboard.press(last)
            self.keyboard.release(last)
            for k in reversed(seq[:-1]):
                self.keyboard.release(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Scrolling Actions
    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        """Scroll the mouse wheel in the specified direction.

        Args:
            x: Horizontal scroll amount
            y: Vertical scroll amount (positive for up, negative for down)

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            self.mouse.scroll(x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll down by the specified number of clicks.

        Args:
            clicks: Number of scroll clicks to perform

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            self.mouse.scroll(0, -clicks)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll up by the specified number of clicks.

        Args:
            clicks: Number of scroll clicks to perform

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            self.mouse.scroll(0, clicks)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Screen Actions
    async def screenshot(self) -> Dict[str, Any]:
        """Capture a screenshot of the current screen.

        Returns:
            Dictionary containing success status and base64-encoded image data or error message
        """
        try:
            screenshot = ImageGrab.grab()
            if not isinstance(screenshot, Image.Image):
                return {"success": False, "error": "Failed to capture screenshot"}

            # Resize image to reduce size (max width 1920, maintain aspect ratio)
            max_width = 1920
            if screenshot.width > max_width:
                ratio = max_width / screenshot.width
                new_height = int(screenshot.height * ratio)
                screenshot = screenshot.resize((max_width, new_height), Image.Resampling.LANCZOS)

            buffered = BytesIO()
            # Use PNG format with optimization to reduce file size
            screenshot.save(buffered, format="PNG", optimize=True)
            buffered.seek(0)
            image_data = base64.b64encode(buffered.getvalue()).decode()
            return {"success": True, "image_data": image_data}
        except Exception as e:
            return {"success": False, "error": f"Screenshot error: {str(e)}"}

    async def get_screen_size(self) -> Dict[str, Any]:
        """Get the dimensions of the current screen.

        Returns:
            Dictionary containing success status and screen size or error message
        """
        try:
            img = ImageGrab.grab()
            return {"success": True, "size": {"width": img.width, "height": img.height}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        """Get the current position of the mouse cursor.

        Returns:
            Dictionary containing success status and cursor position or error message
        """
        try:
            x, y = self.mouse.position
            return {"success": True, "position": {"x": x, "y": y}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Clipboard Actions
    async def copy_to_clipboard(self) -> Dict[str, Any]:
        """Get the current content of the system clipboard.

        Returns:
            Dictionary containing success status and clipboard content or error message
        """
        try:
            import pyperclip

            content = pyperclip.paste()
            return {"success": True, "content": content}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        """Set the content of the system clipboard.

        Args:
            text: Text to copy to the clipboard

        Returns:
            Dictionary containing success status and error message if failed
        """
        try:
            import pyperclip

            pyperclip.copy(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def run_command(self, command: str) -> Dict[str, Any]:
        """Run a shell command and return its output.

        Args:
            command: Shell command to execute

        Returns:
            Dictionary containing success status, stdout, stderr, and return code
        """
        try:
            # Create subprocess
            process = await asyncio.create_subprocess_shell(
                command, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
            )
            # Wait for the subprocess to finish
            stdout, stderr = await process.communicate()
            # Return decoded output
            return {
                "success": True,
                "stdout": stdout.decode() if stdout else "",
                "stderr": stderr.decode() if stderr else "",
                "return_code": process.returncode,
            }
        except Exception as e:
            return {"success": False, "error": str(e)}
