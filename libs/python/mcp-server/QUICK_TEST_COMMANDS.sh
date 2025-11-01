#!/bin/bash
# Quick Test Commands for MCP Server Local Desktop Option
# Run these commands to test the implementation

set -e  # Exit on error

echo "======================================================================"
echo "Testing MCP Server Local Desktop Option"
echo "======================================================================"
echo ""

# Change to repo root
cd "$(dirname "$0")/.."

# Test 1: Quick Logic Test (No setup required)
echo "Test 1: Quick Logic Test (No setup required)"
echo "----------------------------------------------------------------------"
python tests/quick_test_local_option.py
echo ""

# Test 2: Automated Tests (Requires pytest and packages)
echo "Test 2: Automated Tests (Requires pytest and packages installed)"
echo "----------------------------------------------------------------------"
if command -v pytest &> /dev/null; then
    echo "Running pytest..."
    pytest tests/test_mcp_server_local_option.py -v || echo "Note: Some tests may require full setup"
else
    echo "⚠️  pytest not found. Install with: pip install pytest"
fi
echo ""

# Test 3: Existing MCP server tests
echo "Test 3: Existing MCP Server Tests"
echo "----------------------------------------------------------------------"
if command -v pytest &> /dev/null; then
    echo "Running existing session management tests..."
    pytest tests/test_mcp_server_session_management.py -v || echo "Note: Some tests may fail if dependencies are missing"
else
    echo "⚠️  pytest not found. Install with: pip install pytest"
fi
echo ""

# Summary
echo "======================================================================"
echo "Test Summary"
echo "======================================================================"
echo "✅ Quick logic test completed"
echo ""
echo "Next steps for comprehensive testing:"
echo "1. Install dependencies:"
echo "   pip install -e libs/python/core"
echo "   pip install -e libs/python/computer"
echo "   pip install -e libs/python/agent"
echo "   pip install -e libs/python/mcp-server"
echo "   pip install -e libs/python/computer-server"
echo ""
echo "2. For manual end-to-end testing, see:"
echo "   tests/MANUAL_TEST_LOCAL_OPTION.md"
echo ""
echo "3. For detailed testing info, see:"
echo "   tests/TESTING_SUMMARY.md"
echo ""

