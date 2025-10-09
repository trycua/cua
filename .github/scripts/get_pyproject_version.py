#!/usr/bin/env python3
"""
Extracts the version from a pyproject.toml file.

This script is used by the publish workflow to verify that the version
in pyproject.toml matches the version being published.
"""

import sys

try:
    import toml
except ImportError:
    print("ERROR: toml package not installed. Run: pip install toml", file=sys.stderr)
    sys.exit(1)


def main():
    try:
        data = toml.load('pyproject.toml')
        version = data.get('project', {}).get('version')
        if version:
            print(version)
        else:
            print("ERROR: No version found in pyproject.toml", file=sys.stderr)
            sys.exit(1)
    except Exception as e:
        print(f"ERROR: Failed to parse pyproject.toml: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
