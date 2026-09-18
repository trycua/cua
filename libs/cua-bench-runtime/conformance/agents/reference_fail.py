from __future__ import annotations

import argparse

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", required=True)
parser.add_argument("--artifacts", required=True)
parser.parse_args()
raise SystemExit(7)
