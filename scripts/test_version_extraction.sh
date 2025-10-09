#!/bin/bash
# Test script to verify version extraction from pyproject.toml files

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "Testing version extraction from pyproject.toml files..."
echo ""

# Test each package
PACKAGES=(
    "core:libs/python/core"
    "computer:libs/python/computer"
    "som:libs/python/som"
    "agent:libs/python/agent"
    "pylume:libs/python/pylume"
    "computer-server:libs/python/computer-server"
    "mcp-server:libs/python/mcp-server"
)

all_passed=true

for package_info in "${PACKAGES[@]}"; do
    IFS=':' read -r package_name package_dir <<< "$package_info"
    pyproject_file="$REPO_ROOT/$package_dir/pyproject.toml"

    echo "Testing $package_name..."
    echo "  Path: $pyproject_file"

    if [ ! -f "$pyproject_file" ]; then
        echo "  ❌ FAIL: pyproject.toml not found"
        all_passed=false
        continue
    fi

    # Extract version using the same method as the workflow
    VERSION=$(python3 << EOF
import toml
with open("$pyproject_file") as f:
    data = toml.load(f)
    print(data.get("project", {}).get("version", ""))
EOF
    )

    if [ -z "$VERSION" ]; then
        echo "  ❌ FAIL: Could not extract version"
        all_passed=false
    else
        echo "  ✅ PASS: Version = $VERSION"

        # Verify version format (semver)
        if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "  ⚠️  WARNING: Version doesn't match semver format (X.Y.Z)"
        fi
    fi
    echo ""
done

if [ "$all_passed" = true ]; then
    echo "✅ All tests passed!"
    exit 0
else
    echo "❌ Some tests failed"
    exit 1
fi
