#!/usr/bin/env python3
"""
UI Safezone Helper - A utility to get accurate bounds for macOS UI elements

This module provides helper functions to get accurate bounds for macOS UI elements
like the menubar and dock, which are needed for proper screenshot composition.
"""

import sys
import time
from typing import Any, Dict, Optional, Tuple

# Import Objective-C bridge libraries
try:
    import AppKit
    import Foundation
    from AppKit import NSRunningApplication, NSWorkspace
    from ApplicationServices import (
        AXUIElementCopyAttributeValue,
        AXUIElementCopyAttributeValues,
        AXUIElementCreateApplication,
        AXUIElementCreateSystemWide,
        AXUIElementGetTypeID,
        AXValueGetType,
        AXValueGetValue,
        kAXChildrenAttribute,
        kAXErrorSuccess,
        kAXMenuBarAttribute,
        kAXPositionAttribute,
        kAXRoleAttribute,
        kAXSizeAttribute,
        kAXTitleAttribute,
        kAXValueCGPointType,
        kAXValueCGSizeType,
    )
except ImportError:
    print("Error: This script requires PyObjC to be installed.")
    print("Please install it with: pip install pyobjc")
    sys.exit(1)

# Constants for accessibility API
kAXErrorSuccess = 0
kAXRoleAttribute = "AXRole"
kAXSubroleAttribute = "AXSubrole"
kAXTitleAttribute = "AXTitle"
kAXPositionAttribute = "AXPosition"
kAXSizeAttribute = "AXSize"
kAXChildrenAttribute = "AXChildren"
kAXMenuBarAttribute = "AXMenuBar"


def element_attribute(element, attribute):
    """Get an attribute from an accessibility element"""
    if attribute == kAXChildrenAttribute:
        err, value = AXUIElementCopyAttributeValues(element, attribute, 0, 999, None)
        if err == kAXErrorSuccess:
            if isinstance(value, Foundation.NSArray):
                return list(value)
            else:
                return value
    err, value = AXUIElementCopyAttributeValue(element, attribute, None)
    if err == kAXErrorSuccess:
        return value
    return None


def element_value(element, type):
    """Get a value from an accessibility element"""
    err, value = AXValueGetValue(element, type, None)
    if err == True:
        return value
    return None


def get_element_bounds(element):
    """Get the bounds of an accessibility element"""
    bounds = {"x": 0, "y": 0, "width": 0, "height": 0}

    # Get position
    position_value = element_attribute(element, kAXPositionAttribute)
    if position_value:
        position_value = element_value(position_value, kAXValueCGPointType)
        if position_value:
            bounds["x"] = position_value.x
            bounds["y"] = position_value.y

    # Get size
    size_value = element_attribute(element, kAXSizeAttribute)
    if size_value:
        size_value = element_value(size_value, kAXValueCGSizeType)
        if size_value:
            bounds["width"] = size_value.width
            bounds["height"] = size_value.height

    return bounds


def find_dock_process():
    """Find the Dock process"""
    running_apps = NSWorkspace.sharedWorkspace().runningApplications()
    for app in running_apps:
        if app.localizedName() == "Dock" and app.bundleIdentifier() == "com.apple.dock":
            return app.processIdentifier()
    return None


def get_menubar_bounds():
    """Get the bounds of the macOS menubar

    Returns:
        Dictionary with x, y, width, height of the menubar
    """
    # Get the system-wide accessibility element
    system_element = AXUIElementCreateSystemWide()

    # Try to find the menubar
    menubar = element_attribute(system_element, kAXMenuBarAttribute)
    if menubar is None:
        # If we can't get it directly, try through the frontmost app
        frontmost_app = NSWorkspace.sharedWorkspace().frontmostApplication()
        if frontmost_app:
            app_pid = frontmost_app.processIdentifier()
            app_element = AXUIElementCreateApplication(app_pid)
            menubar = element_attribute(app_element, kAXMenuBarAttribute)

    if menubar is None:
        print("Error: Could not get menubar")
        # Return default menubar bounds as fallback
        return {"x": 0, "y": 0, "width": 1800, "height": 24}

    # Get menubar bounds
    return get_element_bounds(menubar)


def get_dock_bounds():
    """Get the bounds of the macOS Dock

    Returns:
        Dictionary with x, y, width, height of the Dock
    """
    dock_pid = find_dock_process()
    if dock_pid is None:
        print("Error: Could not find Dock process")
        # Return empty bounds as fallback
        return {"x": 0, "y": 0, "width": 0, "height": 0}

    # Create an accessibility element for the Dock
    dock_element = AXUIElementCreateApplication(dock_pid)
    if dock_element is None:
        print(f"Error: Could not create accessibility element for Dock (PID {dock_pid})")
        return {"x": 0, "y": 0, "width": 0, "height": 0}

    # Get the Dock's children
    children = element_attribute(dock_element, kAXChildrenAttribute)
    if not children or len(children) == 0:
        print("Error: Could not get Dock children")
        return {"x": 0, "y": 0, "width": 0, "height": 0}

    # Find the Dock's list (first child is usually the main dock list)
    dock_list = None
    for child in children:
        role = element_attribute(child, kAXRoleAttribute)
        if role == "AXList":
            dock_list = child
            break

    if dock_list is None:
        print("Error: Could not find Dock list")
        return {"x": 0, "y": 0, "width": 0, "height": 0}

    # Get the bounds of the dock list
    return get_element_bounds(dock_list)


def get_ui_element_bounds():
    """Get the bounds of important UI elements like menubar and dock

    Returns:
        Dictionary with menubar and dock bounds
    """
    menubar_bounds = get_menubar_bounds()
    dock_bounds = get_dock_bounds()

    return {"menubar": menubar_bounds, "dock": dock_bounds}


if __name__ == "__main__":
    # Example usage
    bounds = get_ui_element_bounds()
    print("Menubar bounds:", bounds["menubar"])
    print("Dock bounds:", bounds["dock"])
