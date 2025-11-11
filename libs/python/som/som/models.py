from typing import Any, Dict, List, Literal, Optional, Tuple, Union

from pydantic import BaseModel, Field, validator


class BoundingBox(BaseModel):
    """Normalized bounding box coordinates."""

    x1: float = Field(..., description="Normalized left coordinate")
    y1: float = Field(..., description="Normalized top coordinate")
    x2: float = Field(..., description="Normalized right coordinate")
    y2: float = Field(..., description="Normalized bottom coordinate")

    @property
    def coordinates(self) -> List[float]:
        """Get coordinates as a list [x1, y1, x2, y2]."""
        return [self.x1, self.y1, self.x2, self.y2]


class UIElement(BaseModel):
    """Base class for UI elements."""

    id: Optional[int] = Field(None, description="Unique identifier for the element (1-indexed)")
    type: Literal["icon", "text"]
    bbox: BoundingBox
    interactivity: bool = Field(default=False, description="Whether the element is interactive")
    confidence: float = Field(default=1.0, description="Detection confidence score")


class IconElement(UIElement):
    """An interactive icon element."""

    type: Literal["icon"] = "icon"
    interactivity: bool = True
    scale: Optional[int] = Field(None, description="Detection scale used")


class TextElement(UIElement):
    """A text element."""

    type: Literal["text"] = "text"
    content: str = Field(..., description="The text content")
    interactivity: bool = False


class ImageData(BaseModel):
    """Image data with dimensions."""

    base64: str = Field(..., description="Base64 encoded image data")
    width: int = Field(..., description="Image width in pixels")
    height: int = Field(..., description="Image height in pixels")

    @validator("width", "height")
    def dimensions_must_be_positive(cls, v):
        if v <= 0:
            raise ValueError("Dimensions must be positive")
        return v


class ParserMetadata(BaseModel):
    """Metadata about the parsing process."""

    image_size: Tuple[int, int] = Field(
        ..., description="Original image dimensions (width, height)"
    )
    num_icons: int = Field(..., description="Number of icons detected")
    num_text: int = Field(..., description="Number of text elements detected")
    device: str = Field(..., description="Device used for detection (cpu/cuda/mps)")
    ocr_enabled: bool = Field(..., description="Whether OCR was enabled")
    latency: float = Field(..., description="Total processing time in seconds")

    @property
    def width(self) -> int:
        """Get image width from image_size."""
        return self.image_size[0]

    @property
    def height(self) -> int:
        """Get image height from image_size."""
        return self.image_size[1]


class ParseResult(BaseModel):
    """Result of parsing a UI screenshot."""

    elements: List[UIElement] = Field(..., description="Detected UI elements")
    annotated_image_base64: str = Field(..., description="Base64 encoded annotated image")
    metadata: ParserMetadata = Field(..., description="Processing metadata")
    screen_info: Optional[List[str]] = Field(
        None, description="Human-readable descriptions of elements"
    )
    parsed_content_list: Optional[List[Dict[str, Any]]] = Field(
        None, description="Parsed elements as dictionaries"
    )

    @property
    def image(self) -> ImageData:
        """Get image data as a convenience property."""
        return ImageData(
            base64=self.annotated_image_base64,
            width=self.metadata.width,
            height=self.metadata.height,
        )

    @property
    def width(self) -> int:
        """Get image width from metadata."""
        return self.metadata.width

    @property
    def height(self) -> int:
        """Get image height from metadata."""
        return self.metadata.height

    def model_dump(self) -> Dict[str, Any]:
        """Convert model to dict for compatibility with older code."""
        result = super().model_dump()
        # Add image data dict for backward compatibility
        result["image"] = self.image.model_dump()
        return result
