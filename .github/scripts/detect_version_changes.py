#!/usr/bin/env python3
"""
Detects version changes in pyproject.toml files and outputs tags to create.

This script is used by the version tag creator workflow to detect when package
versions have changed in pyproject.toml files and determine which git tags need
to be created.
"""

import subprocess
import sys
import os
from pathlib import Path

try:
    import toml
except ImportError:
    print("Error: toml package not installed. Run: pip install toml", file=sys.stderr)
    sys.exit(1)

# Define package mapping: directory name -> tag prefix
PACKAGE_MAP = {
    "core": "core",
    "pylume": "pylume",
    "computer": "computer",
    "som": "som",
    "agent": "agent",
    "computer-server": "computer-server",
    "mcp-server": "mcp-server"
}


def get_changed_files():
    """Get list of changed files in the last commit"""
    result = subprocess.run(
        ["git", "diff", "--name-only", "HEAD~1", "HEAD"],
        capture_output=True,
        text=True,
        check=True
    )
    return result.stdout.strip().split('\n')


def parse_pyproject_version(file_path):
    """Extract version from pyproject.toml"""
    try:
        data = toml.load(file_path)
        return data.get('project', {}).get('version')
    except Exception as e:
        print(f"Error parsing {file_path}: {e}", file=sys.stderr)
        return None


def main():
    changed_files = get_changed_files()
    tags_to_create = []

    for file_path in changed_files:
        # Check if this is a pyproject.toml in libs/python/
        if not file_path.startswith('libs/python/') or not file_path.endswith('pyproject.toml'):
            continue

        # Extract package name from path
        parts = file_path.split('/')
        if len(parts) < 4:
            continue

        package_dir = parts[2]

        if package_dir not in PACKAGE_MAP:
            print(f"Unknown package directory: {package_dir}", file=sys.stderr)
            continue

        # Get current version
        current_version = parse_pyproject_version(file_path)
        if not current_version:
            print(f"Could not parse version from {file_path}", file=sys.stderr)
            continue

        # Get previous version
        try:
            result = subprocess.run(
                ["git", "show", f"HEAD~1:{file_path}"],
                capture_output=True,
                text=True,
                check=True
            )
            # Write to temporary file for parsing
            temp_file = f"/tmp/prev_{package_dir}_pyproject.toml"
            with open(temp_file, 'w') as f:
                f.write(result.stdout)
            prev_version = parse_pyproject_version(temp_file)
            os.remove(temp_file)
        except subprocess.CalledProcessError:
            # File might be new
            prev_version = None

        # Check if version changed
        if prev_version != current_version:
            tag_prefix = PACKAGE_MAP[package_dir]
            tag_name = f"{tag_prefix}-v{current_version}"
            tags_to_create.append(tag_name)
            print(f"Version changed for {package_dir}: {prev_version} -> {current_version}")
            print(f"Will create tag: {tag_name}")

    # Output tags for GitHub Actions
    if tags_to_create:
        print(f"\nTags to create: {','.join(tags_to_create)}")
        # Write to GitHub output
        with open(os.environ.get('GITHUB_OUTPUT', '/dev/null'), 'a') as f:
            f.write(f"tags={','.join(tags_to_create)}\n")
            f.write(f"has_tags=true\n")
    else:
        print("No version changes detected")
        with open(os.environ.get('GITHUB_OUTPUT', '/dev/null'), 'a') as f:
            f.write(f"has_tags=false\n")


if __name__ == '__main__':
    main()
