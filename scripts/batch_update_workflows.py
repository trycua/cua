#!/usr/bin/env python3
"""
Script to update package publish workflows to read version from pyproject.toml
"""

import re
import sys
from pathlib import Path

# Package configurations: (workflow_filename, package_name, package_dir)
PACKAGES = [
    ("pypi-publish-computer.yml", "computer", "libs/python/computer"),
    ("pypi-publish-som.yml", "som", "libs/python/som"),
    ("pypi-publish-pylume.yml", "pylume", "libs/python/pylume"),
    ("pypi-publish-computer-server.yml", "computer-server", "libs/python/computer-server"),
    ("pypi-publish-mcp-server.yml", "mcp-server", "libs/python/mcp-server"),
]

VERSION_DETERMINATION_TEMPLATE = '''      - name: Set up Python
        uses: actions/setup-python@v4
        with:
          python-version: "3.11"

      - name: Install toml parser
        run: pip install toml

      - name: Determine version
        id: get-version
        run: |
          # First, try to extract version from git tag if triggered by tag push
          if [ "${{{{ github.event_name }}}}" == "push" ] && [[ "${{{{ github.ref }}}}" =~ ^refs/tags/{package}-v([0-9]+\\.[0-9]+\\.[0-9]+) ]]; then
            VERSION=${{BASH_REMATCH[1]}}
            echo "Extracted version from tag: $VERSION"
          # Next, try workflow dispatch input
          elif [ "${{{{ github.event_name }}}}" == "workflow_dispatch" ] && [ -n "${{{{ github.event.inputs.version }}}}" ]; then
            VERSION=${{{{ github.event.inputs.version }}}}
            echo "Using version from workflow dispatch: $VERSION"
          # Next, try workflow_call input
          elif [ "${{{{ github.event_name }}}}" == "workflow_call" ] && [ -n "${{{{ inputs.version }}}}" ]; then
            VERSION=${{{{ inputs.version }}}}
            echo "Using version from workflow_call: $VERSION"
          else
            # Default: read from pyproject.toml
            echo "Reading version from pyproject.toml..."
            VERSION=$(python3 << EOF
          import toml
          with open("{package_dir}/pyproject.toml") as f:
              data = toml.load(f)
              print(data.get("project", {{}}).get("version", ""))
          EOF
            )
            if [ -z "$VERSION" ]; then
              echo "Error: Could not extract version from pyproject.toml"
              exit 1
            fi
            echo "Extracted version from pyproject.toml: $VERSION"
          fi
          echo "VERSION=$VERSION"
          echo "version=$VERSION" >> $GITHUB_OUTPUT
'''


def update_workflow(workflow_path: Path, package_name: str, package_dir: str):
    """Update a single workflow file"""
    print(f"Processing {workflow_path.name}...")

    if not workflow_path.exists():
        print(f"  Warning: File not found, skipping")
        return False

    content = workflow_path.read_text()

    # Check if already updated
    if "Reading version from pyproject.toml" in content:
        print(f"  Already updated, skipping")
        return False

    # Create backup
    backup_path = workflow_path.with_suffix('.yml.bak')
    backup_path.write_text(content)

    # Change macos-latest to ubuntu-latest in prepare job
    content = re.sub(
        r'(prepare:\s+runs-on:)\s+macos-latest',
        r'\1 ubuntu-latest',
        content
    )

    # Find the "Determine version" step and everything until the next step
    determine_version_pattern = r'      - name: Determine version.*?(?=\n      - name:|\n  \w+:|\Z)'

    # Replace with new version determination logic
    new_version_step = VERSION_DETERMINATION_TEMPLATE.format(
        package=package_name,
        package_dir=package_dir
    )

    # If there's already a "Determine version" step, replace it
    if re.search(determine_version_pattern, content, re.DOTALL):
        content = re.sub(
            determine_version_pattern,
            new_version_step.rstrip(),
            content,
            flags=re.DOTALL
        )
    else:
        # If there's no "Determine version" step, add it after checkout
        # Find the checkout step and add after it
        checkout_pattern = r'(- uses: actions/checkout@v4.*?\n)'
        match = re.search(checkout_pattern, content, re.DOTALL)
        if match:
            insert_pos = match.end()
            content = content[:insert_pos] + '\n' + new_version_step + content[insert_pos:]

    # Write updated content
    workflow_path.write_text(content)
    print(f"  Updated successfully")
    return True


def main():
    repo_root = Path(__file__).parent.parent
    workflows_dir = repo_root / ".github" / "workflows"

    if not workflows_dir.exists():
        print(f"Error: Workflows directory not found: {workflows_dir}")
        sys.exit(1)

    print("Updating package workflows...\n")

    updated_count = 0
    for workflow_file, package_name, package_dir in PACKAGES:
        workflow_path = workflows_dir / workflow_file
        if update_workflow(workflow_path, package_name, package_dir):
            updated_count += 1

    print(f"\nCompleted! Updated {updated_count} workflows.")
    print("Backup files created with .bak extension")


if __name__ == "__main__":
    main()
