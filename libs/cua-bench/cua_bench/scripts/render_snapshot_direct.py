"""Render a 3x3 grid of random OS's with a simple window."""

import io
import random
from pathlib import Path

from cua_bench.utils import render_windows
from PIL import Image


def main():
    """Generate a 3x3 grid of screenshots with different OS types."""

    # Define available OS types
    os_types = [
        "win11",
        "win10",
        "win7",
        "winxp",
        "win98",
        "macos",
        "android",
        "ios",
        "win11_light",
    ]

    # Shuffle and take 9 OS types for 3x3 grid
    random.shuffle(os_types)
    selected_os = os_types[:9]

    # Dimensions for each screenshot
    screenshot_width = 400
    screenshot_height = 300

    # Create a list to hold images
    images = []

    print(f"Rendering 3x3 grid with OS types: {selected_os}")

    for i, os_type in enumerate(selected_os):
        print(f"Rendering {i+1}/9: {os_type}...")

        # Setup configuration for this OS
        setup_config = {
            "os_type": os_type,
            "width": screenshot_width,
            "height": screenshot_height,
            "background": f"#{''.join(random.choices('0123456789ABCDEF', k=6))}",
        }

        # Define a simple window
        windows = [
            {
                "html": f"""
                    <div style="display: flex; align-items: center; justify-content: center;
                                height: 100%; font-family: Arial, sans-serif;
                                background: linear-gradient(135deg, #{random.randint(0, 255):02x}{random.randint(0, 255):02x}{random.randint(0, 255):02x}, #{random.randint(0, 255):02x}{random.randint(0, 255):02x}{random.randint(0, 255):02x});">
                        <div style="text-align: center; color: white; text-shadow: 2px 2px 4px rgba(0,0,0,0.5);">
                            <h1 style="margin: 0; font-size: 48px;">{os_type.upper()}</h1>
                            <p style="margin: 10px 0 0 0; font-size: 18px;">Sample Window {i+1}</p>
                        </div>
                    </div>
                """,
                "title": f"{os_type.upper()} Window",
                "x": random.randint(0, 32),
                "y": random.randint(0, 32),
                "width": random.randint(300, 350),
                "height": random.randint(200, 250),
            }
        ]

        # Render the screenshot
        screenshot_bytes = render_windows(
            provider="webtop",
            setup_config=setup_config,
            windows=windows,
            screenshot_delay=0.5,
        )

        # Convert bytes to PIL Image
        img = Image.open(io.BytesIO(screenshot_bytes))
        images.append(img)

    print("Creating 3x3 grid...")

    # Create 3x3 grid
    grid_width = screenshot_width * 3
    grid_height = screenshot_height * 3
    grid_image = Image.new("RGB", (grid_width, grid_height))

    # Paste images into grid
    for i, img in enumerate(images):
        row = i // 3
        col = i % 3
        x = col * screenshot_width
        y = row * screenshot_height
        grid_image.paste(img, (x, y))

    # Save the grid
    output_path = Path(__file__).parent / "os_grid_3x3.png"
    grid_image.save(output_path)

    print(f"Grid saved to: {output_path}")
    print(f"Grid dimensions: {grid_width}x{grid_height}")


if __name__ == "__main__":
    main()
