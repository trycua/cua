#!/bin/bash

# Apache Cassandra Launch Script

echo "=== Starting Apache Cassandra ==="

# Set environment variables
export CASSANDRA_HOME="/opt/cassandra"
export PATH="$CASSANDRA_HOME/bin:$PATH"

# Ensure data and log directories exist
sudo mkdir -p /var/lib/cassandra
sudo mkdir -p /var/log/cassandra
sudo chown -R cassandra:cassandra /var/lib/cassandra
sudo chown -R cassandra:cassandra /var/log/cassandra
sudo mkdir -p /opt/cassandra/logs
sudo chown -R cassandra:cassandra /opt/cassandra/logs

# Configure Cassandra to use standard directories
export CASSANDRA_CONF="/etc/cassandra"
export CASSANDRA_DATA="/var/lib/cassandra"

# Start Cassandra in foreground with -R flag (force run as root for demo purposes)
echo "Starting Cassandra in foreground mode..."
echo "Cassandra will be running on port 9042 (CQL), 7000 (cluster communication), and 7199 (JMX)"
echo ""

# Run cassandra directly in foreground - this will output startup logs
exec sudo "$CASSANDRA_HOME/bin/cassandra" -f -R