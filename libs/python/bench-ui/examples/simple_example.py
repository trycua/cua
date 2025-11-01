from __future__ import annotations
import time
from bench_ui import launch_window, get_element_rect, execute_javascript
from pathlib import Path

HTML = """
<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>Bench UI Example</title>
    <style>
      body { font-family: system-ui, sans-serif; margin: 24px; }
      #target { width: 220px; height: 120px; background: #4f46e5; color: white; display: flex; align-items: center; justify-content: center; border-radius: 8px; }
    </style>
  </head>
  <body>
    <h1>Bench UI Example</h1>
    <div id="target">Hello from pywebview</div>
  </body>
</html>
"""

def main():
    # Launch a window with inline HTML content
    pid = launch_window(
        html=HTML,
        title="Bench UI Example",
        width=800,
        height=600,
    )
    print(f"Launched window with PID: {pid}")

    # Give the window a brief moment to render
    time.sleep(1.0)

    # Query the client rect of an element via CSS selector in SCREEN space
    rect = get_element_rect(pid, "#target", space="screen")
    print("Element rect (screen space):", rect)

    # Take a screenshot and overlay the bbox
    try:
        from PIL import ImageGrab, ImageDraw

        img = ImageGrab.grab()  # full screen
        draw = ImageDraw.Draw(img)
        x, y, w, h = rect["x"], rect["y"], rect["width"], rect["height"]
        box = (x, y, x + w, y + h)
        draw.rectangle(box, outline=(255, 0, 0), width=3)
        out_path = Path(__file__).parent / "output_overlay.png"
        img.save(out_path)
        print(f"Saved overlay screenshot to: {out_path}")
    except Exception as e:
        print(f"Failed to capture/annotate screenshot: {e}")

    # Execute arbitrary JavaScript
    text = execute_javascript(pid, "document.querySelector('#t')?.textContent")
    print("text:", text)


if __name__ == "__main__":
    main()
