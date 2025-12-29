<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Python](https://img.shields.io/badge/Python-333333?logo=python&logoColor=white&labelColor=333333)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
[![PyPI](https://img.shields.io/pypi/v/cua-computer?color=333333)](https://pypi.org/project/cua-computer/)

</h1>
</div>

**Som** (Set-of-Mark) is a visual grounding component for the Computer-Use Agent (CUA) framework powering Cua, for detecting and analyzing UI elements in screenshots. Optimized for macOS Silicon with Metal Performance Shaders (MPS), it combines YOLO-based icon detection with EasyOCR text recognition to provide comprehensive UI element analysis.

## Features

- Optimized for Apple Silicon with MPS acceleration
- Icon detection using YOLO with multi-scale processing
- Text recognition using EasyOCR (GPU-accelerated)
- Automatic hardware detection (MPS → CUDA → CPU)
- Smart detection parameters tuned for UI elements
- Detailed visualization with numbered annotations
- Performance benchmarking tools

## System Requirements

- **Recommended**: macOS with Apple Silicon
  - Uses Metal Performance Shaders (MPS)
  - Multi-scale detection enabled
  - ~0.4s average detection time
- **Supported**: Any Python 3.11+ environment
  - Falls back to CPU if no GPU available
  - Single-scale detection on CPU
  - ~1.3s average detection time

## Installation

```bash
# Using PDM (recommended)
pdm install

# Using pip
pip install -e .
```

## Quick Start

```python
from som import OmniParser
from PIL import Image

# Initialize parser
parser = OmniParser()

# Process an image
image = Image.open("screenshot.png")
result = parser.parse(
    image,
    box_threshold=0.3,    # Confidence threshold
    iou_threshold=0.1,    # Overlap threshold
    use_ocr=True         # Enable text detection
)

# Access results
for elem in result.elements:
    if elem.type == "icon":
        print(f"Icon: confidence={elem.confidence:.3f}, bbox={elem.bbox.coordinates}")
    else:  # text
        print(f"Text: '{elem.content}', confidence={elem.confidence:.3f}")
```

## Docs

- [Configuration](http://localhost:8090/docs/libraries/som/configuration)

## Development

### Test Data

- Place test screenshots in `examples/test_data/`
- Not tracked in git to keep repository size manageable
- Default test image: `test_screen.png` (1920x1080)

### Running Tests

```bash
# Run benchmark with no OCR
python examples/omniparser_examples.py examples/test_data/test_screen.png --runs 5 --ocr none

# Run benchmark with OCR
python examples/omniparser_examples.py examples/test_data/test_screen.png --runs 5 --ocr easyocr
```

## License

MIT License - See LICENSE file for details.
