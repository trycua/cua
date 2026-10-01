#!/bin/bash
# Launch Chef CLI and display help/version information

export PATH="/usr/local/bin:$PATH"

# Clear screen
clear

# Display Chef version and basic info
echo "============================================"
echo "        Chef CLI - Configuration Management"
echo "============================================"
echo ""

/usr/local/bin/chef-cli --version

echo ""
echo "Available Chef commands:"
echo "---"
/usr/local/bin/chef-cli --help 2>&1 | head -40

echo ""
echo "For more information, visit: https://www.chef.io"
echo "============================================"