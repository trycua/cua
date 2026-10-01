#!/bin/bash

# Puppet Launch Script for Linux
# Puppet is a command-line tool for infrastructure automation
# This script will display Puppet's help and version information

echo "========================================"
echo "Puppet Infrastructure Automation"
echo "========================================"
echo ""

# Display Puppet version
echo "Puppet Version:"
puppet --version
echo ""

# Display Puppet help
echo "Puppet Commands and Options:"
echo "========================================"
puppet help
echo ""
echo "========================================"
echo ""
echo "Puppet is installed and ready to use!"
echo ""
echo "Key resources:"
echo "  • Main documentation: https://puppet.com/docs/puppet/"
echo "  • Hiera (data lookup): $(which hiera)"
echo "  • Facter (fact system): $(which facter)"
echo ""

# Show system facts
echo "System Information (via Facter):"
echo "========================================"
echo "Hostname: $(facter hostname 2>/dev/null || hostname)"
echo "OS: $(facter os.name 2>/dev/null || cat /etc/os-release | grep NAME | head -1)"
echo "Kernel: $(facter kernel.name 2>/dev/null || uname -s)"
echo ""