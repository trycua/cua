#!/bin/bash
# Wrapper script to ensure we use the correct Python with dependencies

# Try different Python paths in order of preference
PYTHON_PATHS=(
    "/Users/yellcw/Documents/GitHub/cua/.venv/bin/python"
    "/opt/homebrew/bin/python3"
    "/usr/local/bin/python3"
    "python3"
)

# Find the first Python that has the required packages
for python_path in "${PYTHON_PATHS[@]}"; do
    if command -v "$python_path" >/dev/null 2>&1; then
        # Check if it has the required packages
        if "$python_path" -c "import mcp, anyio" >/dev/null 2>&1; then
            echo "Using Python: $python_path" >&2
            exec "$python_path" "$@"
        fi
    fi
done

# If no Python with packages found, try to install them
echo "No Python with required packages found. Attempting to install..." >&2
for python_path in "${PYTHON_PATHS[@]}"; do
    if command -v "$python_path" >/dev/null 2>&1; then
        echo "Installing packages with: $python_path" >&2
        if "$python_path" -m pip install mcp anyio cua-agent[all] cua-computer >/dev/null 2>&1; then
            echo "Packages installed successfully with: $python_path" >&2
            exec "$python_path" "$@"
        fi
    fi
done

echo "Failed to find or install Python with required packages" >&2
exit 1
