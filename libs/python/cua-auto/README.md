# cua-auto

`cua-auto` is a lightweight, cross-platform automation library providing a synchronous, pyautogui-style API for mouse, keyboard, screen, window, clipboard, and shell operations. It runs on Windows, macOS, and Linux using [pynput](https://github.com/moses-palmer/pynput) for input control and [pywinctl](https://github.com/Kalmat/PyWinCtl) for window management, with no display server or GUI framework required.

```python
import cua_auto.mouse as mouse
import cua_auto.keyboard as keyboard
import cua_auto.screen as screen
import cua_auto.window as window
import cua_auto.clipboard as clipboard
import cua_auto.shell as shell

# Mouse
mouse.click(100, 200)               # left click
mouse.right_click(100, 200)
mouse.double_click(100, 200)
mouse.move_to(500, 300)
mouse.mouse_down(100, 200)
mouse.mouse_up(100, 200)
mouse.drag(100, 200, 400, 500)      # start → end
mouse.scroll_up(3)
mouse.scroll_down(3)
x, y = mouse.position()

# Keyboard
keyboard.press_key("enter")
keyboard.type_text("hello world")
keyboard.hotkey(["ctrl", "c"])
keyboard.key_down("shift")
keyboard.key_up("shift")

# Screen
img = screen.screenshot()           # returns PIL.Image
png = screen.screenshot_bytes()     # raw PNG bytes
b64 = screen.screenshot_b64()      # base64 string
w, h = screen.screen_size()
x, y = screen.cursor_position()

# Window
title = window.get_active_window_title()
handle = window.get_active_window_handle()
handles = window.get_windows_with_title("Chrome")
name = window.get_window_name(handle)
x, y = window.get_window_position(handle)
w, h = window.get_window_size(handle)
window.activate_window(handle)
window.minimize_window(handle)
window.maximize_window(handle)
window.close_window(handle)
window.set_window_size(handle, 1280, 800)
window.set_window_position(handle, 0, 0)
window.open("https://example.com")  # or file path
pid = window.launch("notepad.exe")

# Clipboard
text = clipboard.get()
clipboard.set("hello")

# Shell
result = shell.run("echo hi")       # CommandResult
print(result.stdout, result.returncode)
```
