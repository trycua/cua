"""Core visualization utilities for agents."""

import base64
import logging
from io import BytesIO
from typing import Dict, Tuple

from PIL import Image, ImageDraw

logger = logging.getLogger(__name__)


def visualize_click(x: int, y: int, img_base64: str) -> Image.Image:
    """Visualize a click action by drawing a circle on the screenshot.

    Args:
        x: X coordinate of the click
        y: Y coordinate of the click
        img_base64: Base64-encoded screenshot

    Returns:
        PIL Image with visualization
    """
    try:
        # Decode the base64 image
        image_data = base64.b64decode(img_base64)
        img = Image.open(BytesIO(image_data))

        # Create a copy to draw on
        draw_img = img.copy()
        draw = ImageDraw.Draw(draw_img)

        # Draw a circle at the click location
        radius = 15
        draw.ellipse([(x - radius, y - radius), (x + radius, y + radius)], outline="red", width=3)

        # Draw crosshairs
        line_length = 20
        draw.line([(x - line_length, y), (x + line_length, y)], fill="red", width=3)
        draw.line([(x, y - line_length), (x, y + line_length)], fill="red", width=3)

        return draw_img
    except Exception as e:
        logger.error(f"Error visualizing click: {str(e)}")
        # Return a blank image as fallback
        return Image.new("RGB", (800, 600), "white")


def visualize_scroll(direction: str, clicks: int, img_base64: str) -> Image.Image:
    """Visualize a scroll action by drawing arrows on the screenshot.

    Args:
        direction: Direction of scroll ('up' or 'down')
        clicks: Number of scroll clicks
        img_base64: Base64-encoded screenshot

    Returns:
        PIL Image with visualization
    """
    try:
        # Decode the base64 image
        image_data = base64.b64decode(img_base64)
        img = Image.open(BytesIO(image_data))

        # Create a copy to draw on
        draw_img = img.copy()
        draw = ImageDraw.Draw(draw_img)

        # Calculate parameters for visualization
        width, height = img.size
        center_x = width // 2

        # Draw arrows to indicate scrolling
        arrow_length = min(100, height // 4)
        arrow_width = 30
        num_arrows = min(clicks, 3)  # Don't draw too many arrows

        # Calculate starting position
        if direction == "down":
            start_y = height // 3
            arrow_dir = 1  # Down
        else:
            start_y = height * 2 // 3
            arrow_dir = -1  # Up

        # Draw the arrows
        for i in range(num_arrows):
            y_pos = start_y + (i * arrow_length * arrow_dir * 0.7)
            arrow_top = (center_x, y_pos)
            arrow_bottom = (center_x, y_pos + arrow_length * arrow_dir)

            # Draw the main line
            draw.line([arrow_top, arrow_bottom], fill="red", width=5)

            # Draw the arrowhead
            arrowhead_size = 20
            if direction == "down":
                draw.line(
                    [
                        (center_x - arrow_width // 2, arrow_bottom[1] - arrowhead_size),
                        arrow_bottom,
                        (center_x + arrow_width // 2, arrow_bottom[1] - arrowhead_size),
                    ],
                    fill="red",
                    width=5,
                )
            else:
                draw.line(
                    [
                        (center_x - arrow_width // 2, arrow_bottom[1] + arrowhead_size),
                        arrow_bottom,
                        (center_x + arrow_width // 2, arrow_bottom[1] + arrowhead_size),
                    ],
                    fill="red",
                    width=5,
                )

        return draw_img
    except Exception as e:
        logger.error(f"Error visualizing scroll: {str(e)}")
        # Return a blank image as fallback
        return Image.new("RGB", (800, 600), "white")


def calculate_element_center(bbox: Dict[str, float], width: int, height: int) -> Tuple[int, int]:
    """Calculate the center point of a UI element.

    Args:
        bbox: Bounding box dictionary with x1, y1, x2, y2 coordinates (0-1 normalized)
        width: Screen width in pixels
        height: Screen height in pixels

    Returns:
        (x, y) tuple with pixel coordinates
    """
    center_x = int((bbox["x1"] + bbox["x2"]) / 2 * width)
    center_y = int((bbox["y1"] + bbox["y2"]) / 2 * height)
    return center_x, center_y


class VisualizationHelper:
    """Helper class for visualizing agent actions."""

    def __init__(self, agent):
        """Initialize visualization helper.

        Args:
            agent: Reference to the agent that will use this helper
        """
        self.agent = agent

    def visualize_action(self, x: int, y: int, img_base64: str) -> None:
        """Visualize a click action by drawing on the screenshot."""
        if (
            not self.agent.save_trajectory
            or not hasattr(self.agent, "experiment_manager")
            or not self.agent.experiment_manager
        ):
            return

        try:
            # Use the visualization utility
            img = visualize_click(x, y, img_base64)

            # Save the visualization
            self.agent.experiment_manager.save_action_visualization(img, "click", f"x{x}_y{y}")
        except Exception as e:
            logger.error(f"Error visualizing action: {str(e)}")

    def visualize_scroll(self, direction: str, clicks: int, img_base64: str) -> None:
        """Visualize a scroll action by drawing arrows on the screenshot."""
        if (
            not self.agent.save_trajectory
            or not hasattr(self.agent, "experiment_manager")
            or not self.agent.experiment_manager
        ):
            return

        try:
            # Use the visualization utility
            img = visualize_scroll(direction, clicks, img_base64)

            # Save the visualization
            self.agent.experiment_manager.save_action_visualization(
                img, "scroll", f"{direction}_{clicks}"
            )
        except Exception as e:
            logger.error(f"Error visualizing scroll: {str(e)}")

    def save_action_visualization(
        self, img: Image.Image, action_name: str, details: str = ""
    ) -> str:
        """Save a visualization of an action."""
        if hasattr(self.agent, "experiment_manager") and self.agent.experiment_manager:
            return self.agent.experiment_manager.save_action_visualization(
                img, action_name, details
            )
        return ""
