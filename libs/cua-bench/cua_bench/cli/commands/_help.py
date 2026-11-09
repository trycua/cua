"""Help-text helpers shared by the ``cb`` parsers."""

from __future__ import annotations

import argparse


def examples(*items: tuple[str, str]) -> dict:
    """``add_parser`` keyword arguments for an ``Examples:`` epilog.

    Each item is ``(description, command)``. The epilog keeps its layout in
    ``--help`` and the generated CLI reference renders (and checks) the same
    examples.
    """
    lines = ["Examples:"]
    for i, (description, command) in enumerate(items):
        if i:
            lines.append("")
        lines.append(f"  # {description}")
        lines.append(f"  {command}")
    return {
        "epilog": "\n".join(lines),
        "formatter_class": argparse.RawDescriptionHelpFormatter,
    }
