"""Locations of the proof harnesses, their tests, and the live helpers they reuse.

The production proofs live here. Their portable tests, compiled fixtures, and
the live-runner helpers the proofs share with ``tests/*_live.py``
(``driver_input_live``, ``primary_trace``, ``realapp_proof``,
``input_config_toggle`` and ``desktop_faults``) stay in the sibling ``tests``
directory. Import this module before those helpers.
"""
from pathlib import Path
import sys

PROOFS = Path(__file__).resolve().parent
TESTS = PROOFS.parent / 'tests'

if str(TESTS) not in sys.path:
    sys.path.append(str(TESTS))


def harness_file(name):
    """Resolve a file hashed into provenance by its name in proofs/ or tests/."""
    matches = [path for path in (PROOFS / name, TESTS / name) if path.is_file()]
    assert len(matches) == 1, f'harness file {name!r} must exist in exactly one of proofs/ or tests/'
    return matches[0]
