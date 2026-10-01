import logging
import signal
import time
from contextlib import contextmanager
from typing import Any, List, Optional, Sequence, Tuple, Union, cast

import cv2
import easyocr
import matplotlib.pyplot as plt
import numpy as np
from PIL import Image

logger = logging.getLogger(__name__)


class TimeoutException(Exception):
    pass


@contextmanager
def timeout(seconds):
    def timeout_handler(signum, frame):
        logger.warning(f"OCR process timed out after {seconds} seconds")
        raise TimeoutException("OCR processing timed out")

    # Register the signal handler
    original_handler = signal.signal(signal.SIGALRM, timeout_handler)
    signal.alarm(seconds)

    try:
        yield
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, original_handler)


# Initialize EasyOCR with optimized settings
logger.info("Initializing EasyOCR with optimized settings...")
reader = easyocr.Reader(
    ["en"],
    gpu=True,  # Use GPU if available
    model_storage_directory=None,  # Use default directory
    download_enabled=True,
    detector=True,  # Enable text detection
    recognizer=True,  # Enable text recognition
    verbose=False,  # Disable verbose output
    quantize=True,  # Enable quantization for faster inference
    cudnn_benchmark=True,  # Enable cuDNN benchmarking
)
logger.info("EasyOCR initialization complete")


def check_ocr_box(
    image_source: Union[str, Image.Image],
    display_img=True,
    output_bb_format="xywh",
    goal_filtering=None,
    easyocr_args=None,
    use_paddleocr=False,
) -> Tuple[Tuple[List[str], List[Tuple[float, float, float, float]]], Optional[Any]]:
    """Check OCR box using EasyOCR with optimized settings.

    Args:
        image_source: Either a file path or PIL Image
        display_img: Whether to display the annotated image
        output_bb_format: Format for bounding boxes ('xywh' or 'xyxy')
        goal_filtering: Optional filtering of results
        easyocr_args: Arguments for EasyOCR
        use_paddleocr: Ignored (kept for backward compatibility)

    Returns:
        Tuple containing:
        - Tuple of (text_list, bounding_boxes)
        - goal_filtering value
    """
    logger.info("Starting OCR processing...")
    start_time = time.time()

    if isinstance(image_source, str):
        logger.info(f"Loading image from path: {image_source}")
        image_source = Image.open(image_source)
    if image_source.mode == "RGBA":
        logger.info("Converting RGBA image to RGB")
        image_source = image_source.convert("RGB")
    image_np = np.array(image_source)
    w, h = image_source.size
    logger.info(f"Image size: {w}x{h}")

    # Default EasyOCR arguments optimized for speed
    default_args = {
        "paragraph": False,  # Disable paragraph detection
        "text_threshold": 0.5,  # Confidence threshold
        "link_threshold": 0.4,  # Text link threshold
        "canvas_size": 2560,  # Max image size
        "mag_ratio": 1.0,  # Magnification ratio
        "slope_ths": 0.1,  # Slope threshold
        "ycenter_ths": 0.5,  # Y-center threshold
        "height_ths": 0.5,  # Height threshold
        "width_ths": 0.5,  # Width threshold
        "add_margin": 0.1,  # Margin around text
        "min_size": 20,  # Minimum text size
    }

    # Update with user-provided arguments
    if easyocr_args:
        logger.info(f"Using custom EasyOCR arguments: {easyocr_args}")
        default_args.update(easyocr_args)

    try:
        # Use EasyOCR with timeout
        logger.info("Starting EasyOCR detection with 5 second timeout...")
        with timeout(5):  # 5 second timeout
            # EasyOCR's readtext returns a list of tuples, where each tuple is (bbox, text, confidence)
            raw_result = reader.readtext(image_np, **default_args)
            result = cast(Sequence[Tuple[List[Tuple[float, float]], str, float]], raw_result)
            coord = [item[0] for item in result]  # item[0] is the bbox coordinates
            text = [item[1] for item in result]  # item[1] is the text content
            logger.info(f"OCR completed successfully. Found {len(text)} text regions")
            logger.info(f"Detected text: {text}")

    except TimeoutException:
        logger.error("OCR processing timed out after 5 seconds")
        coord = []
        text = []
    except Exception as e:
        logger.error(f"OCR processing failed with error: {str(e)}")
        coord = []
        text = []

    processing_time = time.time() - start_time
    logger.info(f"Total OCR processing time: {processing_time:.2f} seconds")

    if display_img:
        logger.info("Creating visualization of OCR results...")
        opencv_img = cv2.cvtColor(image_np, cv2.COLOR_RGB2BGR)
        bb = []
        for item in coord:
            x, y, a, b = get_xywh(item)
            bb.append((x, y, a, b))
            # Convert float coordinates to integers for cv2.rectangle
            x_val = cast(float, x)
            y_val = cast(float, y)
            a_val = cast(float, a)
            b_val = cast(float, b)
            x_int, y_int = int(x_val), int(y_val)
            a_int, b_int = int(a_val), int(b_val)
            cv2.rectangle(
                opencv_img, (x_int, y_int), (x_int + a_int, y_int + b_int), (0, 255, 0), 2
            )
        plt.imshow(cv2.cvtColor(opencv_img, cv2.COLOR_BGR2RGB))
    else:
        if output_bb_format == "xywh":
            bb = [get_xywh(item) for item in coord]
        elif output_bb_format == "xyxy":
            bb = [get_xyxy(item) for item in coord]

    # Cast the bounding boxes to the expected type
    bb = cast(List[Tuple[float, float, float, float]], bb)

    logger.info("OCR processing complete")
    return (text, bb), goal_filtering


def get_xywh(box):
    """
    Convert a bounding box to xywh format (x, y, width, height).

    Args:
        box: Bounding box coordinates (various formats supported)

    Returns:
        Tuple of (x, y, width, height)
    """
    # Handle different input formats
    if len(box) == 4:
        # If already in xywh format or xyxy format
        if isinstance(box[0], (int, float)) and isinstance(box[2], (int, float)):
            if box[2] < box[0] or box[3] < box[1]:
                # Already xyxy format, convert to xywh
                x1, y1, x2, y2 = box
                return x1, y1, x2 - x1, y2 - y1
            else:
                # Already in xywh format
                return box
    elif len(box) == 2:
        # Format like [[x1,y1],[x2,y2]] from some OCR engines
        (x1, y1), (x2, y2) = box
        return x1, y1, x2 - x1, y2 - y1

    # Default case - try to convert assuming it's a list of points
    x_coords = [p[0] for p in box]
    y_coords = [p[1] for p in box]
    x1, y1 = min(x_coords), min(y_coords)
    width, height = max(x_coords) - x1, max(y_coords) - y1
    return x1, y1, width, height


def get_xyxy(box):
    """
    Convert a bounding box to xyxy format (x1, y1, x2, y2).

    Args:
        box: Bounding box coordinates (various formats supported)

    Returns:
        Tuple of (x1, y1, x2, y2)
    """
    # Get xywh first, then convert to xyxy
    x, y, w, h = get_xywh(box)
    return x, y, x + w, y + h
