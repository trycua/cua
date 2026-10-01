#!/bin/bash
set -e

echo "=== Installing Kubernetes (kubectl + minikube) ==="

# Update package manager
echo "Updating package manager..."
sudo apt-get update -qq

# Install required dependencies
echo "Installing dependencies..."
sudo apt-get install -y -qq \
    curl \
    ca-certificates \
    gnupg \
    apt-transport-https \
    containerd \
    docker.io \
    conntrack \
    socat \
    git

# Add Kubernetes repository signing key and repo
echo "Adding Kubernetes repository..."
curl -fsSL https://pkgs.k8s.io/core:/stable:/v1.35/deb/Release.key | \
    sudo gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg 2>/dev/null || true

echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v1.35/deb/ /' | \
    sudo tee /etc/apt/sources.list.d/kubernetes.list >/dev/null

# Update and install kubectl
echo "Installing kubectl..."
sudo apt-get update -qq
sudo apt-get install -y -qq kubectl

# Download and install minikube
echo "Installing minikube..."
curl -LO https://github.com/kubernetes/minikube/releases/latest/download/minikube-linux-amd64
sudo install -o root -g root -m 0755 minikube-linux-amd64 /usr/local/bin/minikube
rm minikube-linux-amd64

echo "=== Installation Complete ==="
echo ""
echo "Installed versions:"
kubectl version --client
minikube version