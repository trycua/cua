# import subprocess  
# import sys
# import importlib  

# def import_module_and_attribute(module_name, attribute_name):  
#     module = importlib.import_module(module_name)  
  
#     if attribute_name:  
#         attribute_parts = attribute_name.split(".")  
#         attribute = module  
#         for part in attribute_parts:  
#             attribute = getattr(attribute, part)  
#     else:  
#         attribute = None  
  
#     return module, attribute  

# imports = {  
#     "win32gui": "pywin32",  
#     "pyautogui": "pyautogui",  
#     "PIL.ImageGrab": "pillow",  
#     "screeninfo.get_monitors": "screeninfo"  
# }  

# for key in imports:  
#     module_name, *attribute_name = key.split(".")  
#     attribute_name = ".".join(attribute_name)  
#     try:  
#         # Try to import the module and attribute  
#         module, attribute = import_module_and_attribute(module_name, attribute_name)  
#     except ImportError:  
#         # If it fails, pip install the corresponding package which is in the dictionary  
#         subprocess.check_call([sys.executable, "-m", "pip", "install", imports[key]])  
#         # Now try to import the module and attribute again  
#         module, attribute = import_module_and_attribute(module_name, attribute_name)  

import win32gui
import pyautogui
from PIL import ImageGrab
from screeninfo import get_monitors 

# def capture_screenshot():  
#     window = win32gui.GetForegroundWindow()
#     # window_title = win32gui.GetWindowText(window) # this is the same as the next line, but has the hex number before the title
#     window_title = pyautogui.getActiveWindowTitle()
#     rect = win32gui.GetWindowRect(window)
#     image = ImageGrab.grab(rect)
#     # Display the image
#     # image.show()
#     return image, window_title, rect

def capture_screenshot():    
    window = win32gui.GetForegroundWindow()
    try:
        window_title = pyautogui.getActiveWindowTitle()  
    except Exception as e:
        window_title = ''
        
    if window_title=='Program Manager' or window_title=='':  
         # find the monitor that is the primary
        monitors = get_monitors()
        for monitor in monitors:
            if monitor.is_primary:
                main_monitor = monitor
                break
        rect = (main_monitor.x, main_monitor.y, main_monitor.width, main_monitor.height)  
        window_title = "Desktop"  
    else:  
       rect = win32gui.GetWindowRect(window)  # x0, y0, x1, y1 from the screen in global coordinates
  
    image = ImageGrab.grab(rect)  
    # Display the image  
    # image.show()  
    return image, window_title, rect  

def image_to_screen_coordinates(x, y, window_rect):  
    screen_x = x + window_rect[0]  
    screen_y = y + window_rect[1]  
    return screen_x, screen_y  