#!/bin/bash
set -e

echo "====== Launching Elasticsearch ======"

# Start Elasticsearch in the background as elasticsearch user
echo "Starting Elasticsearch directly..."
sudo -n -u elasticsearch /usr/share/elasticsearch/bin/elasticsearch -d -p /tmp/elasticsearch.pid

# Wait for Elasticsearch to be ready
echo "Waiting for Elasticsearch to be ready..."
for i in {1..60}; do
  if curl -s -k --cacert /etc/elasticsearch/certs/http_ca.crt https://localhost:9200/ > /dev/null 2>&1; then
    echo "✓ Elasticsearch is ready!"
    break
  fi
  echo "Attempt $i: Still waiting for Elasticsearch to start..."
  sleep 1
done

# Display basic info
echo ""
echo "====== Elasticsearch is running ======"
echo "API: https://localhost:9200"
echo ""
echo "To check status: curl -s -k --cacert /etc/elasticsearch/certs/http_ca.crt https://localhost:9200/"
echo "PID file: /tmp/elasticsearch.pid"
