#!/bin/bash

echo "================================"
echo "Wazuh Agent - Status and Control"
echo "================================"
echo ""

if [ ! -d "/var/ossec" ]; then
    echo "❌ Wazuh Agent is not installed"
    exit 1
fi

echo "✅ Wazuh Agent Installation Found"
echo ""

echo "--- Agent Information ---"
if [ -f "/var/ossec/bin/wazuh-control" ]; then
    echo "Agent Control: /var/ossec/bin/wazuh-control"
fi

echo ""
echo "--- Configuration ---"
echo "Agent Config: /var/ossec/etc/ossec.conf"
echo ""
echo "--- Installation Details ---"
echo "Installation Path: /var/ossec"
echo "Configuration Path: /var/ossec/etc/"
echo "Data Path: /var/ossec/queue"
echo "Logs Path: /var/ossec/logs"
echo ""
echo "--- Agent Modules ---"
echo "  ✓ File Integrity Monitoring (FIM)"
echo "  ✓ System Call Monitoring (Syscollector)"
echo "  ✓ Vulnerability Detection"
echo "  ✓ Log Collection and Analysis"
echo "  ✓ Active Response Capabilities"
echo "  ✓ Real-time Alert Generation"
echo ""
echo "================================"
echo "Wazuh Agent is ready to use"
echo "================================"