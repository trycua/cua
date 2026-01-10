"""Render a 3x3 grid of different OS themes with Slack and optional Spotify/WhatsApp windows."""

import io
import random
from pathlib import Path

from cua_bench.utils import render_windows
from PIL import Image


def generate_pastel_gradient(width, height):
    """Generate a 2D pastel color gradient."""
    # Random pastel colors (high lightness, low saturation)
    r1, g1, b1 = random.randint(180, 255), random.randint(180, 255), random.randint(180, 255)
    r2, g2, b2 = random.randint(180, 255), random.randint(180, 255), random.randint(180, 255)

    return f"linear-gradient(135deg, rgb({r1},{g1},{b1}) 0%, rgb({r2},{g2},{b2}) 100%)"


def load_html_file(filename):
    """Load HTML content from the gui directory."""
    html_path = Path(__file__).parent / "gui" / filename
    with open(html_path, "r", encoding="utf-8") as f:
        return f.read()


def main():
    """Generate a 3x3 grid of screenshots with different OS types."""

    # Define available OS types (desktop only, no mobile)
    os_types = ["win11", "win10", "win7", "winxp", "win98", "macos", "win11_light"]

    # Shuffle and take up to 9 OS types for 3x3 grid (or repeat if needed)
    random.shuffle(os_types)
    selected_os = []
    while len(selected_os) < 9:
        selected_os.extend(os_types)
    selected_os = selected_os[:9]

    # Dimensions for each screenshot (aspect ratio 209/148)
    screenshot_width = 1045
    screenshot_height = 740

    # Create a list to hold images
    images = []

    print(f"Rendering 3x3 grid with OS types: {selected_os}")

    # Load HTML files
    slack_html = load_html_file("slack.html")
    spotify_html = load_html_file("spotify.html")
    whatsapp_html = load_html_file("whatsapp.html")

    for i, os_type in enumerate(selected_os):
        print(f"Rendering {i+1}/9: {os_type}...")

        # Setup configuration for this OS with pastel gradient background
        setup_config = {
            "os_type": os_type,
            "width": screenshot_width,
            "height": screenshot_height,
            "background": generate_pastel_gradient(screenshot_width, screenshot_height),
        }

        # Define windows list - always start with Slack as the main window
        windows = []

        # Main Slack window (large, centered)
        slack_width = random.randint(600, 700)
        slack_height = random.randint(450, 550)
        slack_x = random.randint(20, screenshot_width - slack_width - 20)
        slack_y = random.randint(20, screenshot_height - slack_height - 20)

        windows.append(
            {
                "html": slack_html,
                "title": "Slack",
                "x": slack_x,
                "y": slack_y,
                "width": slack_width,
                "height": slack_height,
            }
        )

        # 10% chance to add Spotify window (small, random position)
        if random.random() < 0.1:
            spotify_width = random.randint(300, 400)
            spotify_height = random.randint(250, 350)
            # Try to position it so it doesn't completely overlap with Slack
            spotify_x = random.randint(0, screenshot_width - spotify_width)
            spotify_y = random.randint(0, screenshot_height - spotify_height)

            windows.append(
                {
                    "html": spotify_html,
                    "title": "Spotify",
                    "x": spotify_x,
                    "y": spotify_y,
                    "width": spotify_width,
                    "height": spotify_height,
                }
            )
            print("  + Added Spotify window")

        # 10% chance to add WhatsApp window (small, random position)
        if random.random() < 0.1:
            whatsapp_width = random.randint(300, 400)
            whatsapp_height = random.randint(250, 350)
            # Try to position it so it doesn't completely overlap with other windows
            whatsapp_x = random.randint(0, screenshot_width - whatsapp_width)
            whatsapp_y = random.randint(0, screenshot_height - whatsapp_height)

            windows.append(
                {
                    "html": whatsapp_html,
                    "title": "WhatsApp",
                    "x": whatsapp_x,
                    "y": whatsapp_y,
                    "width": whatsapp_width,
                    "height": whatsapp_height,
                }
            )
            print("  + Added WhatsApp window")

        # Render the screenshot
        screenshot_bytes = render_windows(
            provider="webtop",
            setup_config=setup_config,
            windows=windows,
            screenshot_delay=1.0,  # Give time for content to load
        )

        # Convert bytes to PIL Image
        img = Image.open(io.BytesIO(screenshot_bytes))
        images.append(img)

    print("Creating 3x3 grid with 6px gaps...")

    # Create 3x3 grid with 6px gaps
    gap = 6
    grid_width = screenshot_width * 3 + gap * 2
    grid_height = screenshot_height * 3 + gap * 2
    grid_image = Image.new("RGB", (grid_width, grid_height), color=(240, 240, 240))

    # Paste images into grid with gaps
    for i, img in enumerate(images):
        row = i // 3
        col = i % 3
        x = col * (screenshot_width + gap)
        y = row * (screenshot_height + gap)
        grid_image.paste(img, (x, y))

    # Save the grid
    output_path = Path(__file__).parent / "slack_grid_3x3.png"
    grid_image.save(output_path)

    print(f"Grid saved to: {output_path}")
    print(f"Grid dimensions: {grid_width}x{grid_height}")


if __name__ == "__main__":
    main()
