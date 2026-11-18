from __future__ import annotations

import sys
from pathlib import Path

COMPONENT_ROOT = Path(__file__).resolve().parents[2]
SOURCE_ROOT = COMPONENT_ROOT / "python" / "src"

sys.path.insert(0, str(SOURCE_ROOT))
sys.path.insert(0, str(COMPONENT_ROOT))
