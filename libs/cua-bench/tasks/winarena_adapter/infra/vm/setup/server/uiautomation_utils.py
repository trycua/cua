import os
from pathlib import Path
import time
import comtypes.client
from ctypes import windll
import logging
import re

logger = logging.getLogger("uiautomation_utils")

def find_element_by_name(element, iui, name=None, name_re=None, debug=False):
    """
    Recursively search for an element by its name property.
    If name is provided, the function will search for an element with that name.
    If name_re is provided, the function will search for an element with a name that matches the regular expression.

    Args:
        element: The root element to start searching from.
        iui: The UIAutomation interface instance.
        name: The name of the element to find.
        name_re: A regular expression to match the name of the element.
        debug: Whether to print debug information.

    Returns:
        The first UI element that matches the name, or None if not found.
    """
    if name:
        UIA_NamePropertyId = 30005
        TreeScope_Children = 0x2

        condition = iui.CreatePropertyCondition(UIA_NamePropertyId, name)
        found_element = element.FindFirst(TreeScope_Children, condition)

        if found_element:
            return found_element

    walker = iui.ControlViewWalker
    child = walker.GetFirstChildElement(element)
    if debug:
        print("Element name: {}".format(element.CurrentName))

    if name_re and re.match(name_re, element.CurrentName):
        return element

    while child:
        result = find_element_by_name(child, iui, name, name_re, debug)
        if result:
            return result
        child = walker.GetNextSiblingElement(child)
    
    return None

def get_file_list_from_explorer(directory):
    """
    Get the list of files as displayed in the currently active Explorer window.

    Returns:
        A list of filenames in the order displayed by Windows Explorer.
    """
    # Load UIAutomation
    uia = comtypes.client.GetModule('UIAutomationCore.dll')
    iui = comtypes.client.CreateObject(uia.CUIAutomation)

    # Get the active Explorer window
    hwnd = windll.user32.GetForegroundWindow()
    element = iui.ElementFromHandle(hwnd)

    # Recursively search for the "Shell Folder View" element
    file_panel = find_element_by_name(element, iui, "Shell Folder View")

    if not file_panel:
        print("File panel not found.")
        return []

    try:
        # Get the list of files from the file panel
        walker = iui.ControlViewWalker
        file_names = []
        items_list = walker.GetFirstChildElement(file_panel)
        item = walker.GetFirstChildElement(items_list)
        
        while item:
            full_path = os.path.join(directory, item.CurrentName)
            print(full_path)
            if Path(full_path).exists(): # Filter out "Header" which can also be accessed by UIA.
                file_names.append(item.CurrentName)
            item = walker.GetNextSiblingElement(item)
        
        return file_names
    except Exception.COMError as e:
        print(f"Error occurred: {e}")
        return []
    
def is_file_explorer_details_view(directory):
    """
    Check if the current Explorer window is in details view.
    Since there is no direct way to check the view mode, we check if the there is a 
    "header" element in the file panel, which is distinct to details view.
    Returns:
        True if the current Explorer window is in details view, False otherwise.
    """
    # Load UIAutomation
    uia = comtypes.client.GetModule('UIAutomationCore.dll')
    iui = comtypes.client.CreateObject(uia.CUIAutomation)
    # Get the active Explorer window
    hwnd = windll.user32.GetForegroundWindow()
    element = iui.ElementFromHandle(hwnd)
    # Recursively search for the "Shell Folder View" element
    file_panel = find_element_by_name(element, iui, "Shell Folder View")
    if not file_panel:
        logger.error("File panel not found.")
        return False
    try:
        # Get the list view from the file panel
        walker = iui.ControlViewWalker
        items_list = walker.GetFirstChildElement(file_panel)
        
        # Look for the "Header" element
        item = walker.GetFirstChildElement(items_list)
        while item:
            full_path = os.path.join(directory, item.CurrentName)
            print(full_path)
            if not Path(full_path).exists(): # Look for "Header" element in the children of the file panel.

                if item.CurrentName == "Header":
                    return True
            item = walker.GetNextSiblingElement(item)
        
        # If we've gone through all items and haven't found a Header, return False
        return False
    except Exception as e:
        logger.error(f"Error occurred while checking for Details view: {e}")
        return False
    
def _maximize_clock_window(iui, clock_element):
    """
    Maximizes the clock window by clicking the "Maximize Clock" button.
    
    Args:
        clock_element: The root element to start searching from.
        iui: The UIAutomation interface instance.
    """
    maximize_clock_element = find_element_by_name(clock_element, iui, "Maximize Clock")
    if maximize_clock_element:
        print("Maximizing the clock window")
        try:
            UIA_InvokePatternId = 10000
            invoke_pattern = maximize_clock_element.GetCurrentPattern(UIA_InvokePatternId).QueryInterface(comtypes.gen.UIAutomationClient.IUIAutomationInvokePattern)
            invoke_pattern.Invoke()
            time.sleep(1)
        except:
            print("Error occurred while maximizing the clock window.")
    else:
        print("Maximize Clock button not found")

def clock_check_if_timer_started(hours: str, minutes: str, seconds: str) -> bool:

    formatted_timer_text = None
    try:
        hours_text = "hour" if int(hours) == 1 else "hours"
        minutes_text = "minute" if int(minutes) == 1 else "minutes"
        seconds_text = "second" if int(seconds) == 1 else "seconds"
        formatted_timer_text = f"{hours} {hours_text} {minutes} {minutes_text} {seconds} {seconds_text}"
        print("Attempting to look for timer with text: {}".format(formatted_timer_text))
    except:
        print("Error occurred while formatting timer text.")
        return False

    uia = comtypes.client.GetModule('UIAutomationCore.dll')
    iui = comtypes.client.CreateObject(uia.CUIAutomation)

    # Clock should already be foreground window before this function is called
    clock_hwnd = windll.user32.GetForegroundWindow()
    clock_element = iui.ElementFromHandle(clock_hwnd)

    _maximize_clock_window(iui, clock_element)
        
    try:
        re_string = f".*{formatted_timer_text}.*"
        timer_element = find_element_by_name(clock_element, iui, name_re=re_string)
        if timer_element:
            print("Found timer for interval: {}".format(formatted_timer_text))
            pause_element = find_element_by_name(timer_element, iui, "Timer running, Pause")
            if pause_element:
                return True
            else:
                print("Pause button not found for the timer. Timer is not started")
                return False
                   
        print("No timers found for the given interval[{} hours, {} minutes, {} seconds]".format(hours, minutes, seconds))
        return False
    except:
        print("Error occurred while looking for a timer.")
        return False
    
def clock_check_if_world_clock_exists(city: str, country: str) -> bool:
    uia = comtypes.client.GetModule('UIAutomationCore.dll')
    iui = comtypes.client.CreateObject(uia.CUIAutomation)

    # Clock should already be foreground window before this function is called
    clock_hwnd = windll.user32.GetForegroundWindow()
    clock_element = iui.ElementFromHandle(clock_hwnd)

    # Clock app should be on world clock tab already.

    # If the clock window is small, we might not be able to evaluate if the world clock exists.
    # Maximizing clock window to ensure the elements are visible.
    _maximize_clock_window(iui, clock_element)
        
    re_string = f".*{city}, {country}.*"
    try:
        munich_germany_element = find_element_by_name(clock_element, iui, name_re=re_string)
        if munich_germany_element:
            print(f"Found world clock for {city}, {country}")
            return True

        print(f"World clock for {city}, {country} not found")
        return False
    except:
        print("Error occurred while looking for world clock.")
        return False
