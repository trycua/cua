#!/usr/bin/env python3
"""
Example script demonstrating the usage of OmniParser's UI element detection functionality.
This script shows how to:
1. Initialize the OmniParser
2. Load and process images
3. Visualize detection results
4. Compare performance between CPU and MPS (Apple Silicon)
"""

import argparse
import base64
import glob
import io
import logging
import os
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

import numpy as np
from PIL import Image

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.append(path)
        print(f"Added to sys.path: {path}")

# Add the libs directory to the path to find som
libs_path = project_root / "libs"
if str(libs_path) not in sys.path:
    sys.path.append(str(libs_path))
    print(f"Added to sys.path: {libs_path}")

from som import IconElement, OmniParser, ParseResult, TextElement
from som.models import BoundingBox, ParserMetadata, UIElement

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logger = logging.getLogger(__name__)


def setup_logging():
    """Configure logging with a nice format."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(levelname)s - %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
    )


class Timer:
    """Enhanced context manager for timing code blocks."""

    def __init__(self, name: str, logger):
        self.name = name
        self.logger = logger
        self.start_time: float = 0.0
        self.elapsed_time: float = 0.0

    def __enter__(self):
        self.start_time = time.time()
        return self

    def __exit__(self, *args):
        self.elapsed_time = time.time() - self.start_time
        self.logger.info(f"{self.name}: {self.elapsed_time:.3f}s")
        return False


def image_to_bytes(image: Image.Image) -> bytes:
    """Convert PIL Image to PNG bytes."""
    buf = io.BytesIO()
    image.save(buf, format="PNG")
    return buf.getvalue()


def process_image(
    parser: OmniParser, image_path: str, output_dir: Path, use_ocr: bool = False
) -> None:
    """Process a single image and save the result."""
    try:
        # Load image
        logger.info(f"Processing image: {image_path}")
        image = Image.open(image_path).convert("RGB")
        logger.info(f"Image loaded successfully, size: {image.size}")

        # Create output filename
        input_filename = Path(image_path).stem
        output_path = output_dir / f"{input_filename}_analyzed.png"

        # Convert image to PNG bytes
        image_bytes = image_to_bytes(image)

        # Process image
        with Timer(f"Processing {input_filename}", logger):
            result = parser.parse(image_bytes, use_ocr=use_ocr)
            logger.info(
                f"Found {result.metadata.num_icons} icons and {result.metadata.num_text} text elements"
            )

            # Save the annotated image
            logger.info(f"Saving annotated image to: {output_path}")
            try:
                # Save image from base64
                img_data = base64.b64decode(result.annotated_image_base64)
                img = Image.open(io.BytesIO(img_data))
                img.save(output_path)

                # Print detailed results
                logger.info("\nDetected Elements:")
                for elem in result.elements:
                    if isinstance(elem, IconElement):
                        logger.info(
                            f"Icon: confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates}"
                        )
                    elif isinstance(elem, TextElement):
                        logger.info(
                            f"Text: '{elem.content}', confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates}"
                        )

                # Verify file exists and log size
                if output_path.exists():
                    logger.info(
                        f"Successfully saved image. File size: {output_path.stat().st_size} bytes"
                    )
                else:
                    logger.error(f"Failed to verify file at {output_path}")
            except Exception as e:
                logger.error(f"Error saving image: {str(e)}", exc_info=True)

    except Exception as e:
        logger.error(f"Error processing image {image_path}: {str(e)}", exc_info=True)


def run_detection_benchmark(
    input_path: str,
    output_dir: Path,
    use_ocr: bool = False,
    box_threshold: float = 0.01,
    iou_threshold: float = 0.1,
):
    """Run detection benchmark on images."""
    logger.info(
        f"Starting benchmark with OCR enabled: {use_ocr}, box_threshold: {box_threshold}, iou_threshold: {iou_threshold}"
    )

    try:
        # Initialize parser
        logger.info("Initializing OmniParser...")
        parser = OmniParser()

        # Create output directory
        output_dir.mkdir(parents=True, exist_ok=True)
        logger.info(f"Output directory created at: {output_dir}")

        # Get list of PNG files
        if os.path.isdir(input_path):
            image_files = glob.glob(os.path.join(input_path, "*.png"))
        else:
            image_files = [input_path]

        logger.info(f"Found {len(image_files)} images to process")

        # Process each image with specified thresholds
        for image_path in image_files:
            try:
                # Load image
                logger.info(f"Processing image: {image_path}")
                image = Image.open(image_path).convert("RGB")
                logger.info(f"Image loaded successfully, size: {image.size}")

                # Create output filename
                input_filename = Path(image_path).stem
                output_path = output_dir / f"{input_filename}_analyzed.png"

                # Convert image to PNG bytes
                image_bytes = image_to_bytes(image)

                # Process image with specified thresholds
                with Timer(f"Processing {input_filename}", logger):
                    result = parser.parse(
                        image_bytes,
                        use_ocr=use_ocr,
                        box_threshold=box_threshold,
                        iou_threshold=iou_threshold,
                    )
                    logger.info(
                        f"Found {result.metadata.num_icons} icons and {result.metadata.num_text} text elements"
                    )

                    # Save the annotated image
                    logger.info(f"Saving annotated image to: {output_path}")
                    try:
                        # Save image from base64
                        img_data = base64.b64decode(result.annotated_image_base64)
                        img = Image.open(io.BytesIO(img_data))
                        img.save(output_path)

                        # Print detailed results
                        logger.info("\nDetected Elements:")
                        for elem in result.elements:
                            if isinstance(elem, IconElement):
                                logger.info(
                                    f"Icon: confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates}"
                                )
                            elif isinstance(elem, TextElement):
                                logger.info(
                                    f"Text: '{elem.content}', confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates}"
                                )

                        # Verify file exists and log size
                        if output_path.exists():
                            logger.info(
                                f"Successfully saved image. File size: {output_path.stat().st_size} bytes"
                            )
                        else:
                            logger.error(f"Failed to verify file at {output_path}")
                    except Exception as e:
                        logger.error(f"Error saving image: {str(e)}", exc_info=True)

            except Exception as e:
                logger.error(f"Error processing image {image_path}: {str(e)}", exc_info=True)

    except Exception as e:
        logger.error(f"Benchmark failed: {str(e)}", exc_info=True)
        raise


def run_experiments(input_path: str, output_dir: Path, use_ocr: bool = False):
    """Run experiments with different threshold combinations."""
    # Define threshold values to test
    box_thresholds = [0.01, 0.05, 0.1, 0.3]
    iou_thresholds = [0.05, 0.1, 0.2, 0.5]

    logger.info("Starting threshold experiments...")
    logger.info("Box thresholds to test: %s", box_thresholds)
    logger.info("IOU thresholds to test: %s", iou_thresholds)

    # Create results directory for this experiment
    timestamp = time.strftime("%Y%m%d-%H%M%S")
    ocr_suffix = "_ocr" if use_ocr else "_no_ocr"
    exp_dir = output_dir / f"experiment_{timestamp}{ocr_suffix}"
    exp_dir.mkdir(parents=True, exist_ok=True)

    # Create a summary file
    summary_file = exp_dir / "results_summary.txt"
    with open(summary_file, "w") as f:
        f.write("Threshold Experiments Results\n")
        f.write("==========================\n\n")
        f.write(f"Input: {input_path}\n")
        f.write(f"OCR Enabled: {use_ocr}\n")
        f.write(f"Date: {time.strftime('%Y-%m-%d %H:%M:%S')}\n\n")
        f.write("Results:\n")
        f.write("-" * 80 + "\n")
        f.write(
            f"{'Box Thresh':^10} | {'IOU Thresh':^10} | {'Num Icons':^10} | {'Num Text':^10} | {'Time (s)':^10}\n"
        )
        f.write("-" * 80 + "\n")

        # Initialize parser once for all experiments
        parser = OmniParser()

        # Run experiments with each combination
        for box_thresh in box_thresholds:
            for iou_thresh in iou_thresholds:
                logger.info(f"\nTesting box_threshold={box_thresh}, iou_threshold={iou_thresh}")

                # Create directory for this combination
                combo_dir = exp_dir / f"box_{box_thresh}_iou_{iou_thresh}"
                combo_dir.mkdir(exist_ok=True)

                try:
                    # Process each image
                    if os.path.isdir(input_path):
                        image_files = glob.glob(os.path.join(input_path, "*.png"))
                    else:
                        image_files = [input_path]

                    total_icons = 0
                    total_text = 0
                    total_time = 0

                    for image_path in image_files:
                        # Load and process image
                        image = Image.open(image_path).convert("RGB")
                        image_bytes = image_to_bytes(image)

                        # Process with current thresholds
                        with Timer(f"Processing {Path(image_path).stem}", logger) as t:
                            result = parser.parse(
                                image_bytes,
                                use_ocr=use_ocr,
                                box_threshold=box_thresh,
                                iou_threshold=iou_thresh,
                            )

                            # Save annotated image
                            output_path = combo_dir / f"{Path(image_path).stem}_analyzed.png"
                            img_data = base64.b64decode(result.annotated_image_base64)
                            img = Image.open(io.BytesIO(img_data))
                            img.save(output_path)

                            # Update totals
                            total_icons += result.metadata.num_icons
                            total_text += result.metadata.num_text

                            # Log detailed results
                            detail_file = combo_dir / f"{Path(image_path).stem}_details.txt"
                            with open(detail_file, "w") as detail_f:
                                detail_f.write(f"Results for {Path(image_path).name}\n")
                                detail_f.write("-" * 40 + "\n")
                                detail_f.write(f"Number of icons: {result.metadata.num_icons}\n")
                                detail_f.write(
                                    f"Number of text elements: {result.metadata.num_text}\n\n"
                                )

                                detail_f.write("Icon Detections:\n")
                                icon_count = 1
                                text_count = (
                                    result.metadata.num_icons + 1
                                )  # Text boxes start after icons

                                # First list all icons
                                for elem in result.elements:
                                    if isinstance(elem, IconElement):
                                        detail_f.write(f"Box #{icon_count}: Icon\n")
                                        detail_f.write(f"  - Confidence: {elem.confidence:.3f}\n")
                                        detail_f.write(
                                            f"  - Coordinates: {elem.bbox.coordinates}\n"
                                        )
                                        icon_count += 1

                                if use_ocr:
                                    detail_f.write("\nText Detections:\n")
                                    for elem in result.elements:
                                        if isinstance(elem, TextElement):
                                            detail_f.write(f"Box #{text_count}: Text\n")
                                            detail_f.write(f"  - Content: '{elem.content}'\n")
                                            detail_f.write(
                                                f"  - Confidence: {elem.confidence:.3f}\n"
                                            )
                                            detail_f.write(
                                                f"  - Coordinates: {elem.bbox.coordinates}\n"
                                            )
                                            text_count += 1

                        # Update timing totals
                        total_time += t.elapsed_time

                    # Write summary for this combination
                    avg_time = total_time / len(image_files)
                    f.write(
                        f"{box_thresh:^10.3f} | {iou_thresh:^10.3f} | {total_icons:^10d} | {total_text:^10d} | {avg_time:^10.3f}\n"
                    )

                except Exception as e:
                    logger.error(
                        f"Error in experiment box={box_thresh}, iou={iou_thresh}: {str(e)}"
                    )
                    f.write(
                        f"{box_thresh:^10.3f} | {iou_thresh:^10.3f} | {'ERROR':^10s} | {'ERROR':^10s} | {'ERROR':^10s}\n"
                    )

        # Write summary footer
        f.write("-" * 80 + "\n")
        f.write("\nExperiment completed successfully!\n")

    logger.info(f"\nExperiment results saved to {exp_dir}")
    logger.info(f"Summary file: {summary_file}")


def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Run OmniParser benchmark")
    parser.add_argument("input_path", help="Path to input image or directory containing images")
    parser.add_argument(
        "--output-dir", default="examples/output", help="Output directory for annotated images"
    )
    parser.add_argument(
        "--ocr",
        choices=["none", "easyocr"],
        default="none",
        help="OCR engine to use (default: none)",
    )
    parser.add_argument(
        "--mode",
        choices=["single", "experiment"],
        default="single",
        help="Run mode: single run or threshold experiments (default: single)",
    )
    parser.add_argument(
        "--box-threshold",
        type=float,
        default=0.01,
        help="Confidence threshold for detection (default: 0.01)",
    )
    parser.add_argument(
        "--iou-threshold",
        type=float,
        default=0.1,
        help="IOU threshold for Non-Maximum Suppression (default: 0.1)",
    )
    args = parser.parse_args()

    logger.info(f"Starting OmniParser with arguments: {args}")
    use_ocr = args.ocr != "none"
    output_dir = Path(args.output_dir)

    try:
        if args.mode == "experiment":
            run_experiments(args.input_path, output_dir, use_ocr)
        else:
            run_detection_benchmark(
                args.input_path, output_dir, use_ocr, args.box_threshold, args.iou_threshold
            )
    except Exception as e:
        logger.error(f"Process failed: {str(e)}", exc_info=True)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
