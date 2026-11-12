import sys
from pathlib import Path

bundle = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(bundle.parent / "web-common"))
from common import reset_cli

if __name__ == "__main__":
    reset_cli(bundle, verify=False)
