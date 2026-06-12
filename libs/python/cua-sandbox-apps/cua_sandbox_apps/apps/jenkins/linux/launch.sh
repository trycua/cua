#!/bin/bash

echo "Starting Jenkins..."

# Ensure Jenkins home directory exists
sudo mkdir -p /var/lib/jenkins
sudo chown -R jenkins:jenkins /var/lib/jenkins

# Start Jenkins in the background
export JENKINS_HOME=/var/lib/jenkins
sudo -u jenkins /usr/bin/java -jar /opt/jenkins/jenkins.war --httpPort=8080 &

echo "Jenkins starting up... (PID: $!)"
echo "Waiting for Jenkins to initialize (this may take 30-60 seconds)..."

# Wait for Jenkins to be ready
for i in {1..120}; do
    if curl -s http://localhost:8080 > /dev/null 2>&1; then
        echo "Jenkins is ready at http://localhost:8080"
        exit 0
    fi
    echo "Attempt $i: Waiting for Jenkins to start..."
    sleep 1
done

echo "Jenkins started (may still be initializing)"
exit 0
