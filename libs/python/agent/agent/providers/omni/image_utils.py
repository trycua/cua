"""Image processing utilities for the Cua provider."""

import base64
import logging
import re
from io import BytesIO
from typing import Optional, Tuple

from PIL import Image

logger = logging.getLogger(__name__)


def decode_base64_image(img_base64: str) -> Optional[Image.Image]:
    """Decode a base64 encoded image to a PIL Image.

    Args:
        img_base64: Base64 encoded image, may include data URL prefix

    Returns:
        PIL Image or None if decoding fails
    """
    try:
        # Remove data URL prefix if present
        if img_base64.startswith("data:image"):
            img_base64 = img_base64.split(",")[1]

        # Decode base64 to bytes
        img_data = base64.b64decode(img_base64)

        # Convert bytes to PIL Image
        return Image.open(BytesIO(img_data))
    except Exception as e:
        logger.error(f"Error decoding base64 image: {str(e)}")
        return None
