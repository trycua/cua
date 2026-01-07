import os
from typing import List, Union
from skimage.metrics import structural_similarity as ssim
from PIL import Image, ImageChops, ImageStat


def compare_image_list(pred_img_path_list: Union[str, List[str]],
                       gold_img_path_list: Union[str, List[str]]) -> float:
    """ Compare two image lists, only if all images are the same, return 1.0, otherwise return 0.0
    """
    if type(pred_img_path_list) != list:
        pred_img_path_list = [pred_img_path_list]
        gold_img_path_list = [gold_img_path_list]
    for pred_img_path, gold_img_path in zip(pred_img_path_list, gold_img_path_list):
        if not pred_img_path or not gold_img_path:
            return 0.0
        pred_img = Image.open(pred_img_path)
        gold_img = Image.open(gold_img_path)
        diff = ImageChops.difference(pred_img, gold_img)
        if diff.getbbox():
            return 0.0
    return 1.0


def get_gimp_export_path():
    # Path to GIMP's configuration file. This example assumes GIMP version 2.10.
    # You need to adjust the path according to the GIMP version and user's file system.
    gimp_config_file = os.path.expanduser("~/.config/GIMP/2.10/gimprc")

    try:
        # Open and read the configuration file
        with open(gimp_config_file, 'r') as file:
            for line in file:
                # Search for the default export path setting
                if "default-export-path" in line:
                    # Extract the current path from the line (assuming it's enclosed in quotes)
                    current_path = line.split('"')[1]
                    # Compare the current path with the expected path
                    return current_path
    except FileNotFoundError:
        # Handle the case where the configuration file is not found
        print("GIMP configuration file not found")
        return False


def check_file_exists(directory, filename):
    file_path = os.path.join(directory, filename)
    return 1 if os.path.isfile(file_path) else 0


def increase_saturation(image1_path: str, image2_path: str) -> float:
    def calculate_saturation(image):
        # convert the image to HSV mode
        hsv_image = image.convert("HSV")

        saturation_channel = hsv_image.split()[1]

        # calculate the mean saturation level
        stat = ImageStat.Stat(saturation_channel)
        mean_saturation = stat.mean[0]

        return mean_saturation

    image1 = Image.open(image1_path)
    image2 = Image.open(image2_path)

    # calculate the saturation level of each image
    saturation1 = calculate_saturation(image1)
    saturation2 = calculate_saturation(image2)

    return 1 if saturation1 < saturation2 else 0


def decrease_brightness(image1_path: str, image2_path: str) -> float:
    def calculate_brightness(image):
        # Convert the image to grayscale mode
        grayscale_image = image.convert("L")

        # Get the image data
        pixels = list(grayscale_image.getdata())

        brightness = sum(pixels) / len(pixels)
        return brightness

    image1 = Image.open(image1_path)
    image2 = Image.open(image2_path)

    brightness1 = calculate_brightness(image1)
    brightness2 = calculate_brightness(image2)

    return 1 if brightness1 > brightness2 else 0


import cv2
import numpy as np


def find_yellow_triangle(image):
    # Convert the image to RGBA
    rgba = cv2.cvtColor(image, cv2.COLOR_BGR2RGBA)

    # define range of yellow color in HSV
    lower_yellow = np.array([0, 0, 0], dtype=np.uint8)
    upper_yellow = np.array([255, 255, 255], dtype=np.uint8)

    # expand the dimensions of lower and upper yellow to match the image dimensions
    lower_yellow = np.reshape(lower_yellow, (1, 1, 3))
    upper_yellow = np.reshape(upper_yellow, (1, 1, 3))
    # build a mask for the yellow color
    mask = cv2.inRange(rgba, lower_yellow, upper_yellow)

    # search for contours in the mask
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)

    # choose the largest contour
    max_contour = max(contours, key=cv2.contourArea)

    # calculate the center of the contour
    M = cv2.moments(max_contour)
    cx = int(M['m10'] / M['m00'])
    cy = int(M['m01'] / M['m00'])

    return cx, cy


def compare_triangle_positions(image1, image2):
    image1 = cv2.imread(image1, cv2.IMREAD_COLOR)
    image2 = cv2.imread(image2, cv2.IMREAD_COLOR)
    # find the center of the yellow triangle in each image
    cx1, cy1 = find_yellow_triangle(image1)
    cx2, cy2 = find_yellow_triangle(image2)

    # calculate the distance between the center of the triangle and the center of the image
    center_distance1 = np.sqrt(
        (cx1 - image1.shape[1] // 2) ** 2 + (cy1 - image1.shape[0] // 2) ** 2)
    center_distance2 = np.sqrt(
        (cx2 - image2.shape[1] // 2) ** 2 + (cy2 - image2.shape[0] // 2) ** 2)

    return 1 if center_distance1 > center_distance2 else 0


# Functions for the GIMP evaluator
def calculate_brightness(image):
    """Calculate the average brightness of an image"""
    grayscale = image.convert('L')
    stat = ImageStat.Stat(grayscale)
    return stat.mean[0]


def normalize_brightness(image, target_brightness):
    """Normalize the brightness of an image to a target brightness in [0, 1]"""
    current_brightness = calculate_brightness(image)
    factor = target_brightness / current_brightness

    # Apply a point transform to each pixel
    def point_transform(x):
        return min(255, max(0, int(x * factor)))

    return image.point(point_transform)


def measure_saturation(hsv_image):
    """Measure the average saturation of an image"""
    # Split into H, S, V channels
    _, s, _ = hsv_image.split()
    # Convert the saturation channel to a numpy array
    s_array = np.array(s)
    # Calculate the average saturation
    avg_saturation = np.mean(s_array)
    return avg_saturation


def calculate_contrast(image):
    """Calculate the contrast of an image as the standard deviation of the pixel
    values."""
    pixels = np.asarray(image, dtype=np.float32)
    return np.std(pixels)


def calculate_image_sharpness(image_path):
    # Load the image in grayscale
    image = cv2.imread(image_path, cv2.IMREAD_GRAYSCALE)
    # Apply the Laplacian operator
    laplacian = cv2.Laplacian(image, cv2.CV_64F)
    # Calculate the variance
    variance = np.var(laplacian)
    return variance


def structure_check_by_mse(img1, img2, threshold=0.03):
    """Check if two images are approximately the same by MSE"""
    mse = np.mean(
        (np.array(img1, dtype=np.float32) / 255
         - np.array(img2, dtype=np.float32) / 255) ** 2)
    structure_same = True if mse < threshold else False
    print("MSE: ", mse)
    return structure_same


def structure_check_by_ssim(img1, img2, threshold=0.9):
    """Check if two images are approximately the same by SSIM"""
    similarity = ssim(np.array(img1), np.array(img2), multichannel=True, channel_axis=-1)
    print("SSIM: ", similarity)
    return similarity >= threshold


def check_brightness_decrease_and_structure_sim(src_path, tgt_path):
    """
    Check the brightness of src is lower than tgt and the structures are similar
    gimp:7a4deb26-d57d-4ea9-9a73-630f66a7b568
    """
    if src_path is None or tgt_path is None:
        return 0.

    img_src = Image.open(src_path)
    img_tgt = Image.open(tgt_path)

    # Brightness comparison
    brightness_src = calculate_brightness(img_src)
    brightness_tgt = calculate_brightness(img_tgt)
    brightness_reduced = brightness_tgt > brightness_src

    # Normalize and compare images
    target_brightness = 128
    img_src_normalized = normalize_brightness(img_src, target_brightness)
    img_tgt_normalized = normalize_brightness(img_tgt, target_brightness)

    structure_same = structure_check_by_mse(img_src_normalized, img_tgt_normalized)
    if brightness_reduced and structure_same:
        return 1.
    else:
        return 0.


def check_saturation_increase_and_structure_sim(src_path, tgt_path):
    """
    Check the saturation of src is higher than tgt and the structures are similar
    gimp:554785e9-4523-4e7a-b8e1-8016f565f56a
    """
    if src_path is None or tgt_path is None:
        return 0.

    img_src = Image.open(src_path)
    hsv_img_src = img_src.convert('HSV')
    img_tgt = Image.open(tgt_path)
    hsv_img_tgt = img_tgt.convert('HSV')

    # Saturation comparison
    src_saturation = measure_saturation(hsv_img_src)
    tgt_saturation = measure_saturation(hsv_img_tgt)

    saturation_increased = tgt_saturation < src_saturation

    # Structure comparison
    h1, s1, v1 = hsv_img_src.split()
    h2, s2, v2 = hsv_img_tgt.split()
    h_same = structure_check_by_ssim(h1, h2)
    v_same = structure_check_by_ssim(v1, v2)
    if h_same and v_same:
        structure_same = True
    else:
        structure_same = False

    if saturation_increased and structure_same:
        return 1.
    else:
        return 0.


def check_file_exists_and_structure_sim(src_path, tgt_path):
    """
    Check if the image has been exported to the desktop
    gimp:77b8ab4d-994f-43ac-8930-8ca087d7c4b4
    """
    if src_path is None or tgt_path is None:
        return 0.

    # Check if the file exists
    export_file_exists = os.path.isfile(src_path)
    if not export_file_exists:
        return 0.

    # Check whether the target image is the same as the source image
    img_src = Image.open(src_path)
    img_tgt = Image.open(tgt_path)
    structure_same = structure_check_by_ssim(img_src, img_tgt)

    if structure_same:
        return 1.
    else:
        return 0.


def check_triangle_position(tgt_path):
    """
    Check if the triangle is in the middle of the image.
    gimp:f4aec372-4fb0-4df5-a52b-79e0e2a5d6ce
    """
    if tgt_path is None:
        return 0.

    # Load the image
    img = Image.open(tgt_path)
    img_array = np.array(img)

    # We assume the triangle is a different color from the background
    # Find the unique colors
    unique_colors, counts = np.unique(img_array.reshape(-1, img_array.shape[2]), axis=0,
                                      return_counts=True)
    unique_colors_sorted = unique_colors[np.argsort(counts)]

    # Assuming the background is the most common color and the triangle is a different color
    triangle_color = unique_colors_sorted[1]

    # Create a mask where the triangle pixels are True
    triangle_mask = np.all(img_array == triangle_color, axis=2)

    # Get the coordinates of the triangle pixels
    triangle_coords = np.argwhere(triangle_mask)

    # Calculate the centroid of the triangle
    centroid = triangle_coords.mean(axis=0)

    # Check if the centroid is approximately in the middle of the image
    image_center = np.array(img_array.shape[:2]) / 2

    # We will consider the triangle to be in the middle if the centroid is within 5% of the image's center
    tolerance = 0.05 * np.array(img_array.shape[:2])
    middle = np.all(np.abs(centroid - image_center) < tolerance)

    if bool(middle):
        return 1.
    else:
        return 0.


def check_structure_sim(src_path, tgt_path):
    """
    Check if the structure of the two images are similar
    gimp:2a729ded-3296-423d-aec4-7dd55ed5fbb3
    """
    if src_path is None or tgt_path is None:
        return 0.

    img_src = Image.open(src_path)
    img_tgt = Image.open(tgt_path)
    structure_same = structure_check_by_ssim(img_src, img_tgt)
    return structure_same


def check_structure_sim_resized(src_path, tgt_path):
    """
    Check if the structure of the two images are similar after resizing.
    gimp:d16c99dc-2a1e-46f2-b350-d97c86c85c15
    """
    if src_path is None or tgt_path is None:
        return 0.

    img_src = Image.open(src_path)
    img_tgt = Image.open(tgt_path)

    # Resize the images to the same size
    img_src = img_src.resize(img_tgt.size)

    # Check if the structure is similar
    structure_same = structure_check_by_ssim(img_src, img_tgt)
    return structure_same


def check_contrast_increase_and_structure_sim(src_path, tgt_path):
    """
    Check if the src image has higher contrast than the tgt image and the structures are similar
    gimp:f723c744-e62c-4ae6-98d1-750d3cd7d79d
    """
    if src_path is None or tgt_path is None:
        return 0.

    # Load images
    source_image = Image.open(src_path)
    target_image = Image.open(tgt_path)

    # Calculate contrast
    source_contrast = calculate_contrast(source_image)
    target_contrast = calculate_contrast(target_image)
    higher_contrast = target_contrast < source_contrast

    # Check structure
    structure_same = structure_check_by_ssim(source_image, target_image, threshold=0.65)

    if higher_contrast and structure_same:
        return 1.
    else:
        return 0.


def check_config_status(actual_config_path, rule):
    """
    Check if the GIMP status is as expected
    """
    if actual_config_path is None:
        return 0.

    with open(actual_config_path, 'r') as f:
        content = f.readlines()

    for line in content:
        if line.startswith('#') or line == '\n':
            continue
        items = line.strip().lstrip('(').rstrip(')\n').split()
        if isinstance(rule["key"], str):
            if items[0] == rule["key"] and items[-1] == rule["value"]:
                return 1.
        elif isinstance(rule["key"], list) and len(rule["key"]) == 2:
            if items[0] == rule["key"][0] \
                    and items[1] == rule["key"][1] \
                    and items[-1] == rule["value"]:
                return 1.
    return 0.


def check_image_size(src_path, rule):
    """
    Check if the size of the src image is correct
    multi-apps:42f4d1c7-4521-4161-b646-0a8934e36081
    """
    if src_path is None:
        return 0.

    # Load the image
    img = Image.open(src_path)

    # Check the size
    if rule.get("height", None) is not None:
        height_same = img.size[1] == rule["height"]
    else:
        height_same = True
    if rule.get("width", None) is not None:
        width_same = img.size[0] == rule["width"]
    else:
        width_same = True

    if height_same and width_same:
        return 1.
    else:
        return 0.


def check_palette_and_structure_sim(src_path, tgt_path):
    """
    Check if the src image is palette-based and the structure of the two images are similar
    gimp:06ca5602-62ca-47f6-ad4f-da151cde54cc
    """
    if src_path is None or tgt_path is None:
        return 0.

    # Check if the source image is palette-based
    source_image = Image.open(src_path)
    palette_based = source_image.mode == 'P'

    # Check structure
    target_image = Image.open(tgt_path)
    source_image = source_image.convert('RGB')
    structure_same = structure_check_by_ssim(source_image, target_image)
    if palette_based and structure_same:
        return 1.
    else:
        return 0.


def check_textbox_on_leftside(src_path):
    """
    Check if the textbox is on the left side of the image.
    gimp:e2dd0213-26db-4349-abe5-d5667bfd725c
    """
    if src_path is None:
        return 0.

    source_image = Image.open(src_path)
    gray_image = source_image.convert("L")
    width, height = source_image.size

    # Find the bounds of the black text
    left_most_dark_pixel = width  # Start with the farthest possible left position
    for y in range(height):
        for x in range(width):
            # If the pixel is dark, consider it as part of the text
            if gray_image.getpixel((x, y)) < 128:  # Arbitrary threshold for "dark"
                left_most_dark_pixel = min(left_most_dark_pixel, x)
                break  # Stop after finding the first dark pixel in this row

    # Here we define "almost" on the left side as being within the left 5% of the image
    if left_most_dark_pixel < width * 0.05:
        return 1.
    else:
        return 0.


def check_image_mirror(src_path, tgt_path):
    """
    Check if the image is mirrored
    gimp:72f83cdc-bf76-4531-9a1b-eb893a13f8aa
    """
    if src_path is None or tgt_path is None:
        return 0.

    # Load images
    source_image = Image.open(src_path)
    target_image = Image.open(tgt_path)

    # Check if the image is mirrored
    transposed_image = source_image.transpose(Image.FLIP_LEFT_RIGHT)
    # Use 0.99 because the image may not be exactly mirrored by gimp
    mirrored = structure_check_by_ssim(transposed_image, target_image, 0.99)
    if mirrored:
        return 1.
    else:
        return 0.


def check_green_background(src_path, tgt_path):
    """
    Check if the background of the source image is green.
    gimp:734d6579-c07d-47a8-9ae2-13339795476b
    """
    if src_path is None or tgt_path is None:
        return 0.

    # Load images
    source_image = Image.open(src_path)
    target_image = Image.open(tgt_path)

    source_pixels = np.array(source_image)
    target_pixels = np.array(target_image)

    for x in range(target_image.width):
        for y in range(target_image.height):
            # Identify background pixel in target image (not black)
            if tuple(target_pixels[x, y][:3]) != (0, 0, 0):
                # Check if corresponding pixel in source image is green
                # Here, "green" means more green than red or blue
                r, g, b = source_pixels[x, y][:3]
                if not (g > r and g > b):
                    return 0.

    return 1.


def check_sharper(src_path, tgt_path):
    """
    Check if the source image is sharper than the target image.
    multi-app:bb7db4c2-30b5-4be7-8dd7-b8c4ec7d3108
    """
    sharpness_src = calculate_image_sharpness(src_path)
    sharpness_tgt = calculate_image_sharpness(tgt_path)
    return 1.0 if sharpness_src > sharpness_tgt else 0.0


def check_image_file_size(src_path, rule):
    """
    Check if the size of the src image within 500KB
    """
    if src_path is None:
        return 0.0

    # Check the size
    file_size = os.path.getsize(src_path)
    if file_size < rule["max_size"]:
        return 1.0
    else:
        return 0.0


if __name__ == "__main__":
    actual_config_path = "../../../cache/sessionrc_test"
    rule = {
        "key": "hide-docks",
        "value": "no"
    }
    print(check_config_status(actual_config_path, rule))

    actual_config_path = "../../../cache/action-history_test"
    rule = {
        "key": ["history-item", "\"filters-vignette\""],
        "value": "1"
    }
    print(check_config_status(actual_config_path, rule))

    actual_config_path = "../../../cache/gimprc_test"
    rule = {
        "key": "undo-levels",
        "value": "100"
    }
    print(check_config_status(actual_config_path, rule))

    src_path = "../../../cache/734d6579-c07d-47a8-9ae2-13339795476b/green_background_with_object.png"
    tgt_path = "../../../cache/734d6579-c07d-47a8-9ae2-13339795476b/white_background_with_object.png"
    print(check_green_background(src_path, tgt_path))

    tgt_path = "../../../cache/f4aec372-4fb0-4df5-a52b-79e0e2a5d6ce/Triangle_In_The_Middle.png"
    print(check_triangle_position(tgt_path))

    src_path = "../../../cache/bb7db4c2-30b5-4be7-8dd7-b8c4ec7d3108/anmi_sharper.png"
    tgt_path = "../../../cache/bb7db4c2-30b5-4be7-8dd7-b8c4ec7d3108/anmi.png"
    print(check_sharper(src_path, tgt_path))

    src_path = "../../../cache/3c8f201a-009d-4bbe-8b65-a6f8b35bb57f/compressed.jpeg"
    rule = {
        "max_size": 500000
    }
    print(check_image_file_size(src_path, rule))

    src_path = "../../../cache/d16c99dc-2a1e-46f2-b350-d97c86c85c15/resized.png"
    tgt_path = "../../../cache/d16c99dc-2a1e-46f2-b350-d97c86c85c15/dog_with_background.png"
    rule = {
        "height": 512
    }
    print(check_image_size(src_path, rule))
    print(check_structure_sim_resized(src_path, tgt_path))