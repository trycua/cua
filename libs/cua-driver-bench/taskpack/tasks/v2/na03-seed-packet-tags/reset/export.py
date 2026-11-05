"""After the agent: close Finder's windows (folder and Get Info windows) so none is left for the next trial.

Runs as the bench user before the evaluator, which reads only the files. Exports nothing."""

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
    try:
        mac.finder_close_windows()
    except Exception as error:  # noqa: BLE001 - cleanup only; the verdict comes from the files
        print(f"could not close Finder windows: {error}", file=sys.stderr)
