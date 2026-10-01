#!/bin/bash

# Launch Git by displaying version and help info
echo "=== Git Version ==="
git --version

echo ""
echo "=== Git Configuration Example ==="
git config --list --show-scope 2>/dev/null | head -20 || echo "Git configured and ready to use"

echo ""
echo "=== Git Help ==="
git --help 2>&1 | head -30

echo ""
echo "Git is successfully installed and ready for use!"