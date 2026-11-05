"""Verify the reset of MA-01."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common  # noqa: E402

if __name__ == "__main__":
    common.verify_main(BUNDLE, "MA-01")
