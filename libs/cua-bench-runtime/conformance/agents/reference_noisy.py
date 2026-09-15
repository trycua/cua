from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", type=Path, required=True)
parser.add_argument("--artifacts", type=Path, required=True)
parser.parse_args()

for _ in range(20):
    sys.stdout.write("x" * 100_000)
    sys.stdout.flush()
    time.sleep(0.01)
