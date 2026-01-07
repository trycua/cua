import pyautogui  
import time
import pyperclip
import screen_utils
import os
from PIL import Image
import win32gui
import win32con
from io import BytesIO
import win32clipboard
from PIL import Image
import ctypes  
import pygetwindow as gw  
from pygetwindow import PyGetWindowException  
import re  
# from matplotlib import pyplot as plt
import difflib 
import logging
import subprocess
import winreg


### mouse fix:
# the cursor doesn't show up in screenshots otherwise
import ctypes
from ctypes import Structure, c_long, c_ulong, sizeof, POINTER
class MOUSEINPUT(Structure):
   _fields_ = [("dx", c_long), ("dy", c_long), ("mouseData", c_ulong), 
               ("dwFlags", c_ulong), ("time", c_ulong), ("dwExtraInfo", POINTER(c_ulong))]
class INPUT_UNION(ctypes.Union):
   _fields_ = [("mi", MOUSEINPUT)]
class INPUT(Structure):
   _fields_ = [("type", c_ulong), ("union", INPUT_UNION)]
user32 = ctypes.WinDLL('user32')
extra = c_ulong(0)
ii_ = INPUT_UNION()
ii_.mi = MOUSEINPUT(100, 0, 0, 0x0001, 0, ctypes.pointer(extra))
x = INPUT(0, ii_)
user32.ShowCursor(True)
user32.SendInput(1, ctypes.pointer(x), sizeof(INPUT))


sleep_after_execution = 0.25
class Mouse:  
    def __init__(self, rects, window_rect, scale=(1.0, 1.0), logger=None):  
        self.rects = rects
        self.window_rect = window_rect
        self.current_id = None  
        self.scale = scale
        if logger is None:
            self.logger = logging.getLogger(__name__)
        else:
            self.logger = logger
        self.logger.info(f"Mouse initialized with scale = {scale}")
  
    def move_id(self, id):  
        rect = self.rects[id]  
        center_x = (rect[0] + rect[2]) // 2  
        center_y = (rect[1] + rect[3]) // 2  
        # scale
        center_x = center_x * self.scale[0]
        center_y = center_y * self.scale[1]
        screen_x, screen_y = screen_utils.image_to_screen_coordinates(center_x, center_y, self.window_rect)
        
        pyautogui.moveTo(screen_x, screen_y)
        self.current_id = id  
        time.sleep(sleep_after_execution)
        
    
    def move_abs(self, x, y):  
        # x and y are normalized to the image size. Need to convert to image pixel coordinates and then to screen coordinates
        x_img = int(x * (self.window_rect[2]-self.window_rect[0]))
        y_img = int(y * (self.window_rect[3]-self.window_rect[1])) 
        # scale
        # x_img = x_img * self.scale[0]
        # y_img = y_img * self.scale[1]
        screen_x, screen_y = screen_utils.image_to_screen_coordinates(x_img, y_img, self.window_rect)
        
        pyautogui.moveTo(screen_x, screen_y)
        time.sleep(sleep_after_execution)

    def single_click(self):  
        pyautogui.click()
        time.sleep(sleep_after_execution)
    
    def double_click(self):  
        pyautogui.doubleClick()
        time.sleep(sleep_after_execution)

    def right_click(self):  
        # if self.current_id is not None:  
        pyautogui.rightClick()
        time.sleep(sleep_after_execution)
    
    def scroll(self, dir="down"):  
        if dir == "up":  
            pyautogui.scroll(400)  
        elif dir == "down":  
            pyautogui.scroll(-400)  
        else:  
            raise ValueError("direction must be 'up' or 'down'")
        time.sleep(sleep_after_execution)

    def drag(self, screen_x, screen_y):
        pyautogui.dragTo(screen_x, screen_y, button='left')   
        time.sleep(sleep_after_execution)
         

        
class OS:
    def maximize_window(self, window=None):    
        if not window:    
            window = win32gui.GetForegroundWindow()  
  
        # Set the window position to the main monitor (0,0) before maximizing  
        win32gui.SetWindowPos(window, 0, 0, 0, 0, 0, win32con.SWP_NOSIZE)  
  
        # Maximize the window  
        win32gui.ShowWindow(window, win32con.SW_MAXIMIZE)   
        time.sleep(sleep_after_execution)
        
    
    def is_installed(self, program):
        try:
            # Check for 32 bit windows
            registry_key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, f"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{program}.exe")
            executable, _ = winreg.QueryValueEx(registry_key, "")
            winreg.CloseKey(registry_key)
            if executable:
                return True
        except WindowsError:
            pass
        try:
            # Check for 64 bit windows
            registry_key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, f"SOFTWARE\\Wow6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{program}.exe")
            executable, _ = winreg.QueryValueEx(registry_key, "")
            winreg.CloseKey(registry_key)
            if executable:
                return True
        except WindowsError:
            pass
        # If not found in registry, try to run the program
        try:
            subprocess.Popen([program], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).communicate()
            return True
        except OSError:
            return False  
        
    def open_program(self, program):
        # check if program exists in windows
        if not self.is_installed(program):
            print(f"error: Program {program} is not installed")
            return
        else:
            os.system(f'start {program}')
            time.sleep(2.0)
            pyautogui.press('esc')

            # Maximize the window on the main monitor  
            self.maximize_window()  
            
            # make it full screen
            # pyautogui.hotkey('win', 'up')
            # time.sleep(1.0)
            # window = win32gui.GetForegroundWindow()
            # win32gui.ShowWindow(window, win32con.SW_MAXIMIZE)  
            
            # press escape key
            pyautogui.press('esc')
            time.sleep(1.0)
            pyautogui.press('esc')
            time.sleep(1.0)
            pyautogui.press('esc')
  
class Keyboard:  
    def write(self, text):  
        pyautogui.write(text)  
        time.sleep(sleep_after_execution)
  
    def press(self, key):  
        if len(key) > 1 and ("+" in key or "-" in key): # for ctrl+c, ctrl+v, etc
            keys = [k.strip() for k in key.split("+" if "+" in key else "-")]
                
            pyautogui.hotkey(*keys)
        else:
            pyautogui.press(key)
        time.sleep(sleep_after_execution)

def send_to_clipboard(clip_type, data):
    win32clipboard.OpenClipboard()
    win32clipboard.EmptyClipboard()
    win32clipboard.SetClipboardData(clip_type, data)
    win32clipboard.CloseClipboard()

class Clipboard:
    def __init__(self, rects, screenshot, clipboard_content=None, swap_ctrl_alt=False):
        self.rects = rects
        self.screenshot = screenshot
        self.swap_ctrl_alt = swap_ctrl_alt
        self.clipboard_content = clipboard_content

    def view_clipboard(self):
        """
        Returns the current content of on the clipboard.
        """
        return pyperclip.paste()

    def copy_text(self, text=None):
        """
        Copies the given text to the clipboard.
        """
        print(text)
        if text is not None:
            pyperclip.copy(text)
        else:
            pyautogui.doubleClick(pyautogui.position())
            if self.swap_ctrl_alt:
                pyautogui.hotkey("alt", "c")
            else:
                pyautogui.hotkey("ctrl", "c")
        self.clipboard_content = text
            

    def copy_image(self, id, description=""):
        """
        Copies the image at the given screen coordinates to the clipboard.
        """
        rect = self.rects[id]
        #clip the img using the bbox rect
        image = self.screenshot.crop(rect)
        # Convert the image to a format that can be copied to the clipboard 
        output = BytesIO()
        image.convert("RGB").save(output, "BMP")
        data = output.getvalue()[14:]
        output.close()
        send_to_clipboard(win32clipboard.CF_DIB, data)
        self.clipboard_content = description


    def paste(self):
        """
        Pastes the current content of the clipboard.
        """
        if self.swap_ctrl_alt:
            pyautogui.hotkey('alt', 'v')
        else:
            pyautogui.hotkey('ctrl', 'v')
        time.sleep(sleep_after_execution)


class WindowManager:  
    def __init__(self):  
        self.applications = []  
  
    def get_app_name(self, hwnd):  
        length = ctypes.windll.user32.GetWindowTextLengthW(hwnd)  
        buf = ctypes.create_unicode_buffer(length + 1)  
        ctypes.windll.user32.GetWindowTextW(hwnd, buf, length + 1)  
        return buf.value  
  
    def is_window_visible(self, hwnd):  
        return win32gui.IsWindowVisible(hwnd)  
  
    def find_open_applications(self):  
        self.applications = []  
        for window in gw.getAllWindows():  
            if self.is_window_visible(window._hWnd) and window.title != "":  
                app_name = self.get_app_name(window._hWnd)  
                if app_name not in self.applications:  
                    self.applications.append(app_name)  
        return self.applications  
    
    def normalize_app_name(self, name):  
        return ' '.join(name.split())  
    
    def switch_to_application(self, app_name):  
        self.find_open_applications()
        alt_key = 0x12  # Virtual key code for the ALT key  
  
        app_name = self.normalize_app_name(app_name)  
  
        # Find the closest matching app name  
        closest_match = difflib.get_close_matches(app_name, [self.normalize_app_name(a) for a in self.applications], n=1)  
        if not closest_match:  
            print(f"No open window found for the application '{app_name}'")  
            return  
        else:  
            app_name = closest_match[0]  
  
        # The rest of the method remains the same  
        for window in gw.getAllWindows():  
            if self.is_window_visible(window._hWnd) and self.normalize_app_name(self.get_app_name(window._hWnd)) == app_name:  
                window.restore()  # Restore the window if it's minimized  
                window.maximize()  # Maximize the window  
  
                # Simulate ALT key press and release  
                ctypes.windll.user32.keybd_event(alt_key, 0, 0, 0)  
                ctypes.windll.user32.keybd_event(alt_key, 0, 2, 0)  
  
                # Bring the window to the front  
                win32gui.SetForegroundWindow(window._hWnd)  

                # press escape key once
                pyautogui.press('esc')
                break  
        time.sleep(sleep_after_execution)
    
    def switch_to_application1(self, app_name):  
        alt_key = 0x12  # Virtual key code for the ALT key  
        for window in gw.getAllWindows():  
            if self.is_window_visible(window._hWnd) and self.get_app_name(window._hWnd) == app_name:  
                window.restore()  # Restore the window if it's minimized  
                window.maximize()  # Maximize the window  
                  
                # Simulate ALT key press and release  
                ctypes.windll.user32.keybd_event(alt_key, 0, 0, 0)  
                ctypes.windll.user32.keybd_event(alt_key, 0, 2, 0)  
  
                # Bring the window to the front  
                win32gui.SetForegroundWindow(window._hWnd)  
                break  
  
    def switch_to_application2(self, app_name):  
        target_window = None  
        for window in gw.getAllWindows():  
            if self.is_window_visible(window._hWnd) and window.title != "":  
                current_app_name = self.get_app_name(window._hWnd)  
                if current_app_name == app_name:  
                    target_window = window  
                    break  
    
        if target_window is not None:  
            hwnd = target_window._hWnd  
            win32gui.ShowWindow(hwnd, win32con.SW_RESTORE)  
            win32gui.SetForegroundWindow(hwnd)  
            # pyautogui.moveTo(target_window.left + 50, target_window.top + 50)  
            # pyautogui.click()  

            # maximize the window just to make sure
            window = win32gui.GetForegroundWindow()  
            win32gui.SetWindowPos(window, 0, 0, 0, 0, 0, win32con.SWP_NOSIZE)  
            win32gui.ShowWindow(window, win32con.SW_MAXIMIZE)   
        else:  
            print(f"No open window found for the application '{app_name}'")  
  
    def __str__(self):  
        return '\n'.join(self.applications)  
    
class Computer:  
    def __init__(self, logger=None):
        self.mouse = None
        self.clipboard = None
        self.os = None
        self.window_manager = None
        self.keyboard = None
        self.search = None
        if logger is None:
            self.logger = logging.getLogger(__name__)
        else:
            self.logger = logger

    def update(self, rects, window_rect, screenshot, scale=(1.0, 1.0), clipboard_content=None, swap_ctrl_alt=False):
        self.mouse = Mouse(rects, window_rect, scale, logger=self.logger)  
        self.clipboard = Clipboard(rects, screenshot, clipboard_content, swap_ctrl_alt)
        self.os = OS()
        self.window_manager = WindowManager()
        self.keyboard = Keyboard()


def test_computer_with_clipboard(img_path):
    img = Image.open(img_path)
    rects = [[1388, 533, 2020, 696], [330, 600, 1060, 800]]
    window_rect = [0, 0, 2250, 1420] # x, y, width, height
    computer = Computer(rects, window_rect, swap_ctrl_alt=True) 
    computer.mouse.move(id=0)
    computer.clipboard.copy_image(id = 0, img = img)
    computer.mouse.move(id=1)
    computer.mouse.single_click()

    time.sleep(2.0)
    computer.clipboard.view_clipboard()
    
    time.sleep(2.0)
    computer.clipboard.paste()
    time.sleep(1.0)


def main():

    open_apps = WindowManager()  
    app_names = open_apps.find_open_applications()  
    for app in app_names:
        print(app)
    print(open_apps)  
    open_apps.switch_to_application("Spotify Premium")

    screenshot_img, window_title, window_rect = screen_utils.capture_screenshot()

    # fake rectangles
    rects = [[0, 0, 100, 100], [200, 20, 400, 60]]

    computer = Computer(rects, window_rect, screenshot_img, scale=(1.0, 1.0), swap_ctrl_alt=True) 

    computer.os.open_program("msedge")

    computer.mouse.move(id=1) 

    time.sleep(2)
    computer.mouse.move_abs(0.5, 0.5)


    time.sleep(2)
    computer.mouse.scroll(dir="down")
    time.sleep(2)
    computer.mouse.scroll(dir="up")
    # computer.mouse.right_click()

    computer.mouse.click()  
    computer.keyboard.write("amazon.com")  
    computer.keyboard.press("enter")



if __name__ == "__main__":
    main()
