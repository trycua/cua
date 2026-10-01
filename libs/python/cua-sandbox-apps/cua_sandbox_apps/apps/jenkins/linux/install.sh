#!/bin/bash

echo "=============== Jenkins Installation Script ==============="

# Update package lists (ignore errors from bad PPA)
echo "Updating package lists..."
sudo apt-get update 2>/dev/null || true

# Install Java (Jenkins requires Java)
echo "Installing Java (OpenJDK 17)..."
sudo apt-get install -y openjdk-17-jdk

# Install curl and wget
echo "Installing curl and wget..."
sudo apt-get install -y curl wget

# Create jenkins user if needed
echo "Creating jenkins user..."
sudo useradd -r -m -s /bin/bash jenkins 2>/dev/null || true

# Create jenkins directories
echo "Creating Jenkins directories..."
sudo mkdir -p /opt/jenkins
sudo mkdir -p /var/lib/jenkins
sudo chown -R jenkins:jenkins /var/lib/jenkins /opt/jenkins

# Download Jenkins WAR from official source
echo "Downloading Jenkins WAR file..."
sudo -u jenkins wget -O /opt/jenkins/jenkins.war https://get.jenkins.io/war-stable/latest/jenkins.war 2>&1

# Verify download
if [ ! -f /opt/jenkins/jenkins.war ]; then
    echo "ERROR: Failed to download Jenkins WAR"
    exit 1
fi

echo "Jenkins WAR downloaded successfully"
sudo ls -lh /opt/jenkins/jenkins.war

# Create systemd service file for Jenkins
echo "Creating systemd service file..."
sudo tee /etc/systemd/system/jenkins.service > /dev/null <<'SYSTEMD_EOF'
[Unit]
Description=Jenkins Automation Server
After=network.target

[Service]
Type=simple
User=jenkins
Group=jenkins
WorkingDirectory=/var/lib/jenkins
Environment="JENKINS_HOME=/var/lib/jenkins"
ExecStart=/usr/bin/java -jar /opt/jenkins/jenkins.war --httpPort=8080
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
SYSTEMD_EOF

# Create a simple shell script to run jenkins directly
echo "Creating jenkins wrapper script..."
sudo tee /usr/local/bin/jenkins > /dev/null <<'SCRIPT_EOF'
#!/bin/bash
export JENKINS_HOME=/var/lib/jenkins
exec /usr/bin/java -jar /opt/jenkins/jenkins.war --httpPort=8080 "$@"
SCRIPT_EOF
sudo chmod +x /usr/local/bin/jenkins

# Enable and prepare the service
echo "Enabling Jenkins service..."
sudo systemctl daemon-reload
sudo systemctl enable jenkins 2>/dev/null || true

echo "=============== Jenkins Installation Complete ==============="
echo "Jenkins WAR file location: /opt/jenkins/jenkins.war"
echo "Jenkins home directory: /var/lib/jenkins"
echo "Jenkins can be started with: sudo systemctl start jenkins"
echo "Or run directly with: /usr/local/bin/jenkins"
