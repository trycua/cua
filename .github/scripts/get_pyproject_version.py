#!/usr/bin/env python3
"""
Verifies that the version in pyproject.toml matches the expected version.

Usage:
    python get_pyproject_version.py <pyproject_path> <expected_version>

Exit codes:
    0 - Versions match
    1 - Versions don't match or error occurred
"""

import sys
import toml


def main():
    if len(sys.argv) != 3:
        print("Usage: python get_pyproject_version.py <pyproject_path> <expected_version>", file=sys.stderr)
        sys.exit(1)

    pyproject_path = sys.argv[1]
    expected_version = sys.argv[2]

    data = toml.load(pyproject_path)
    actual_version = data.get('project', {}).get('version')

    if not actual_version:
        print("❌ ERROR: No version found in pyproject.toml", file=sys.stderr)
        sys.exit(1)

    if actual_version != expected_version:
        print("❌ Version mismatch detected!", file=sys.stderr)
        print(f"   pyproject.toml version: {actual_version}", file=sys.stderr)
        print(f"   Expected version: {expected_version}", file=sys.stderr)
        print("", file=sys.stderr)
        print("The version in pyproject.toml must match the version being published.", file=sys.stderr)
        print(f"Please update pyproject.toml to version {expected_version} or use the correct tag.", file=sys.stderr)
        sys.exit(1)

    print(f"✅ Version consistency check passed: {actual_version}")


if __name__ == '__main__':
    main()
