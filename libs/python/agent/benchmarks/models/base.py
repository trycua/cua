"""
Base protocol for benchmark models.
"""

from typing import Optional, Protocol, Tuple

from PIL import Image


class ModelProtocol(Protocol):
    """Protocol for benchmark models that can predict click coordinates."""

    @property
    def model_name(self) -> str:
        """Return the name of the model."""
        ...

    async def load_model(self) -> None:
        """Load the model into memory."""
        ...

    async def unload_model(self) -> None:
        """Unload the model from memory."""
        ...

    async def predict_click(
        self, image: Image.Image, instruction: str
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates for the given image and instruction.

        Args:
            image: PIL Image to analyze
            instruction: Text instruction describing what to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        ...
