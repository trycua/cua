# Contributing Reference Agent Implementations

This guide explains how to add your own reference agent implementations to the benchmark system.

## Adding Reference Agent Implementations

### 1. Implement the ModelProtocol

Create a new file in `models/` directory implementing the `ModelProtocol`:

```python
from models.base import ModelProtocol
from typing import Optional, Tuple
from PIL import Image

class YourModelName(ModelProtocol):
    def __init__(self, model_path: str):
        self.model_path = model_path
        self._model = None

    @property
    def model_name(self) -> str:
        return self.model_path

    async def load_model(self) -> None:
        """Load the model into memory."""
        # Your model loading logic here
        pass

    async def unload_model(self) -> None:
        """Unload the model from memory."""
        # Your model cleanup logic here
        pass

    async def predict_click(self, image: Image.Image, instruction: str) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates for the given image and instruction.

        Args:
            image: PIL Image to analyze
            instruction: Text instruction describing what to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        # Your prediction logic here
        return (x, y)  # Return predicted coordinates
```

### 2. Register Your Model

Add your model to the `get_available_models()` function in `utils.py`:

```python
def get_available_models() -> List[Union[str, ModelProtocol]]:
    models = [
        # Computer Agent SDK providers
        "huggingface-local/HelloKKMe/GTA1-7B",

        # Reference implementations
        GTA1Model("HelloKKMe/GTA1-7B"),
        YourModelName("path/to/your/model"),  # Add your model here
    ]
    return models
```

### 3. Test Your Implementation

Before submitting, test your model with the interactive tool:

```bash
python interactive.py
```

This will help you verify that your model loads correctly and produces reasonable predictions.

## Example: Adding a New Model

Here's a complete example of adding a hypothetical "MyVisionModel":

1. **Create `models/my_vision_model.py`:**

```python
import torch
from transformers import AutoModel, AutoProcessor
from models.base import ModelProtocol
from typing import Optional, Tuple
from PIL import Image

class MyVisionModel(ModelProtocol):
    def __init__(self, model_path: str):
        self.model_path = model_path
        self.model = None
        self.processor = None

    @property
    def model_name(self) -> str:
        return f"MyVisionModel({self.model_path})"

    async def load_model(self) -> None:
        """Load the model and processor."""
        self.processor = AutoProcessor.from_pretrained(self.model_path)
        self.model = AutoModel.from_pretrained(
            self.model_path,
            torch_dtype=torch.float16,
            device_map="auto"
        )

    async def unload_model(self) -> None:
        """Clean up model resources."""
        del self.model
        del self.processor
        self.model = None
        self.processor = None
        torch.cuda.empty_cache()

    async def predict_click(self, image: Image.Image, instruction: str) -> Optional[Tuple[int, int]]:
        """Predict click coordinates."""
        try:
            # Preprocess inputs
            inputs = self.processor(
                text=instruction,
                images=image,
                return_tensors="pt"
            )

            # Run inference
            with torch.no_grad():
                outputs = self.model(**inputs)

            # Extract coordinates (model-specific logic)
            x, y = self._extract_coordinates(outputs)
            return (int(x), int(y))

        except Exception as e:
            print(f"Prediction failed: {e}")
            return None

    def _extract_coordinates(self, outputs):
        """Extract x, y coordinates from model outputs."""
        # Your model-specific coordinate extraction logic
        pass
```

2. **Update `models/__init__.py`:**

```python
from .gta1 import GTA1Model
from .my_vision_model import MyVisionModel

__all__ = ["GTA1Model", "MyVisionModel"]
```

3. **Update `utils.py`:**

```python
from models import GTA1Model, MyVisionModel

def get_available_models() -> List[Union[str, ModelProtocol]]:
    models = [
        "huggingface-local/HelloKKMe/GTA1-7B",
        GTA1Model("HelloKKMe/GTA1-7B"),
        MyVisionModel("my-org/my-vision-model"),  # Add here
    ]
    return models
```
