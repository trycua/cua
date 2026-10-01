#!/bin/bash

echo "=== MariaDB Server Launcher ==="
echo ""

# Ensure socket directory exists
sudo mkdir -p /var/run/mysqld
sudo chown mysql:mysql /var/run/mysqld 2>/dev/null || true

# Check if MariaDB is already running
if pgrep -x mysqld > /dev/null 2>&1; then
    echo "✓ MariaDB is already running"
    pgrep -a mysqld | grep mysqld
else
    echo "Starting MariaDB Server..."
    echo ""
    
    # Start MariaDB daemon in background
    sudo /usr/sbin/mysqld --user=mysql --datadir=/var/lib/mysql --socket=/var/run/mysqld/mysqld.sock --pid-file=/var/run/mysqld/mysqld.pid --skip-external-locking > /tmp/mariadb.log 2>&1 &
    
    # Wait for startup
    sleep 3
    
    # Verify it started
    if pgrep -x mysqld > /dev/null 2>&1; then
        echo "✓ MariaDB Server started successfully"
    else
        echo "✗ Failed to start MariaDB"
        cat /tmp/mariadb.log
        exit 1
    fi
fi

echo ""
echo "=== MariaDB Status ==="
pgrep -a mysqld
echo ""
echo "=== Connection Info ==="
echo "Socket: /var/run/mysqld/mysqld.sock"
echo "Port:   3306"
echo "User:   root (needs authorization setup)"
echo ""
mysqld --version
echo ""
echo "=== MariaDB Ready ==="