"""Export NA-06's app state for the evaluator (runs as the bench user after the agent)."""

import argparse
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import mac  # noqa: E402

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--workspace", required=True, type=Path)
    mac.write_export(ap.parse_args().workspace, "notes", mac.notes_export(None))
