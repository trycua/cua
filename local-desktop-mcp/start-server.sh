#!/bin/bash
# Quick start script for the computer server

echo "================================================"
echo "Starting CUA Computer Server"
echo "================================================"
echo ""
echo "This server provides low-level desktop control"
echo "Keep this terminal open while using Claude Code"
echo ""
echo "Press Ctrl+C to stop the server"
echo ""
echo "================================================"
echo ""

python3 -m computer_server --host localhost --port 8000
