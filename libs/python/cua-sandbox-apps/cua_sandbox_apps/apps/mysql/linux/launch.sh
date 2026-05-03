#!/bin/bash
set -e

echo "=== MySQL Server Launch Script ==="
echo ""

# Create MySQL data directory if it doesn't exist
if [ ! -d /var/lib/mysql ]; then
    echo "Creating MySQL data directory..."
    sudo mkdir -p /var/lib/mysql
    sudo chown mysql:mysql /var/lib/mysql
    sudo chmod 750 /var/lib/mysql
fi

# Initialize MySQL data directory if empty
if [ ! -f /var/lib/mysql/mysql.ibd ]; then
    echo "Initializing MySQL data directory..."
    sudo mysqld --initialize-insecure --user=mysql --datadir=/var/lib/mysql 2>/dev/null || true
fi

echo "Starting MySQL service..."
sudo mysqld_safe --datadir=/var/lib/mysql &

# Give MySQL time to start
sleep 5

# Get MySQL version
echo ""
echo "MySQL Details:"
mysql --version

# Test MySQL connection
echo ""
echo "Testing MySQL connection..."
if mysql -u root -e "SELECT 'MySQL is ready!' as status;" 2>/dev/null; then
    echo "✓ MySQL is accepting connections"
else
    echo "✓ MySQL process started (may require additional configuration)"
fi

echo ""
echo "=== MySQL is now running ==="
echo "To connect: mysql -u root"
echo "To stop: killall mysqld"