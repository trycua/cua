import argparse
import base64
import io
import logging
import signal
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple, Union, cast

import cv2
import numpy as np
import supervision as sv
import torch
import torchvision.ops
import torchvision.transforms as T
from huggingface_hub import hf_hub_download
from PIL import Image
from supervision.detection.core import Detections
from ultralytics import YOLO

from .detection import DetectionProcessor
from .models import (
    BoundingBox,
    IconElement,
    ParseResult,
    ParserMetadata,
    TextElement,
    UIElement,
)
from .ocr import OCRProcessor
from .visualization import BoxAnnotator

logger = logging.getLogger(__name__)


class TimeoutException(Exception):
    pass


@contextmanager
def timeout(seconds: int):
    def timeout_handler(signum, frame):
        raise TimeoutException("OCR process timed out")

    # Register the signal handler
    original_handler = signal.signal(signal.SIGALRM, timeout_handler)
    signal.alarm(seconds)

    try:
        yield
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, original_handler)


def process_text_box(box, image):
    """Process a single text box with OCR."""
    try:
        from typing import Any, List, Sequence, Tuple

        import easyocr

        x1 = int(min(point[0] for point in box))
        y1 = int(min(point[1] for point in box))
        x2 = int(max(point[0] for point in box))
        y2 = int(max(point[1] for point in box))

        # Add padding
        pad = 2
        x1 = max(0, x1 - pad)
        y1 = max(0, y1 - pad)
        x2 = min(image.shape[1], x2 + pad)
        y2 = min(image.shape[0], y2 + pad)

        region = image[y1:y2, x1:x2]
        if region.size > 0:
            reader = easyocr.Reader(["en"])
            results = reader.readtext(region)
            if results and len(results) > 0:
                # EasyOCR returns a list of tuples (bbox, text, confidence)
                first_result = results[0]
                if isinstance(first_result, (list, tuple)) and len(first_result) >= 3:
                    text = str(first_result[1])
                    confidence = float(first_result[2])
                    if confidence > 0.5:
                        return text, [x1, y1, x2, y2], confidence
    except Exception:
        pass
    return None


def check_ocr_box(image_path: Union[str, Path]) -> Tuple[List[str], List[List[float]]]:
    """Check OCR box using EasyOCR."""
    # Read image once
    if isinstance(image_path, str):
        image_path = Path(image_path)

    # Read image into memory
    image_cv = cv2.imread(str(image_path))
    if image_cv is None:
        logger.error(f"Failed to read image: {image_path}")
        return [], []

    # Get image dimensions
    img_height, img_width = image_cv.shape[:2]
    confidence_threshold = 0.5

    # Use EasyOCR
    import ssl

    import easyocr

    # Create unverified SSL context for development
    ssl._create_default_https_context = ssl._create_unverified_context
    try:
        reader = easyocr.Reader(["en"])
        with timeout(5):  # 5 second timeout for EasyOCR
            results = reader.readtext(image_cv, paragraph=False, text_threshold=0.5)
    except TimeoutException:
        logger.warning("EasyOCR timed out, returning no results")
        return [], []
    except Exception as e:
        logger.warning(f"EasyOCR failed: {str(e)}")
        return [], []
    finally:
        # Restore default SSL context
        ssl._create_default_https_context = ssl.create_default_context

    texts = []
    boxes = []

    for box, text, conf in results:
        # Convert box format to [x1, y1, x2, y2]
        x1 = min(point[0] for point in box)
        y1 = min(point[1] for point in box)
        x2 = max(point[0] for point in box)
        y2 = max(point[1] for point in box)

        if float(conf) > 0.5:  # Only keep higher confidence detections
            texts.append(text)
            boxes.append([x1, y1, x2, y2])

    return texts, boxes


class OmniParser:
    """Enhanced UI parser using computer vision and OCR for detecting interactive elements."""

    def __init__(
        self,
        model_path: Optional[Union[str, Path]] = None,
        cache_dir: Optional[Union[str, Path]] = None,
        force_device: Optional[str] = None,
    ):
        """Initialize the OmniParser.

        Args:
            model_path: Optional path to the YOLO model
            cache_dir: Optional directory to cache model files
            force_device: Force specific device (cpu/cuda/mps)
        """
        self.detector = DetectionProcessor(
            model_path=Path(model_path) if model_path else None,
            cache_dir=Path(cache_dir) if cache_dir else None,
            force_device=force_device,
        )
        self.ocr = OCRProcessor()
        self.visualizer = BoxAnnotator()

    def process_image(
        self,
        image: Image.Image,
        box_threshold: float = 0.3,
        iou_threshold: float = 0.1,
        use_ocr: bool = True,
    ) -> Tuple[Image.Image, List[UIElement]]:
        """Process an image to detect UI elements and optionally text.

        Args:
            image: Input PIL Image
            box_threshold: Confidence threshold for detection
            iou_threshold: IOU threshold for NMS
            use_ocr: Whether to enable OCR processing

        Returns:
            Tuple of (annotated image, list of detections)
        """
        try:
            logger.info("Starting UI element detection...")

            # Detect icons
            icon_detections = self.detector.detect_icons(
                image=image, box_threshold=box_threshold, iou_threshold=iou_threshold
            )
            logger.info(f"Found {len(icon_detections)} interactive elements")

            # Convert icon detections to typed objects
            elements: List[UIElement] = cast(
                List[UIElement],
                [
                    IconElement(
                        id=i + 1,
                        bbox=BoundingBox(
                            x1=det["bbox"][0],
                            y1=det["bbox"][1],
                            x2=det["bbox"][2],
                            y2=det["bbox"][3],
                        ),
                        confidence=det["confidence"],
                        scale=det.get("scale"),
                    )
                    for i, det in enumerate(icon_detections)
                ],
            )

            # Run OCR if enabled
            if use_ocr:
                logger.info("Running OCR detection...")
                text_detections = self.ocr.detect_text(image=image, confidence_threshold=0.5)
                if text_detections is None:
                    text_detections = []
                logger.info(f"Found {len(text_detections)} text regions")

                # Convert text detections to typed objects
                text_elements = cast(
                    List[UIElement],
                    [
                        TextElement(
                            id=len(elements) + i + 1,
                            bbox=BoundingBox(
                                x1=det["bbox"][0],
                                y1=det["bbox"][1],
                                x2=det["bbox"][2],
                                y2=det["bbox"][3],
                            ),
                            content=det["content"],
                            confidence=det["confidence"],
                        )
                        for i, det in enumerate(text_detections)
                    ],
                )

                if elements and text_elements:
                    # Filter out non-OCR elements that have OCR elements with center points colliding with them
                    filtered_elements = []
                    for elem in elements:  # elements at this point contains only non-OCR elements
                        should_keep = True
                        for text_elem in text_elements:
                            # Calculate center point of the text element
                            center_x = (text_elem.bbox.x1 + text_elem.bbox.x2) / 2
                            center_y = (text_elem.bbox.y1 + text_elem.bbox.y2) / 2

                            # Check if this center point is inside the non-OCR element
                            if (
                                center_x >= elem.bbox.x1
                                and center_x <= elem.bbox.x2
                                and center_y >= elem.bbox.y1
                                and center_y <= elem.bbox.y2
                            ):
                                should_keep = False
                                break

                        if should_keep:
                            filtered_elements.append(elem)
                    elements = filtered_elements

                    # Merge detections using NMS
                    all_elements = elements + text_elements
                    boxes = torch.tensor([elem.bbox.coordinates for elem in all_elements])
                    scores = torch.tensor([elem.confidence for elem in all_elements])
                    keep_indices = torchvision.ops.nms(boxes, scores, iou_threshold)
                    elements = [all_elements[i] for i in keep_indices]
                else:
                    # Just add text elements to the list if IOU doesn't need to be applied
                    elements.extend(text_elements)

            # Calculate drawing parameters based on image size
            box_overlay_ratio = max(image.size) / 3200
            draw_config = {
                "font_size": int(12 * box_overlay_ratio),
                "box_thickness": max(int(2 * box_overlay_ratio), 1),
                "text_padding": max(int(3 * box_overlay_ratio), 1),
            }

            # Convert elements back to dict format for visualization
            detection_dicts = [
                {
                    "type": elem.type,
                    "bbox": elem.bbox.coordinates,
                    "confidence": elem.confidence,
                    "content": elem.content if isinstance(elem, TextElement) else None,
                }
                for elem in elements
            ]

            # Create visualization
            logger.info("Creating visualization...")
            annotated_image = self.visualizer.draw_boxes(
                image=image.copy(), detections=detection_dicts, draw_config=draw_config
            )
            logger.info("Visualization complete")

            return annotated_image, elements

        except Exception as e:
            logger.error(f"Error in process_image: {str(e)}")
            import traceback

            logger.error(traceback.format_exc())
            raise

    def parse(
        self,
        screenshot_data: Union[bytes, str],
        box_threshold: float = 0.3,
        iou_threshold: float = 0.1,
        use_ocr: bool = True,
    ) -> ParseResult:
        """Parse a UI screenshot to detect interactive elements and text.

        Args:
            screenshot_data: Raw bytes or base64 string of the screenshot
            box_threshold: Confidence threshold for detection
            iou_threshold: IOU threshold for NMS
            use_ocr: Whether to enable OCR processing

        Returns:
            ParseResult object containing elements, annotated image, and metadata
        """
        try:
            start_time = time.time()

            # Convert input to PIL Image
            if isinstance(screenshot_data, str):
                screenshot_data = base64.b64decode(screenshot_data)
            image = Image.open(io.BytesIO(screenshot_data)).convert("RGB")

            # Process image
            annotated_image, elements = self.process_image(
                image=image,
                box_threshold=box_threshold,
                iou_threshold=iou_threshold,
                use_ocr=use_ocr,
            )

            # Convert annotated image to base64
            buffered = io.BytesIO()
            annotated_image.save(buffered, format="PNG")
            annotated_image_base64 = base64.b64encode(buffered.getvalue()).decode("utf-8")

            # Generate screen info text
            screen_info = []
            parsed_content_list = []

            # Set element IDs and generate human-readable descriptions
            for i, elem in enumerate(elements):
                # Set the ID (1-indexed)
                elem.id = i + 1

                if isinstance(elem, IconElement):
                    screen_info.append(
                        f"Box #{i+1}: Icon (confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates})"
                    )
                    parsed_content_list.append(
                        {
                            "id": i + 1,
                            "type": "icon",
                            "bbox": elem.bbox.coordinates,
                            "confidence": elem.confidence,
                            "content": None,
                        }
                    )
                elif isinstance(elem, TextElement):
                    screen_info.append(
                        f"Box #{i+1}: Text '{elem.content}' (confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates})"
                    )
                    parsed_content_list.append(
                        {
                            "id": i + 1,
                            "type": "text",
                            "bbox": elem.bbox.coordinates,
                            "confidence": elem.confidence,
                            "content": elem.content,
                        }
                    )

            # Calculate metadata
            latency = time.time() - start_time
            width, height = image.size

            # Create ParseResult object with enhanced properties
            result = ParseResult(
                elements=elements,
                annotated_image_base64=annotated_image_base64,
                screen_info=screen_info,
                parsed_content_list=parsed_content_list,
                metadata=ParserMetadata(
                    image_size=(width, height),
                    num_icons=len([e for e in elements if isinstance(e, IconElement)]),
                    num_text=len([e for e in elements if isinstance(e, TextElement)]),
                    device=self.detector.device,
                    ocr_enabled=use_ocr,
                    latency=latency,
                ),
            )

            # Return the ParseResult object directly
            return result

        except Exception as e:
            logger.error(f"Error in parse: {str(e)}")
            import traceback

            logger.error(traceback.format_exc())
            raise


def main():
    """Command line interface for UI element detection."""
    parser = argparse.ArgumentParser(description="Detect UI elements and text in images")
    parser.add_argument("image_path", help="Path to the input image")
    parser.add_argument("--model-path", help="Path to YOLO model")
    parser.add_argument(
        "--box-threshold", type=float, default=0.3, help="Box confidence threshold (default: 0.3)"
    )
    parser.add_argument(
        "--iou-threshold", type=float, default=0.1, help="IOU threshold (default: 0.1)"
    )
    parser.add_argument(
        "--ocr", action="store_true", default=True, help="Enable OCR processing (default: True)"
    )
    parser.add_argument("--output", help="Output path for annotated image")
    args = parser.parse_args()

    # Setup logging
    logging.basicConfig(level=logging.INFO)

    try:
        # Initialize parser
        parser = OmniParser(model_path=args.model_path)

        # Load and process image
        logger.info(f"Loading image from: {args.image_path}")
        image = Image.open(args.image_path).convert("RGB")
        logger.info(f"Image loaded successfully, size: {image.size}")

        # Process image
        annotated_image, elements = parser.process_image(
            image=image,
            box_threshold=args.box_threshold,
            iou_threshold=args.iou_threshold,
            use_ocr=args.ocr,
        )

        # Save output image
        output_path = args.output or str(
            Path(args.image_path).parent
            / f"{Path(args.image_path).stem}_analyzed{Path(args.image_path).suffix}"
        )
        logger.info(f"Saving annotated image to: {output_path}")

        Path(output_path).parent.mkdir(parents=True, exist_ok=True)
        annotated_image.save(output_path)
        logger.info(f"Image saved successfully to {output_path}")

        # Print detections
        logger.info("\nDetections:")
        for i, elem in enumerate(elements):
            if isinstance(elem, IconElement):
                logger.info(
                    f"Interactive element {i}: confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates}"
                )
            elif isinstance(elem, TextElement):
                logger.info(f"Text {i}: '{elem.content}', bbox={elem.bbox.coordinates}")

    except Exception as e:
        logger.error(f"Error processing image: {str(e)}")
        import traceback

        logger.error(traceback.format_exc())
        return 1

    return 0


if __name__ == "__main__":
    import sys

    sys.exit(main())
