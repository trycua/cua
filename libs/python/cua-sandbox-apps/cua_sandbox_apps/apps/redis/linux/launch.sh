#!/bin/bash
set -e

echo "Launching Redis..."

# Start Redis server in the background (use /tmp for log file)
redis-server --daemonize yes --logfile /tmp/redis-server.log --dir /tmp

# Give it a moment to start
sleep 2

# Verify it's running
if pgrep -x "redis-server" > /dev/null; then
    echo "✓ Redis server is running"
    
    # Show some info
    redis-cli ping
    echo ""
    redis-cli INFO server | grep -E "redis_version|process_id|uptime"
    echo ""
    echo "Redis is ready. Type 'redis-cli' to connect."
else
    echo "✗ Failed to start Redis server"
    exit 1
fi

# Keep the script running (don't exit)
sleep infinity