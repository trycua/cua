import logging
import os
import platform
from typing import Any, Dict, List, Tuple

import numpy as np
import supervision as sv
from PIL import Image, ImageDraw, ImageFont

logger = logging.getLogger(__name__)


class BoxAnnotator:
    """Class for drawing bounding boxes and labels on images."""

    def __init__(self):
        """Initialize the box annotator with a color palette."""
        # WCAG 2.1 compliant color palette optimized for accessibility
        self.colors = [
            "#2E7D32",  # Green
            "#C62828",  # Red
            "#1565C0",  # Blue
            "#6A1B9A",  # Purple
            "#EF6C00",  # Orange
            "#283593",  # Indigo
            "#4527A0",  # Deep Purple
            "#00695C",  # Teal
            "#D84315",  # Deep Orange
            "#1B5E20",  # Dark Green
            "#B71C1C",  # Dark Red
            "#0D47A1",  # Dark Blue
            "#4A148C",  # Dark Purple
            "#E65100",  # Dark Orange
            "#1A237E",  # Dark Indigo
            "#311B92",  # Darker Purple
            "#004D40",  # Dark Teal
            "#BF360C",  # Darker Orange
            "#33691E",  # Darker Green
            "#880E4F",  # Pink
        ]
        self.color_index = 0
        self.default_font = None
        self._initialize_font()

    def _initialize_font(self) -> None:
        """Initialize the default font."""
        # Try to load a system font first
        system = platform.system()
        font_paths = []

        if system == "Darwin":  # macOS
            font_paths = [
                "/System/Library/Fonts/Helvetica.ttc",
                "/System/Library/Fonts/Arial.ttf",
                "/Library/Fonts/Arial.ttf",
            ]
        elif system == "Linux":
            font_paths = [
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            ]
        else:  # Windows
            font_paths = ["C:\\Windows\\Fonts\\arial.ttf"]

        # Try each font path
        for font_path in font_paths:
            if os.path.exists(font_path):
                try:
                    # Test the font with a small size
                    test_font = ImageFont.truetype(font_path, 12)
                    # Test if the font can render text
                    test_font.getbbox("1")
                    self.default_font = font_path
                    return
                except Exception:
                    continue

    def _get_next_color(self) -> str:
        """Get the next color from the palette."""
        color = self.colors[self.color_index]
        self.color_index = (self.color_index + 1) % len(self.colors)
        return color

    def _hex_to_rgb(self, hex_color: str) -> Tuple[int, int, int]:
        """Convert hex color to RGB tuple."""
        hex_color = hex_color.lstrip("#")
        # Create explicit tuple of 3 integers to match the return type
        r = int(hex_color[0:2], 16)
        g = int(hex_color[2:4], 16)
        b = int(hex_color[4:6], 16)
        return (r, g, b)

    def draw_boxes(
        self, image: Image.Image, detections: List[Dict[str, Any]], draw_config: Dict[str, Any]
    ) -> Image.Image:
        """Draw bounding boxes and labels on the image."""
        draw = ImageDraw.Draw(image)

        # Create smaller font while keeping contrast
        try:
            if self.default_font:
                font = ImageFont.truetype(self.default_font, size=12)  # Reduced from 16 to 12
            else:
                # If no TrueType font available, use default
                font = ImageFont.load_default()
        except Exception:
            font = ImageFont.load_default()

        padding = 2  # Reduced padding for smaller overall box
        spacing = 1  # Reduced spacing between elements

        # Keep track of used label areas to check for collisions
        used_areas = []

        # Store label information for third pass
        labels_to_draw = []

        # First pass: Initialize used_areas with all bounding boxes
        for detection in detections:
            box = detection["bbox"]
            x1, y1, x2, y2 = [
                int(coord * dim) for coord, dim in zip(box, [image.width, image.height] * 2)
            ]
            used_areas.append((x1, y1, x2, y2))

        # Second pass: Draw all bounding boxes
        for idx, detection in enumerate(detections, 1):
            # Get box coordinates
            box = detection["bbox"]
            x1, y1, x2, y2 = [
                int(coord * dim) for coord, dim in zip(box, [image.width, image.height] * 2)
            ]

            # Get color for this detection
            color = self._get_next_color()
            rgb_color = self._hex_to_rgb(color)

            # Draw bounding box with original width
            draw.rectangle(((x1, y1), (x2, y2)), outline=rgb_color, width=2)

            # Use detection number as label
            label = str(idx)

            # Get text dimensions using getbbox
            bbox = font.getbbox(label)
            text_width = bbox[2] - bbox[0]
            text_height = bbox[3] - bbox[1]

            # Create box dimensions with padding
            box_width = text_width + (padding * 2)  # Removed multiplier for tighter box
            box_height = text_height + (padding * 2)  # Removed multiplier for tighter box

            def is_inside_bbox(x, y):
                """Check if a label box would be inside the bounding box."""
                return x >= x1 and x + box_width <= x2 and y >= y1 and y + box_height <= y2

            # Try different positions until we find one without collision
            positions = [
                # Top center (above bbox)
                lambda: (x1 + ((x2 - x1) - box_width) // 2, y1 - box_height - spacing),
                # Bottom center (below bbox)
                lambda: (x1 + ((x2 - x1) - box_width) // 2, y2 + spacing),
                # Right center (right of bbox)
                lambda: (x2 + spacing, y1 + ((y2 - y1) - box_height) // 2),
                # Left center (left of bbox)
                lambda: (x1 - box_width - spacing, y1 + ((y2 - y1) - box_height) // 2),
                # Top right (outside corner)
                lambda: (x2 + spacing, y1 - box_height - spacing),
                # Top left (outside corner)
                lambda: (x1 - box_width - spacing, y1 - box_height - spacing),
                # Bottom right (outside corner)
                lambda: (x2 + spacing, y2 + spacing),
                # Bottom left (outside corner)
                lambda: (x1 - box_width - spacing, y2 + spacing),
            ]

            def check_occlusion(x, y):
                """Check if a label box occludes any existing ones or is inside bbox."""
                # First check if it's inside the bounding box
                if is_inside_bbox(x, y):
                    return True

                # Then check collision with other labels
                new_box = (x, y, x + box_width, y + box_height)
                label_width = new_box[2] - new_box[0]
                label_height = new_box[3] - new_box[1]

                for used_box in used_areas:
                    if not (
                        new_box[2] < used_box[0]  # new box is left of used box
                        or new_box[0] > used_box[2]  # new box is right of used box
                        or new_box[3] < used_box[1]  # new box is above used box
                        or new_box[1] > used_box[3]  # new box is below used box
                    ):
                        # Calculate dimensions of the used box
                        used_box_width = used_box[2] - used_box[0]
                        used_box_height = used_box[3] - used_box[1]

                        # Only consider as collision if used box is NOT more than 5x bigger in both dimensions
                        if not (
                            used_box_width > 5 * label_width and used_box_height > 5 * label_height
                        ):
                            return True
                return False

            # Try each position until we find one without collision
            label_x = None
            label_y = None

            for get_pos in positions:
                x, y = get_pos()
                # Ensure position is within image bounds
                if x < 0 or y < 0 or x + box_width > image.width or y + box_height > image.height:
                    continue
                if not check_occlusion(x, y):
                    label_x = x
                    label_y = y
                    break

            # If all positions collide or are out of bounds, find the best possible position
            if label_x is None:
                # Try to place it in the nearest valid position outside the bbox
                best_pos = positions[0]()  # Default to top center
                label_x = max(0, min(image.width - box_width, best_pos[0]))
                label_y = max(0, min(image.height - box_height, best_pos[1]))

                # Ensure it's not inside the bounding box
                if is_inside_bbox(label_x, label_y):
                    # Force it above the bounding box
                    label_y = max(0, y1 - box_height - spacing)

            # Add this label area to used areas
            if (
                label_x is not None
                and label_y is not None
                and box_width is not None
                and box_height is not None
            ):
                used_areas.append((label_x, label_y, label_x + box_width, label_y + box_height))

            # Store label information for second pass
            labels_to_draw.append(
                {
                    "label": label,
                    "x": label_x,
                    "y": label_y,
                    "width": box_width,
                    "height": box_height,
                    "text_width": text_width,
                    "text_height": text_height,
                    "color": rgb_color,
                }
            )

        # Third pass: Draw all labels on top
        for label_info in labels_to_draw:
            # Draw background box with white outline
            draw.rectangle(
                (
                    (label_info["x"] - 1, label_info["y"] - 1),
                    (
                        label_info["x"] + label_info["width"] + 1,
                        label_info["y"] + label_info["height"] + 1,
                    ),
                ),
                outline="white",
                width=2,
            )
            draw.rectangle(
                (
                    (label_info["x"], label_info["y"]),
                    (label_info["x"] + label_info["width"], label_info["y"] + label_info["height"]),
                ),
                fill=label_info["color"],
            )

            # Center text in box
            text_x = label_info["x"] + (label_info["width"] - label_info["text_width"]) // 2
            text_y = label_info["y"] + (label_info["height"] - label_info["text_height"]) // 2

            # Draw text with black outline for better visibility
            outline_width = 1
            for dx in [-outline_width, outline_width]:
                for dy in [-outline_width, outline_width]:
                    draw.text(
                        (text_x + dx, text_y + dy), label_info["label"], fill="black", font=font
                    )

            # Draw the main white text
            draw.text((text_x, text_y), label_info["label"], fill=(255, 255, 255), font=font)

        logger.info("Finished drawing all boxes")
        return image
