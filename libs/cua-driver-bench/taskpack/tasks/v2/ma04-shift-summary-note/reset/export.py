"""After the agent: quit TextEdit saving open edits, so the evaluator reads the files as they will stay."""

import argparse
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import mac  # noqa: E402

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--workspace", required=True, type=Path)
    ap.parse_args()
    mac.quit_saving("TextEdit")
