from __future__ import annotations

import argparse
import time

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", required=True)
parser.add_argument("--artifacts", required=True)
parser.parse_args()
time.sleep(60)
