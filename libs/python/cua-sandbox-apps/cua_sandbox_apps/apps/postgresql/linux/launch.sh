#!/bin/bash

echo "Starting PostgreSQL server..."

# Start PostgreSQL as the postgres user
sudo -u postgres /usr/lib/postgresql/14/bin/pg_ctl -D /var/lib/postgresql/14/main -l /var/log/postgresql/postgresql-14-main.log start

# Wait for the service to be ready
sleep 2

# Check if the server is running
if sudo -u postgres /usr/lib/postgresql/14/bin/pg_isready -h localhost > /dev/null 2>&1; then
    echo "✓ PostgreSQL server started successfully"
    
    # Show PostgreSQL version
    echo ""
    echo "PostgreSQL version:"
    psql --version
    
    echo ""
    echo "Server is accepting connections on localhost:5432"
    
    # Show server info from the database
    echo ""
    echo "Database cluster information:"
    sudo -u postgres psql -c "SELECT version();" 2>/dev/null
    
else
    echo "PostgreSQL server is starting..."
    sleep 1
    if sudo -u postgres /usr/lib/postgresql/14/bin/pg_isready -h localhost > /dev/null 2>&1; then
        echo "✓ PostgreSQL server is now ready"
    else
        echo "⚠ PostgreSQL server startup in progress"
    fi
fi