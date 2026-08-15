package main

import (
	"strings"
	"testing"
)

func TestVerifyAcceptsServingObjects(t *testing.T) {
	t.Parallel()

	if err := verify(strings.NewReader(validManifest())); err != nil {
		t.Fatalf("verify() error = %v, want nil", err)
	}
}

func TestVerifyRejectsPodTemplateLabelMismatch(t *testing.T) {
	t.Parallel()

	manifest := strings.Replace(
		validManifest(),
		"      app: cyclops-cs\n    spec:\n      topologySpreadConstraints:",
		"      app: wrong-app\n    spec:\n      topologySpreadConstraints:",
		1,
	)

	err := verify(strings.NewReader(manifest))
	if err == nil {
		t.Fatal("verify() error = nil, want pod template label mismatch")
	}
	if !strings.Contains(err.Error(), "pod template app label") {
		t.Fatalf("verify() error = %q, want pod template app label failure", err)
	}
}

func TestVerifyRejectsStateProjector(t *testing.T) {
	t.Parallel()

	err := verify(strings.NewReader(validManifest() + `
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: k8s-state-projector
  namespace: cyclops-cs
`))
	if err == nil {
		t.Fatal("verify() error = nil, want state projector rejection")
	}
	if !strings.Contains(err.Error(), "state projector") {
		t.Fatalf("verify() error = %q, want state projector failure", err)
	}
}

func TestVerifyProjectorAcceptsExpectedImage(t *testing.T) {
	t.Parallel()

	manifest := strings.Replace(validProjectorManifest(), "PROJECTOR_IMAGE", "296062593712.dkr.ecr.us-west-2.amazonaws.com/cyclops-cs-backend:main-1786507556", 1)
	if err := verifyProjector(strings.NewReader(manifest)); err != nil {
		t.Fatalf("verifyProjector() error = %v, want nil", err)
	}
}

func TestVerifyProjectorRejectsWrongImage(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name  string
		image string
		want  string
	}{
		{
			name:  "repository",
			image: "296062593712.dkr.ecr.us-west-2.amazonaws.com/wrong-repository:main-1786507556",
			want:  "repository",
		},
		{
			name:  "tag",
			image: "296062593712.dkr.ecr.us-west-2.amazonaws.com/cyclops-cs-backend:latest",
			want:  "tag",
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			manifest := strings.Replace(validProjectorManifest(), "PROJECTOR_IMAGE", test.image, 1)

			err := verifyProjector(strings.NewReader(manifest))
			if err == nil {
				t.Fatal("verifyProjector() error = nil, want image validation failure")
			}
			if !strings.Contains(err.Error(), test.want) {
				t.Fatalf("verifyProjector() error = %q, want %q failure", err, test.want)
			}
		})
	}
}

func validProjectorManifest() string {
	return `apiVersion: v1
kind: ServiceAccount
metadata:
  name: k8s-state-projector
  namespace: cyclops-cs
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: cyclops-cs-k8s-state-projector
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: cyclops-cs-k8s-state-projector
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: k8s-state-projector
  namespace: cyclops-cs
spec:
  template:
    spec:
      containers:
        - name: projector
          image: PROJECTOR_IMAGE
`
}

func validManifest() string {
	return `apiVersion: apps/v1
kind: Deployment
metadata:
  name: cyclops-cs
  namespace: cyclops-cs
spec:
  replicas: 2
  progressDeadlineSeconds: 600
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxUnavailable: 0
      maxSurge: 1
  selector:
    matchLabels:
      app: cyclops-cs
  template:
    metadata:
      labels:
        app: cyclops-cs
    spec:
      topologySpreadConstraints:
        - maxSkew: 1
          topologyKey: kubernetes.io/hostname
          whenUnsatisfiable: ScheduleAnyway
          labelSelector:
            matchLabels:
              app: cyclops-cs
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: cyclops-cs
  namespace: cyclops-cs
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app: cyclops-cs
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cyclops-cs-backend
  namespace: cyclops-cs
spec:
  replicas: 2
  progressDeadlineSeconds: 600
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxUnavailable: 0
      maxSurge: 1
  selector:
    matchLabels:
      app: cyclops-cs-backend
  template:
    metadata:
      labels:
        app: cyclops-cs-backend
    spec:
      topologySpreadConstraints:
        - maxSkew: 1
          topologyKey: kubernetes.io/hostname
          whenUnsatisfiable: ScheduleAnyway
          labelSelector:
            matchLabels:
              app: cyclops-cs-backend
      containers:
        - name: cyclops-cs-backend
          readinessProbe:
            httpGet:
              path: /healthz
          livenessProbe:
            httpGet:
              path: /healthz
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: cyclops-cs-backend
  namespace: cyclops-cs
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app: cyclops-cs-backend
`
}
