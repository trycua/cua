#!/bin/bash
set -e

echo "=== Kubernetes CLI Tool ==="
echo ""
echo "kubectl - Kubernetes Command-Line Interface"
echo ""

# Show kubectl version
echo "Kubectl Version Information:"
kubectl version --client
echo ""

# Show kubectl help
echo "Kubectl Help (basic commands):"
kubectl --help | head -40
echo ""

# Create a sample Kubernetes manifest directory for demonstration
echo "Creating sample Kubernetes configuration..."
mkdir -p ~/.kube
cat > ~/.kube/config.example << 'EOF'
# This is an example kubeconfig file
# It would typically connect to a Kubernetes cluster
apiVersion: v1
kind: Config
current-context: kubernetes-admin@kubernetes
clusters:
- cluster:
    certificate-authority-data: LS0tLS1CRUdJTi... (base64 encoded)
    server: https://kubernetes.default:443
  name: kubernetes
contexts:
- context:
    cluster: kubernetes
    user: kubernetes-admin
  name: kubernetes-admin@kubernetes
users:
- name: kubernetes-admin
  user:
    client-certificate-data: LS0tLS1CRUdJTi... (base64 encoded)
    client-key-data: LS0tLS1CRUdJTi... (base64 encoded)
EOF

echo "Sample config created at ~/.kube/config.example"
echo ""

# Show what commands are available
echo "Kubernetes kubectl available commands:"
kubectl api-resources 2>/dev/null | head -20 || echo "Note: No cluster connected, but kubectl is fully installed"
echo ""

echo "=== Kubernetes Successfully Installed ==="
echo ""
echo "Installation Details:"
echo "- Binary: $(which kubectl)"
echo "- Minikube: $(which minikube)"
echo "- Version: $(kubectl version --client | grep -oP 'v[0-9.]+' | head -1)"