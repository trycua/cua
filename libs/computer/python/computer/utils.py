import base64
from typing import Tuple, Optional, Dict, Any
from PIL import Image, ImageDraw
import io

def decode_base64_image(base64_str: str) -> bytes:
    """Decode a base64 string into image bytes."""
    return base64.b64decode(base64_str)

def encode_base64_image(image_bytes: bytes) -> str:
    """Encode image bytes to base64 string."""
    return base64.b64encode(image_bytes).decode('utf-8')

def bytes_to_image(image_bytes: bytes) -> Image.Image:
    """Convert bytes to PIL Image.
    
    Args:
        image_bytes: Raw image bytes
        
    Returns:
        PIL.Image: The converted image
    """
    return Image.open(io.BytesIO(image_bytes))

def image_to_bytes(image: Image.Image, format: str = 'PNG') -> bytes:
    """Convert PIL Image to bytes."""
    buf = io.BytesIO()
    image.save(buf, format=format)
    return buf.getvalue()

def resize_image(image_bytes: bytes, scale_factor: float) -> bytes:
    """Resize an image by a scale factor.
    
    Args:
        image_bytes: The original image as bytes
        scale_factor: Factor to scale the image by (e.g., 0.5 for half size, 2.0 for double)
        
    Returns:
        bytes: The resized image as bytes
    """
    image = bytes_to_image(image_bytes)
    if scale_factor != 1.0:
        new_size = (int(image.width * scale_factor), int(image.height * scale_factor))
        image = image.resize(new_size, Image.Resampling.LANCZOS)
    return image_to_bytes(image)

def draw_box(
    image_bytes: bytes,
    x: int,
    y: int,
    width: int,
    height: int,
    color: str = "#FF0000",
    thickness: int = 2
) -> bytes:
    """Draw a box on an image.
    
    Args:
        image_bytes: The original image as bytes
        x: X coordinate of top-left corner
        y: Y coordinate of top-left corner
        width: Width of the box
        height: Height of the box
        color: Color of the box in hex format
        thickness: Thickness of the box border in pixels
        
    Returns:
        bytes: The modified image as bytes
    """
    # Convert bytes to PIL Image
    image = bytes_to_image(image_bytes)
    
    # Create drawing context
    draw = ImageDraw.Draw(image)
    
    # Draw rectangle
    draw.rectangle(
        [(x, y), (x + width, y + height)],
        outline=color,
        width=thickness
    )
    
    # Convert back to bytes
    return image_to_bytes(image)

def get_image_size(image_bytes: bytes) -> Tuple[int, int]:
    """Get the dimensions of an image.
    
    Args:
        image_bytes: The image as bytes
        
    Returns:
        Tuple[int, int]: Width and height of the image
    """
    image = bytes_to_image(image_bytes)
    return image.size

def parse_vm_info(vm_info: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """Parse VM info from pylume response."""
    if not vm_info:
        return None 