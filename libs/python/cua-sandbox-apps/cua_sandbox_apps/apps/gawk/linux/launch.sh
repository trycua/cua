#!/bin/bash
# Launch script for GNU Awk
# Since gawk is a CLI tool, we'll demonstrate its functionality with interactive mode

echo "=== GNU Awk (gawk) - Launch Demo ==="
echo ""
echo "GNU Awk version:"
gawk --version
echo ""
echo "=== Interactive gawk Demo ==="
echo "Starting gawk interactive mode..."
echo ""
echo "Example: Print lines containing 'a'"
echo 'hello' | gawk '/a/ {print}' || echo "(no match)"
echo ""
echo "Example: Pattern and action with field processing"
echo 'John 25 Engineer' | gawk '{print "Name: " $1 ", Age: " $2 ", Job: " $3}'
echo ""
echo "Example: Multiple records"
echo -e "apple 5\nbanana 3\ncherry 8" | gawk '{sum += $2} END {print "Total: " sum}'
echo ""
echo "=== gawk is ready for use ==="
echo "Run 'gawk --help' for more information"