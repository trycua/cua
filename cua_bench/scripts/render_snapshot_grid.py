"""Render a grid of OS types (y-axis) vs window clutter (x-axis)."""

import io
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

from cua_bench.utils import render_windows


def main():
    """Generate a grid showing OS types vs window clutter levels."""
    
    # Define OS types for y-axis
    os_types = ["win11", "win10", "win7", "winxp", "win98", "macos", "android", "ios"]
    
    # Define window counts for x-axis
    window_counts = [0, 1, 2]
    
    # Dimensions for each screenshot
    screenshot_width = 400
    screenshot_height = 300
    
    # Grid dimensions
    num_rows = len(os_types)
    num_cols = len(window_counts)
    
    # Create a list to hold images
    images = []
    
    print(f"Rendering {num_rows}x{num_cols} grid: OS types vs window clutter")
    print(f"OS types: {os_types}")
    print(f"Window counts: {window_counts}\n")
    
    # Generate all screenshots
    for row_idx, os_type in enumerate(os_types):
        for col_idx, window_count in enumerate(window_counts):
            cell_num = row_idx * num_cols + col_idx + 1
            print(f"Rendering cell {cell_num}/{num_rows * num_cols}: {os_type} with {window_count} window(s)...")
            
            # Setup configuration for this OS
            width = screenshot_width if os_type not in ("android", "ios") else 384
            height = screenshot_height if os_type not in ("android", "ios") else 640
            
            setup_config = {
                "os_type": os_type,
                "width": width,
                "height": height,
                "background": f"#{''.join(random.choices('456789ABCDEF', k=6))}",
            }
            
            # Generate windows based on window_count
            windows = []
            for i in range(window_count):
                # Randomize window position and size
                base_x = 50 + (i * 100) % 200
                base_y = 50 + (i * 80) % 150
                
                windows.append({
                    "html": f"""
                        <div style="display: flex; align-items: center; justify-content: center; 
                                    height: 100%; font-family: Arial, sans-serif; 
                                    background: linear-gradient(135deg, #{random.randint(100, 255):02x}{random.randint(100, 255):02x}{random.randint(100, 255):02x}, #{random.randint(100, 255):02x}{random.randint(100, 255):02x}{random.randint(100, 255):02x});">
                            <div style="text-align: center; color: white; text-shadow: 2px 2px 4px rgba(0,0,0,0.5);">
                                <h1 style="margin: 0; font-size: 36px;">Window {i+1}</h1>
                                <p style="margin: 10px 0 0 0; font-size: 16px;">{os_type}</p>
                            </div>
                        </div>
                    """,
                    "title": f"Window {i+1}",
                    "x": base_x,
                    "y": base_y,
                    "width": random.randint(250, 400),
                    "height": random.randint(200, 350),
                })
            
            # Render the screenshot
            screenshot_bytes = render_windows(
                provider="webtop",
                setup_config=setup_config,
                windows=windows,
                screenshot_delay=0.3,
            )
            
            # Convert bytes to PIL Image and resize to standard size if needed
            img = Image.open(io.BytesIO(screenshot_bytes))
            if img.size != (screenshot_width, screenshot_height):
                img = img.resize((screenshot_width, screenshot_height), Image.Resampling.LANCZOS)
            images.append(img)
    
    print("\nCreating grid with labels...")
    
    # Add space for labels
    label_width = 100
    label_height = 40
    
    # Create grid with labels
    grid_width = label_width + (screenshot_width * num_cols)
    grid_height = label_height + (screenshot_height * num_rows)
    grid_image = Image.new('RGB', (grid_width, grid_height), color='white')
    draw = ImageDraw.Draw(grid_image)
    
    # Try to use a better font, fall back to default
    try:
        font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 16)
        font_small = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 14)
    except:
        try:
            font = ImageFont.truetype("arial.ttf", 16)
            font_small = ImageFont.truetype("arial.ttf", 14)
        except:
            font = ImageFont.load_default()
            font_small = ImageFont.load_default()
    
    # Add column headers (window counts)
    for col_idx, window_count in enumerate(window_counts):
        x = label_width + (col_idx * screenshot_width) + (screenshot_width // 2)
        y = label_height // 2
        text = f"{window_count} win" if window_count != 1 else "1 win"
        # Center text
        bbox = draw.textbbox((0, 0), text, font=font)
        text_width = bbox[2] - bbox[0]
        draw.text((x - text_width // 2, y - 10), text, fill='black', font=font)
    
    # Add row headers (OS types) and paste images
    for row_idx, os_type in enumerate(os_types):
        # Draw OS type label
        x = label_width // 2
        y = label_height + (row_idx * screenshot_height) + (screenshot_height // 2)
        # Center text vertically
        bbox = draw.textbbox((0, 0), os_type, font=font_small)
        text_height = bbox[3] - bbox[1]
        text_width = bbox[2] - bbox[0]
        draw.text((x - text_width // 2, y - text_height // 2), os_type, fill='black', font=font_small)
        
        # Paste screenshots for this row
        for col_idx in range(num_cols):
            img_idx = row_idx * num_cols + col_idx
            img = images[img_idx]
            x_pos = label_width + (col_idx * screenshot_width)
            y_pos = label_height + (row_idx * screenshot_height)
            grid_image.paste(img, (x_pos, y_pos))
    
    # Draw grid lines
    for i in range(num_rows + 1):
        y = label_height + (i * screenshot_height)
        draw.line([(label_width, y), (grid_width, y)], fill='gray', width=1)
    
    for i in range(num_cols + 1):
        x = label_width + (i * screenshot_width)
        draw.line([(x, label_height), (x, grid_height)], fill='gray', width=1)
    
    # Save the grid
    output_path = Path(__file__).parent / "os_clutter_grid.png"
    grid_image.save(output_path)
    
    print(f"\nGrid saved to: {output_path}")
    print(f"Grid dimensions: {grid_width}x{grid_height}")
    print(f"Layout: {num_rows} OS types × {num_cols} window counts")


if __name__ == "__main__":
    main()
