"""Put the sibling ``proofs`` directory on ``sys.path`` for the portable tests.

Import this before any proof harness module. It is not a test module.
"""
from pathlib import Path
import sys

PROOFS = Path(__file__).resolve().parents[1] / 'proofs'

if str(PROOFS) not in sys.path:
    sys.path.insert(0, str(PROOFS))
