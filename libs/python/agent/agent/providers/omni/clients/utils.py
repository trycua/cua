import base64


def is_image_path(text: str) -> bool:
    """Check if a text string is an image file path.
    
    Args:
        text: Text string to check
        
    Returns:
        True if text ends with image extension, False otherwise
    """
    image_extensions = (".jpg", ".jpeg", ".png", ".gif", ".bmp", ".tiff", ".tif")
    return text.endswith(image_extensions)

def encode_image(image_path: str) -> str:
    """Encode image file to base64.
    
    Args:
        image_path: Path to image file
        
    Returns:
        Base64 encoded image string
    """
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode("utf-8") 