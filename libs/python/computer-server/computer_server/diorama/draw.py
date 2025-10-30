#!/usr/bin/env python3
"""Diorama Renderer - A tool for rendering selective views of macOS desktops

This script renders filtered views of the macOS desktop, preserving only selected applications
while maintaining system UI elements like menubar and dock. Each "diorama" shows a consistent
view of the system while isolating specific applications.

The image is "smart resized" to remove any empty space around the menubar and dock.

Key features:
- Captures shared window state, z-order and position information
- Filters windows by application based on whitelist
- Preserves system context (menubar, dock) in each view
- Preserves menu-owning / keyboard-focused window in each view
- Supports parallel views of the same desktop for multi-agent systems
"""

import argparse
import asyncio
import functools
import io
import json
import logging
import os
import sys
import time
from typing import Any, Dict, List, Optional, Tuple

from PIL import Image, ImageDraw

# simple, nicely formatted logging
logger = logging.getLogger(__name__)

from computer_server.diorama.safezone import (
    get_dock_bounds,
    get_menubar_bounds,
)


# Timing decorator for profiling
def timing_decorator(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        start_time = time.time()
        result = func(*args, **kwargs)
        end_time = time.time()
        elapsed_time = end_time - start_time
        logger.debug(f"Function {func.__name__} took {elapsed_time:.4f} seconds to run")
        return result

    return wrapper


# Import Objective-C bridge libraries
try:
    import AppKit
    import Foundation
    import objc
    import Quartz
    from AppKit import NSApp, NSApplication, NSRunningApplication, NSWorkspace
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
    from Foundation import NSMakeRect, NSObject
except ImportError:
    logger.error("Error: This script requires PyObjC to be installed.")
    logger.error("Please install it with: pip install pyobjc")
    sys.exit(1)

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
    def list_helper(list_value):
        list_builder = []
        for item in list_value:
            list_builder.append(CFAttributeToPyObject(item))
        return list_builder

    def number_helper(number_value):
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
    err, value = AXValueGetValue(element, type, None)
    if err == True:
        return value
    return None


@timing_decorator
def get_running_apps() -> List[NSRunningApplication]:
    """Get list of all running applications

    Returns:
        List of NSRunningApplication objects
    """
    return NSWorkspace.sharedWorkspace().runningApplications()


# @timing_decorator
def get_app_info(app: NSRunningApplication) -> Dict[str, Any]:
    """Get information about an application

    Args:
        app: NSRunningApplication object

    Returns:
        Dictionary with application information
    """
    return {
        "name": app.localizedName(),
        "bundle_id": app.bundleIdentifier(),
        "pid": app.processIdentifier(),
        "active": app.isActive(),
        "hidden": app.isHidden(),
        "terminated": app.isTerminated(),
    }


@timing_decorator
def get_all_windows() -> List[Dict[str, Any]]:
    """Get all windows from all applications with z-order information

    Returns:
        List of window dictionaries with z-order information
    """
    # Get all windows from Quartz
    # The kCGWindowListOptionOnScreenOnly flag gets only visible windows with preserved z-order
    window_list = Quartz.CGWindowListCopyWindowInfo(
        Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID
    )

    # Create a dictionary of window z-order
    z_order = {
        window["kCGWindowNumber"]: z_index for z_index, window in enumerate(window_list[::-1])
    }

    # The kCGWindowListOptionAll flag gets all windows *without* z-order preserved
    window_list_all = Quartz.CGWindowListCopyWindowInfo(
        Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID
    )

    # Process all windows
    windows = []
    for window in window_list_all:
        # We track z_index which is the index in the window list (0 is the desktop / background)

        # Get window properties
        window_id = window.get("kCGWindowNumber", 0)
        window_name = window.get("kCGWindowName", "")
        window_pid = window.get("kCGWindowOwnerPID", 0)
        window_bounds = window.get("kCGWindowBounds", {})
        window_owner = window.get("kCGWindowOwnerName", "")
        window_is_on_screen = window.get("kCGWindowIsOnscreen", False)

        # Get z-order information
        # Note: kCGWindowLayer provides the system's layer value (lower values are higher in the stack)
        layer = window.get(kCGWindowLayer, 0)
        opacity = window.get(kCGWindowAlpha, 1.0)
        z_index = z_order.get(window_id, -1)

        # Determine window role (desktop, dock, menubar, app)
        if window_name == "Dock" and window_owner == "Dock":
            role = "dock"
        elif window_name == "Menubar" and window_owner == "Window Server":
            role = "menubar"
        elif window_owner in ["Window Server", "Dock"]:
            role = "desktop"
        else:
            role = "app"

        # Only include windows with valid bounds
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
                    "layer": layer,  # System layer (lower values are higher in stack)
                    "z_index": z_index,  # Our z-index (0 is the desktop)
                    "opacity": opacity,
                }
            )

    windows = sorted(windows, key=lambda x: x["z_index"])

    return windows


def get_app_windows(app_pid: int, all_windows: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Get all windows for a specific application

    Args:
        app_pid: Process ID of the application
        all_windows: List of all windows with z-order information

    Returns:
        List of window dictionaries for the app
    """
    # Filter windows by PID
    return [window for window in all_windows if window["pid"] == app_pid]


@timing_decorator
def draw_desktop_screenshot(
    app_whitelist: List[str] = None,
    all_windows: List[Dict[str, Any]] = None,
    dock_bounds: Dict[str, float] = None,
    dock_items: List[Dict[str, Any]] = None,
    menubar_bounds: Dict[str, float] = None,
    menubar_items: List[Dict[str, Any]] = None,
) -> Tuple[Optional[Image.Image], List[Dict[str, Any]]]:
    """Capture a screenshot of the entire desktop using Quartz compositing, including dock as a second pass.
    Args:
        app_whitelist: Optional list of app names to include in the screenshot
    Returns:
        PIL Image of the desktop or None if capture failed
    """
    import ctypes

    if dock_bounds is None:
        dock_bounds = get_dock_bounds()
    if dock_items is None:
        dock_items = get_dock_items()
    if menubar_bounds is None:
        menubar_bounds = get_menubar_bounds()
    if menubar_items is None:
        menubar_items = get_menubar_items()
    if all_windows is None:
        all_windows = get_all_windows()
    all_windows = all_windows[::-1]
    all_windows = [window for window in all_windows if window["is_on_screen"]]

    main_screen = AppKit.NSScreen.mainScreen()
    if main_screen:
        frame = main_screen.frame()
        screen_rect = Quartz.CGRectMake(0, 0, frame.size.width, frame.size.height)
    else:
        screen_rect = Quartz.CGRectNull

    # Screenshot-to-screen hitboxes
    hitboxes = []

    if app_whitelist is None:
        # Single pass: desktop, menubar, app, dock
        window_list = Foundation.CFArrayCreateMutable(None, len(all_windows), None)
        for window in all_windows:
            Foundation.CFArrayAppendValue(window_list, window["id"])
        cg_image = Quartz.CGWindowListCreateImageFromArray(
            screen_rect, window_list, Quartz.kCGWindowImageDefault
        )
        if cg_image is None:
            return None

        # Create CGContext for compositing
        width = int(frame.size.width)
        height = int(frame.size.height)
        color_space = Quartz.CGColorSpaceCreateWithName(Quartz.kCGColorSpaceSRGB)
        cg_context = Quartz.CGBitmapContextCreate(
            None, width, height, 8, 0, color_space, Quartz.kCGImageAlphaPremultipliedLast
        )
        Quartz.CGContextDrawImage(cg_context, screen_rect, cg_image)
        hitboxes.append({"hitbox": [0, 0, width, height], "target": [0, 0, width, height]})
    else:
        # Filter out windows that are not in the whitelist
        all_windows = [
            window
            for window in all_windows
            if window["owner"] in app_whitelist or window["role"] != "app"
        ]
        app_windows = [window for window in all_windows if window["role"] == "app"]

        dock_orientation = "side" if dock_bounds["width"] < dock_bounds["height"] else "bottom"

        menubar_length = (
            max(item["bounds"]["x"] + item["bounds"]["width"] for item in menubar_items)
            if menubar_items
            else 0
        )

        # Calculate bounds of app windows
        app_bounds = {
            "x": min(window["bounds"]["x"] for window in app_windows) if app_windows else 0,
            "y": min(window["bounds"]["y"] for window in app_windows) if app_windows else 0,
        }
        app_bounds["width"] = (
            max(window["bounds"]["x"] + window["bounds"]["width"] for window in app_windows)
            - app_bounds["x"]
            if app_windows
            else 0
        )
        app_bounds["height"] = (
            max(window["bounds"]["y"] + window["bounds"]["height"] for window in app_windows)
            - app_bounds["y"]
            if app_windows
            else 0
        )

        # Set minimum bounds of 256x256
        app_bounds["width"] = max(app_bounds["width"], 256)
        app_bounds["height"] = max(app_bounds["height"], 256)

        # Add dock bounds to app bounds
        if dock_orientation == "bottom":
            app_bounds["height"] += dock_bounds["height"] + 4
        elif dock_orientation == "side":
            if dock_bounds["x"] > frame.size.width / 2:
                app_bounds["width"] += dock_bounds["width"] + 4
            else:
                app_bounds["x"] -= dock_bounds["width"] + 4
                app_bounds["width"] += dock_bounds["width"] + 4

        # Add menubar bounds to app bounds
        app_bounds["height"] += menubar_bounds["height"]

        # Make sure app bounds contains menubar bounds
        app_bounds["width"] = max(app_bounds["width"], menubar_length)

        # Clamp bounds to screen
        app_bounds["x"] = max(app_bounds["x"], 0)
        app_bounds["y"] = max(app_bounds["y"], 0)
        app_bounds["width"] = min(app_bounds["width"], frame.size.width - app_bounds["x"])
        app_bounds["height"] = min(
            app_bounds["height"], frame.size.height - app_bounds["y"] + menubar_bounds["height"]
        )

        # Create CGContext for compositing
        width = int(app_bounds["width"])
        height = int(app_bounds["height"])
        color_space = Quartz.CGColorSpaceCreateWithName(Quartz.kCGColorSpaceSRGB)
        cg_context = Quartz.CGBitmapContextCreate(
            None, width, height, 8, 0, color_space, Quartz.kCGImageAlphaPremultipliedLast
        )

        def _draw_layer(cg_context, all_windows, source_rect, target_rect):
            """Draw a layer of windows from source_rect to target_rect on the given context."""
            window_list = Foundation.CFArrayCreateMutable(None, len(all_windows), None)
            for window in all_windows:
                Foundation.CFArrayAppendValue(window_list, window["id"])
            cg_image = Quartz.CGWindowListCreateImageFromArray(
                source_rect, window_list, Quartz.kCGWindowImageDefault
            )
            if cg_image is not None:
                Quartz.CGContextDrawImage(cg_context, target_rect, cg_image)

        # --- FIRST PASS: desktop, apps ---
        source_position = [app_bounds["x"], app_bounds["y"]]
        source_size = [app_bounds["width"], app_bounds["height"]]
        target_position = [0, min(menubar_bounds["y"] + menubar_bounds["height"], app_bounds["y"])]
        target_size = [app_bounds["width"], app_bounds["height"]]

        if dock_orientation == "bottom":
            source_size[1] += dock_bounds["height"]
            target_size[1] += dock_bounds["height"]
        elif dock_orientation == "side":
            if dock_bounds["x"] < frame.size.width / 2:
                source_position[0] -= dock_bounds["width"]
                target_position[0] -= dock_bounds["width"]
            source_size[0] += dock_bounds["width"]
            target_size[0] += dock_bounds["width"]

        app_source_rect = Quartz.CGRectMake(
            source_position[0], source_position[1], source_size[0], source_size[1]
        )
        app_target_rect = Quartz.CGRectMake(
            target_position[0],
            app_bounds["height"] - target_position[1] - target_size[1],
            target_size[0],
            target_size[1],
        )
        first_pass_windows = [
            w for w in all_windows if w["role"] == "app" or w["role"] == "desktop"
        ]
        _draw_layer(cg_context, first_pass_windows, app_source_rect, app_target_rect)

        hitboxes.append(
            {
                "hitbox": [
                    0,
                    menubar_bounds["height"],
                    app_bounds["width"],
                    menubar_bounds["height"] + app_bounds["height"],
                ],
                "target": [
                    app_source_rect.origin.x,
                    app_source_rect.origin.y,
                    app_source_rect.origin.x + app_bounds["width"],
                    app_source_rect.origin.y + app_bounds["height"],
                ],
            }
        )

        # --- SECOND PASS: menubar ---
        allowed_roles = {"menubar"}
        menubar_windows = [w for w in all_windows if w["role"] in allowed_roles]
        menubar_source_rect = Quartz.CGRectMake(0, 0, app_bounds["width"], menubar_bounds["height"])
        menubar_target_rect = Quartz.CGRectMake(
            0,
            app_bounds["height"] - menubar_bounds["height"],
            app_bounds["width"],
            menubar_bounds["height"],
        )
        _draw_layer(cg_context, menubar_windows, menubar_source_rect, menubar_target_rect)

        hitboxes.append(
            {
                "hitbox": [0, 0, app_bounds["width"], menubar_bounds["height"]],
                "target": [0, 0, app_bounds["width"], menubar_bounds["height"]],
            }
        )

        # --- THIRD PASS: dock, filtered ---
        # Step 1: Collect dock items to draw, with their computed target rects
        dock_draw_items = []
        for index, item in enumerate(dock_items):
            source_position = (item["bounds"]["x"], item["bounds"]["y"])
            source_size = (item["bounds"]["width"], item["bounds"]["height"])

            # apply whitelist to middle items
            if not (index == 0 or index == len(dock_items) - 1):
                if item["subrole"] == "AXApplicationDockItem":
                    if item["title"] not in app_whitelist:
                        continue
                elif item["subrole"] == "AXMinimizedWindowDockItem":
                    if not any(
                        window["name"] == item["title"]
                        and window["role"] == "app"
                        and window["owner"] in app_whitelist
                        for window in all_windows
                    ):
                        continue
                elif item["subrole"] == "AXFolderDockItem":
                    continue

            # Preserve unscaled (original) source position and size before any modification
            hitbox_position = source_position
            hitbox_size = source_size

            screen_position = source_position
            screen_size = source_size

            # stretch to screen size
            padding = 32
            if dock_orientation == "bottom":
                source_position = (source_position[0], 0)
                source_size = (source_size[0], frame.size.height)

                hitbox_position = (source_position[0], app_bounds["height"] - hitbox_size[1])
                hitbox_size = (source_size[0], hitbox_size[1])

                if index == 0:
                    source_size = (padding + source_size[0], source_size[1])
                    source_position = (source_position[0] - padding, 0)
                elif index == len(dock_items) - 1:
                    source_size = (source_size[0] + padding, source_size[1])
                    source_position = (source_position[0], 0)

            elif dock_orientation == "side":
                source_position = (0, source_position[1])
                source_size = (frame.size.width, source_size[1])

                hitbox_position = (
                    (
                        source_position[0]
                        if dock_bounds["x"] < frame.size.width / 2
                        else app_bounds["width"] - hitbox_size[0]
                    ),
                    source_position[1],
                )
                hitbox_size = (hitbox_size[0], source_size[1])

                if index == 0:
                    source_size = (source_size[0], padding + source_size[1])
                    source_position = (0, source_position[1] - padding)
                elif index == len(dock_items) - 1:
                    source_size = (source_size[0], source_size[1] + padding)
                    source_position = (0, source_position[1])

            # Compute the initial target position
            target_position = source_position
            target_size = source_size

            dock_draw_items.append(
                {
                    "item": item,
                    "index": index,
                    "source_position": source_position,
                    "source_size": source_size,
                    "target_size": target_size,
                    "target_position": target_position,  # Will be updated after packing
                    "hitbox_position": hitbox_position,
                    "hitbox_size": hitbox_size,
                    "screen_position": screen_position,
                    "screen_size": screen_size,
                }
            )

        # Step 2: Pack the target rects along the main axis, removing gaps
        packed_positions = []
        if dock_orientation == "bottom":
            # Pack left-to-right
            x_cursor = 0
            for draw_item in dock_draw_items:
                packed_positions.append((x_cursor, draw_item["target_position"][1]))
                x_cursor += draw_item["target_size"][0]
            packed_strip_length = x_cursor
            # Center horizontally
            x_offset = (app_bounds["width"] - packed_strip_length) / 2
            y_offset = frame.size.height - app_bounds["height"]
            for i, draw_item in enumerate(dock_draw_items):
                px, py = packed_positions[i]
                draw_item["target_position"] = (px + x_offset, py - y_offset)

            # Pack unscaled source rects
            x_cursor = 0
            for draw_item in dock_draw_items:
                draw_item["hitbox_position"] = (x_cursor, draw_item["hitbox_position"][1])
                x_cursor += draw_item["hitbox_size"][0]
            packed_strip_length = x_cursor
            # Center horizontally
            x_offset = (app_bounds["width"] - packed_strip_length) / 2
            for i, draw_item in enumerate(dock_draw_items):
                px, py = draw_item["hitbox_position"]
                draw_item["hitbox_position"] = (px + x_offset, py)
        elif dock_orientation == "side":
            # Pack top-to-bottom
            y_cursor = 0
            for draw_item in dock_draw_items:
                packed_positions.append((draw_item["target_position"][0], y_cursor))
                y_cursor += draw_item["target_size"][1]
            packed_strip_length = y_cursor
            # Center vertically
            y_offset = (app_bounds["height"] - packed_strip_length) / 2
            x_offset = (
                0
                if dock_bounds["x"] < frame.size.width / 2
                else frame.size.width - app_bounds["width"]
            )
            for i, draw_item in enumerate(dock_draw_items):
                px, py = packed_positions[i]
                draw_item["target_position"] = (px - x_offset, py + y_offset)

            # Pack unscaled source rects
            y_cursor = 0
            for draw_item in dock_draw_items:
                draw_item["hitbox_position"] = (draw_item["hitbox_position"][0], y_cursor)
                y_cursor += draw_item["hitbox_size"][1]
            packed_strip_length = y_cursor
            # Center vertically
            y_offset = (app_bounds["height"] - packed_strip_length) / 2
            for i, draw_item in enumerate(dock_draw_items):
                px, py = draw_item["hitbox_position"]
                draw_item["hitbox_position"] = (px, py + y_offset)

        dock_windows = [window for window in all_windows if window["role"] == "dock"]
        # Step 3: Draw dock items using packed and recentered positions
        for draw_item in dock_draw_items:
            item = draw_item["item"]
            source_position = draw_item["source_position"]
            source_size = draw_item["source_size"]
            target_position = draw_item["target_position"]
            target_size = draw_item["target_size"]

            # flip target position y
            target_position = (
                target_position[0],
                app_bounds["height"] - target_position[1] - target_size[1],
            )

            source_rect = Quartz.CGRectMake(*source_position, *source_size)
            target_rect = Quartz.CGRectMake(*target_position, *target_size)

            _draw_layer(cg_context, dock_windows, source_rect, target_rect)

            hitbox_position = draw_item["hitbox_position"]
            hitbox_size = draw_item["hitbox_size"]

            # Debug: Draw true hitbox rect (packed position, unscaled size)
            # # Flip y like target_rect
            # hitbox_position_flipped = (
            #     hitbox_position[0],
            #     app_bounds['height'] - hitbox_position[1] - hitbox_size[1]
            # )
            # hitbox_rect = Quartz.CGRectMake(*hitbox_position_flipped, *hitbox_size)
            # Quartz.CGContextSetStrokeColorWithColor(cg_context, Quartz.CGColorCreateGenericRGB(0, 1, 0, 1))
            # Quartz.CGContextStrokeRect(cg_context, hitbox_rect)

            hitboxes.append(
                {
                    "hitbox": [
                        *hitbox_position,
                        hitbox_position[0] + hitbox_size[0],
                        hitbox_position[1] + hitbox_size[1],
                    ],
                    "target": [
                        *draw_item["screen_position"],
                        draw_item["screen_position"][0] + draw_item["screen_size"][0],
                        draw_item["screen_position"][1] + draw_item["screen_size"][1],
                    ],
                }
            )

    # Convert composited context to CGImage
    final_cg_image = Quartz.CGBitmapContextCreateImage(cg_context)
    ns_image = AppKit.NSImage.alloc().initWithCGImage_size_(final_cg_image, Foundation.NSZeroSize)
    ns_data = ns_image.TIFFRepresentation()
    bitmap_rep = AppKit.NSBitmapImageRep.imageRepWithData_(ns_data)
    png_data = bitmap_rep.representationUsingType_properties_(AppKit.NSBitmapImageFileTypePNG, None)
    image_data = io.BytesIO(png_data)
    return Image.open(image_data), hitboxes


@timing_decorator
def get_menubar_items(active_app_pid: int = None) -> List[Dict[str, Any]]:
    """Get menubar items from the active application using Accessibility API

    Args:
        active_app_pid: PID of the active application

    Returns:
        List of dictionaries with menubar item information
    """
    menubar_items = []

    if active_app_pid is None:
        # Get the frontmost application's PID if none provided
        frontmost_app = NSWorkspace.sharedWorkspace().frontmostApplication()
        if frontmost_app:
            active_app_pid = frontmost_app.processIdentifier()
        else:
            logger.error("Error: Could not determine frontmost application")
            return menubar_items

    # Create an accessibility element for the application
    app_element = AXUIElementCreateApplication(active_app_pid)
    if app_element is None:
        logger.error(f"Error: Could not create accessibility element for PID {active_app_pid}")
        return menubar_items

    # Get the menubar
    menubar = element_attribute(app_element, kAXMenuBarAttribute)
    if menubar is None:
        logger.error(f"Error: Could not get menubar for application with PID {active_app_pid}")
        return menubar_items

    # Get the menubar items
    children = element_attribute(menubar, kAXChildrenAttribute)
    if children is None:
        logger.error("Error: Could not get menubar items")
        return menubar_items

    # Process each menubar item
    for i in range(len(children)):
        item = children[i]

        # Get item title
        title = element_attribute(item, kAXTitleAttribute) or "Untitled"

        # Create bounding box
        bounds = {"x": 0, "y": 0, "width": 0, "height": 0}

        # Get item position
        position_value = element_attribute(item, kAXPositionAttribute)
        if position_value:
            position_value = element_value(position_value, kAXValueCGPointType)
            bounds["x"] = position_value.x
            bounds["y"] = position_value.y

        # Get item size
        size_value = element_attribute(item, kAXSizeAttribute)
        if size_value:
            size_value = element_value(size_value, kAXValueCGSizeType)
            bounds["width"] = size_value.width
            bounds["height"] = size_value.height

        # Add to list
        menubar_items.append(
            {"title": title, "bounds": bounds, "index": i, "app_pid": active_app_pid}
        )

    return menubar_items


@timing_decorator
def get_dock_items() -> List[Dict[str, Any]]:
    """Get all items in the macOS Dock

    Returns:
        List of dictionaries with Dock item information
    """
    dock_items = []

    # Find the Dock process
    dock_pid = None
    running_apps = get_running_apps()
    for app in running_apps:
        if app.localizedName() == "Dock" and app.bundleIdentifier() == "com.apple.dock":
            dock_pid = app.processIdentifier()
            break

    if dock_pid is None:
        logger.error("Error: Could not find Dock process")
        return dock_items

    # Create an accessibility element for the Dock
    dock_element = AXUIElementCreateApplication(dock_pid)
    if dock_element is None:
        logger.error(f"Error: Could not create accessibility element for Dock (PID {dock_pid})")
        return dock_items

    # Get the Dock's main element
    dock_list = element_attribute(dock_element, kAXChildrenAttribute)
    if dock_list is None or len(dock_list) == 0:
        logger.error("Error: Could not get Dock children")
        return dock_items

    # Find the Dock's application list (usually the first child)
    dock_app_list = None
    for child in dock_list:
        role = element_attribute(child, kAXRoleAttribute)
        if role == "AXList":
            dock_app_list = child
            break

    if dock_app_list is None:
        logger.error("Error: Could not find Dock application list")
        return dock_items

    # Get all items in the Dock
    items = element_attribute(dock_app_list, kAXChildrenAttribute)
    if items is None:
        logger.error("Error: Could not get Dock items")
        return dock_items

    # Process each Dock item
    for i, item in enumerate(items):
        # Get item attributes
        title = element_attribute(item, kAXTitleAttribute) or "Untitled"
        description = element_attribute(item, "AXDescription") or ""
        role = element_attribute(item, kAXRoleAttribute) or ""
        subrole = element_attribute(item, "AXSubrole") or ""

        # Create bounding box
        bounds = {"x": 0, "y": 0, "width": 0, "height": 0}

        # Get item position
        position_value = element_attribute(item, kAXPositionAttribute)
        if position_value:
            position_value = element_value(position_value, kAXValueCGPointType)
            bounds["x"] = position_value.x
            bounds["y"] = position_value.y

        # Get item size
        size_value = element_attribute(item, kAXSizeAttribute)
        if size_value:
            size_value = element_value(size_value, kAXValueCGSizeType)
            bounds["width"] = size_value.width
            bounds["height"] = size_value.height

        # Determine if this is an application, file/folder, or separator
        item_type = "unknown"
        if subrole == "AXApplicationDockItem":
            item_type = "application"
        elif subrole == "AXFolderDockItem":
            item_type = "folder"
        elif subrole == "AXDocumentDockItem":
            item_type = "document"
        elif subrole == "AXSeparatorDockItem" or role == "AXSeparator":
            item_type = "separator"
        elif "trash" in title.lower():
            item_type = "trash"

        # Add to list
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


class AppActivationContext:
    def __init__(self, active_app_pid=None, active_app_to_use="", logger=None):
        self.active_app_pid = active_app_pid
        self.active_app_to_use = active_app_to_use
        self.logger = logger
        self.frontmost_app = None

    def __enter__(self):
        from AppKit import NSWorkspace

        if self.active_app_pid:
            if self.logger and self.active_app_to_use:
                self.logger.debug(
                    f"Automatically activating app '{self.active_app_to_use}' for screenshot composition"
                )
            self.frontmost_app = NSWorkspace.sharedWorkspace().frontmostApplication()
            running_apps_list = NSWorkspace.sharedWorkspace().runningApplications()
            for app in running_apps_list:
                if app.processIdentifier() == self.active_app_pid:
                    app.activateWithOptions_(0)
                    # sleep for 0.5 seconds
                    time.sleep(0.5)
                    break
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.frontmost_app:
            # sleep for 0.5 seconds
            time.sleep(0.5)
            self.frontmost_app.activateWithOptions_(0)


def get_frontmost_and_active_app(all_windows, running_apps, app_whitelist):
    from AppKit import NSWorkspace

    frontmost_app = NSWorkspace.sharedWorkspace().frontmostApplication()

    active_app_to_use = None
    active_app_pid = None

    # Find the topmost (highest z_index) non-filtered app
    for window in reversed(all_windows):
        owner = window.get("owner")
        role = window.get("role")
        is_on_screen = window.get("is_on_screen")

        # Skip non-app windows
        if role != "app":
            continue

        # Skip not-on-screen windows
        if not is_on_screen:
            continue

        # Skip filtered apps
        if app_whitelist is not None and owner not in app_whitelist:
            continue

        # Found a suitable app
        active_app_to_use = owner
        active_app_pid = window.get("pid")
        break

    # If no suitable app found, use Finder
    if active_app_to_use is None:
        active_app_to_use = "Finder"
        for app in running_apps:
            if app.localizedName() == "Finder":
                active_app_pid = app.processIdentifier()
                break

    return frontmost_app, active_app_to_use, active_app_pid


def capture_all_apps(
    save_to_disk: bool = False,
    app_whitelist: List[str] = None,
    output_dir: str = None,
    take_focus: bool = True,
) -> Tuple[Dict[str, Any], Optional[Image.Image]]:
    """Capture screenshots of all running applications

    Args:
        save_to_disk: Whether to save screenshots to disk
        app_whitelist: Optional list of app names to include in the recomposited screenshot
                    (will always include 'Window Server' and 'Dock')

    Returns:
        Dictionary with application information and screenshots
        Optional PIL Image of the recomposited screenshot
    """
    result = {
        "timestamp": time.time(),
        "applications": [],
        "windows": [],  # New array to store all windows, including those without apps
        "menubar_items": [],  # New array to store menubar items
        "dock_items": [],  # New array to store dock items
    }

    # Get all windows with z-order information
    all_windows = get_all_windows()

    # Get all running applications
    running_apps = get_running_apps()

    frontmost_app, active_app_to_use, active_app_pid = (
        get_frontmost_and_active_app(all_windows, running_apps, app_whitelist)
        if take_focus
        else (None, None, None)
    )

    # Use AppActivationContext to activate the app and restore focus
    with AppActivationContext(active_app_pid, active_app_to_use, logger):

        # Process applications
        for app in running_apps:
            # Skip system apps without a bundle ID
            if app.bundleIdentifier() is None:
                continue

            app_info = get_app_info(app)
            app_windows = get_app_windows(app.processIdentifier(), all_windows)

            app_data = {"info": app_info, "windows": [window["id"] for window in app_windows]}

            result["applications"].append(app_data)

        # Add all windows to the result
        result["windows"] = all_windows

        # Get menubar items from the active application
        menubar_items = get_menubar_items(active_app_pid)
        result["menubar_items"] = menubar_items

        # Get dock items
        dock_items = get_dock_items()
        result["dock_items"] = dock_items

        # Get menubar bounds
        menubar_bounds = get_menubar_bounds()
        result["menubar_bounds"] = menubar_bounds

        # Get dock bounds
        dock_bounds = get_dock_bounds()
        result["dock_bounds"] = dock_bounds

        # Capture the entire desktop using Quartz compositing
        desktop_screenshot, hitboxes = draw_desktop_screenshot(
            app_whitelist, all_windows, dock_bounds, dock_items, menubar_bounds, menubar_items
        )

        result["hitboxes"] = hitboxes

        from PIL import Image, ImageChops, ImageDraw

        def _draw_hitboxes(img, hitboxes, key="target"):
            """
            Overlay opaque colored rectangles for each hitbox (using hitbox[key])
            with color depending on index, then multiply overlay onto img.
            Args:
                img: PIL.Image (RGBA or RGB)
                hitboxes: list of dicts with 'hitbox' and 'target' keys
                key: 'hitbox' or 'target'
            Returns:
                PIL.Image with overlayed hitboxes (same mode/size as input)
            """
            # Ensure RGBA mode for blending
            base = img.convert("RGBA")
            overlay = Image.new("RGBA", base.size, (0, 0, 0, 0))
            draw = ImageDraw.Draw(overlay)

            # Distinct colors for order
            colors = [
                (255, 0, 0, 180),  # Red
                (0, 255, 0, 180),  # Green
                (0, 0, 255, 180),  # Blue
                (255, 255, 0, 180),  # Yellow
                (0, 255, 255, 180),  # Cyan
                (255, 0, 255, 180),  # Magenta
                (255, 128, 0, 180),  # Orange
                (128, 0, 255, 180),  # Purple
                (0, 128, 255, 180),  # Sky blue
                (128, 255, 0, 180),  # Lime
            ]
            # Set minimum brightness for colors
            min_brightness = 0
            colors = [
                (
                    max(min_brightness, c[0]),
                    max(min_brightness, c[1]),
                    max(min_brightness, c[2]),
                    c[3],
                )
                for c in colors
            ]

            for i, h in enumerate(hitboxes):
                rect = h.get(key)
                color = colors[i % len(colors)]
                if rect:
                    draw.rectangle(rect, fill=color)

            # Multiply blend overlay onto base
            result = ImageChops.multiply(base, overlay)
            return result

        # DEBUG: Save hitboxes to disk
        if desktop_screenshot and save_to_disk and output_dir:
            desktop_path = os.path.join(output_dir, "desktop.png")
            desktop_screenshot.save(desktop_path)
            result["desktop_screenshot"] = desktop_path

            logger.info(f"Saved desktop screenshot to {desktop_path}")

            if app_whitelist:
                # Take screenshot without whitelist
                desktop_screenshot_full, hitboxes_full = draw_desktop_screenshot(
                    None, all_windows, dock_bounds, dock_items, menubar_bounds, menubar_items
                )

                # Draw hitboxes on both images using overlay
                img1 = _draw_hitboxes(desktop_screenshot.copy(), hitboxes, key="hitbox")
                img2 = (
                    _draw_hitboxes(desktop_screenshot_full.copy(), hitboxes, key="target")
                    if desktop_screenshot_full
                    else None
                )

                if img2 and hitboxes_full:

                    # Compose side-by-side
                    from PIL import Image

                    width = img1.width + img2.width
                    height = max(img1.height, img2.height)
                    combined = Image.new("RGBA", (width, height), (0, 0, 0, 0))
                    combined.paste(img1, (0, 0))
                    combined.paste(img2, (img1.width, 0))
                    side_by_side_path = os.path.join(output_dir, "side_by_side_hitboxes.png")
                    combined.save(side_by_side_path)
                    result["side_by_side_hitboxes"] = side_by_side_path
            else:
                # Overlay hitboxes using new function
                hitbox_img = _draw_hitboxes(desktop_screenshot.copy(), hitboxes, key="hitbox")
                hitbox_path = os.path.join(output_dir, "hitboxes.png")
                hitbox_img.save(hitbox_path)
                result["hitbox_screenshot"] = hitbox_path

        # Focus restoration is now handled by AppActivationContext

    return result, desktop_screenshot


async def run_capture():
    """Run the screenshot capture asynchronously"""
    # Parse command line arguments
    parser = argparse.ArgumentParser(
        description="Capture screenshots of running macOS applications"
    )
    parser.add_argument(
        "--output", "-o", help="Output directory for screenshots", default="app_screenshots"
    )
    parser.add_argument(
        "--filter",
        "-f",
        nargs="+",
        help="Filter recomposited screenshot to only include specified apps",
    )
    parser.add_argument(
        "--menubar",
        "-m",
        action="store_true",
        help="List menubar and status items with their bounding boxes",
    )
    parser.add_argument(
        "--dock", "-d", action="store_true", help="List Dock items with their bounding boxes"
    )
    parser.add_argument(
        "--demo",
        nargs="*",
        help="Demo mode: pass app names to capture individual and combinations, create mosaic PNG",
    )
    args = parser.parse_args()

    # Create output directory in the current directory if not absolute
    if not os.path.isabs(args.output):
        output_dir = os.path.join(os.getcwd(), args.output)
    else:
        output_dir = args.output

    # DEMO MODE: capture each app and all non-empty combinations, then mosaic
    if args.demo:
        from PIL import Image

        demo_apps = args.demo
        print(f"Running in DEMO mode for apps: {demo_apps}")
        groups = []
        for item in demo_apps:
            if "/" in item:
                group = [x.strip() for x in item.split("/") if x.strip()]
            else:
                group = [item.strip()]
            if group:
                groups.append(group)
        screenshots = []
        for group in groups:
            print(f"Capturing for apps: {group}")
            _, img = capture_all_apps(app_whitelist=group)
            if img:
                screenshots.append((group, img))
        if not screenshots:
            print("No screenshots captured in demo mode.")
            return

        # Mosaic-pack: grid (rows of sqrt(N))
        def make_mosaic(images, pad=64, bg=(30, 30, 30)):
            import rpack

            sizes = [(img.width + pad, img.height + pad) for _, img in images]
            positions = rpack.pack(sizes)
            # Find the bounding box for the mosaic
            max_x = max(x + w for (x, y), (w, h) in zip(positions, sizes))
            max_y = max(y + h for (x, y), (w, h) in zip(positions, sizes))
            mosaic = Image.new("RGBA", (max_x, max_y), bg)
            for (group, img), (x, y) in zip(images, positions):
                mosaic.paste(img, (x, y))
            return mosaic

        mosaic_img = make_mosaic(screenshots)
        mosaic_path = os.path.join(output_dir, "demo_mosaic.png")
        os.makedirs(output_dir, exist_ok=True)
        mosaic_img.save(mosaic_path)
        print(f"Demo mosaic saved to: {mosaic_path}")
        return

    # Capture all apps and save to disk, including a recomposited screenshot
    print("Capturing screenshots of all running applications...")
    print(f"Saving screenshots to: {output_dir}")

    # If filter is provided, show what we're filtering by
    if args.filter:
        print(
            f"Filtering recomposited screenshot to only include: {', '.join(args.filter)} (plus Window Server and Dock)"
        )

    result, img = capture_all_apps(
        save_to_disk=True, app_whitelist=args.filter, output_dir=output_dir, take_focus=True
    )

    # Print summary
    print("\nCapture complete!")
    print(f"Captured {len(result['applications'])} applications")

    total_app_windows = sum(len(app["windows"]) for app in result["applications"])
    print(f"Total application windows captured: {total_app_windows}")
    print(f"Total standalone windows captured: {len(result['windows'])}")

    # Print details of each application
    print("\nApplication details:")
    for app in result["applications"]:
        app_info = app["info"]
        windows = app["windows"]
        print(f"  - {app_info['name']} ({len(windows)} windows)")

    # Print recomposited screenshot path if available
    if "desktop_screenshot" in result:
        print(f"\nRecomposited screenshot saved to: {result['desktop_screenshot']}")

    # Print menubar items if requested
    if args.menubar and "menubar_items" in result:
        print("\nMenubar items:")

        # Find app name for the PID
        app_name_by_pid = {}
        for app in result["applications"]:
            app_info = app["info"]
            app_name_by_pid[app_info["pid"]] = app_info["name"]

        for item in result["menubar_items"]:
            print(f"  - {item['title']}")
            print(
                f"    Bounds: x={item['bounds']['x']}, y={item['bounds']['y']}, width={item['bounds']['width']}, height={item['bounds']['height']}"
            )

            if "app_pid" in item:
                app_name = app_name_by_pid.get(
                    item["app_pid"], f"Unknown App (PID: {item['app_pid']})"
                )
                print(f"    App: {app_name} (PID: {item['app_pid']})")

            if "window_id" in item:
                print(f"    Window ID: {item['window_id']}")
            if "owner" in item:
                print(f"    Owner: {item['owner']}")
            if "layer" in item and "z_index" in item:
                print(f"    Layer: {item['layer']}, Z-Index: {item['z_index']}")
            print("")

    # Print dock items if requested
    if args.dock and "dock_items" in result:
        print("\nDock items:")
        for item in result["dock_items"]:
            print(f"  - {item['title']} ({item['type']})")
            print(f"    Description: {item['description']}")
            print(
                f"    Bounds: x={item['bounds']['x']}, y={item['bounds']['y']}, width={item['bounds']['width']}, height={item['bounds']['height']}"
            )
            print(f"    Role: {item['role']}, Subrole: {item['subrole']}")
            print(f"    Index: {item['index']}")
            print("")

    # Save the metadata to a JSON file
    metadata_path = os.path.join(output_dir, "metadata.json")
    with open(metadata_path, "w") as f:
        json.dump(result, f, indent=2)

    print(f"\nMetadata saved to: {metadata_path}")


if __name__ == "__main__":
    asyncio.run(run_capture())
