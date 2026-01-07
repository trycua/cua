import string
from PIL import Image
import numpy as np
import os
import logging
import platform
from typing import Dict, Any
from .file import get_vm_file
from io import BytesIO

logger = logging.getLogger("desktopenv.metric.microsoftpaint")

def find_red_circle(image) -> bool:
     if not image:
          return 0
     image = Image.open(BytesIO(image))
     image = image.convert('RGB')
     image_np = np.array(image)

     # Define the red color range for circumference
     lower_red = np.array([150, 0, 0])
     upper_red = np.array([255, 100, 100])

     # Define the white color range for interior
     lower_white = np.array([200, 200, 200])
     upper_white = np.array([255, 255, 255])

     # Create masks for red and white colors
     red_mask = ((image_np >= lower_red) & (image_np <= upper_red)).all(axis=-1)
     white_mask = ((image_np >= lower_white) & (image_np <= upper_white)).all(axis=-1)

     # Find the coordinates of the red pixels
     red_pixels = np.where(red_mask)

     if red_pixels[0].size == 0:
          return False

     # Calculate the centroid of the red pixels
     centroid_x = np.mean(red_pixels[1])
     centroid_y = np.mean(red_pixels[0])

     # Calculate the radius as the average distance from the centroid to the red pixels
     distances = np.sqrt((red_pixels[1] - centroid_x) ** 2 + (red_pixels[0] - centroid_y) ** 2)
     radius = int(np.mean(distances))

     # Define the bounding box of the detected circle
     top_left = (int(centroid_x - radius), int(centroid_y - radius))
     bottom_right = (int(centroid_x + radius), int(centroid_y + radius))
     
     # Extract the region inside the detected circle
     circle_interior = white_mask[top_left[1]:bottom_right[1], top_left[0]:bottom_right[0]]

     # Check if the interior is predominantly white
     white_pixel_count = np.sum(circle_interior)
     total_pixel_count = circle_interior.size
     if total_pixel_count == 0:
          return False
     white_ratio = white_pixel_count / total_pixel_count

     # Define a threshold for considering it a valid white interior
     white_threshold = 0.8  # 80% of the interior should be white
     if white_ratio > white_threshold:
          logger.info("Result: Found a red circle in the screenshot")
          return True
     else:
          logger.info("Result: No red circle found in the screenshot")
          return False

def get_is_red_circle_present_on_canvas(env, config: Dict[str, str]) -> bool:
     if not config["filepath"]:
          return False
     screenshot_path = config["filepath"]
     return find_red_circle(env.controller.get_file(screenshot_path))

def get_image_dimension_matches_input(env, config: Dict[str,str]) -> bool:
     if not config["filepath"]:
          return False
     if not config["width"]:
          return False
     if not config["height"]:
          return False
     
     filepath = config["filepath"]
     expectedWidth = float(config["width"])
     expectedHeight = float(config["height"])
     
     file_localhost_path = get_vm_file(env, {"path": filepath, "dest": os.path.split(filepath)[-1]})
     with Image.open(file_localhost_path) as img:
          width, height = img.size
         
     logger.info("ImageWidth:" + str(img.width) + ", ImageHeight:" + str(img.height))
     if((img.width != expectedWidth) or (img.height != expectedHeight)):
          return False
     
     return True