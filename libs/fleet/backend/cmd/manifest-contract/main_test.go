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
      app: cyclops-cs-primary
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
` + hpaDocument() + canaryDocument()
}

// These two documents are the only thing keeping the Flagger-managed
// Deployment at two replicas, since its count deliberately isn't in git.
func hpaDocument() string {
	return `---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: cyclops-cs
  namespace: cyclops-cs
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: cyclops-cs
  minReplicas: 2
  maxReplicas: 2
`
}

func canaryDocument() string {
	return `---
apiVersion: flagger.app/v1beta1
kind: Canary
metadata:
  name: cyclops-cs
  namespace: cyclops-cs
spec:
  provider: kubernetes
  targetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: cyclops-cs
  autoscalerRef:
    apiVersion: autoscaling/v2
    kind: HorizontalPodAutoscaler
    name: cyclops-cs
`
}

// The two Flagger-specific branches exist to catch a regression that puts the
// old shapes back. Without these, re-adding replicas: 2 or flipping the PDB
// selector to app: cyclops-cs would pass the suite silently — and a PDB on the
// bare app label matches zero pods, so it protects nothing while looking fine.
func TestVerifyRejectsReplicasOnTheFlaggerManagedDeployment(t *testing.T) {
	manifest := strings.Replace(
		validManifest(),
		"  name: cyclops-cs\n  namespace: cyclops-cs\nspec:\n  progressDeadlineSeconds: 600",
		"  name: cyclops-cs\n  namespace: cyclops-cs\nspec:\n  replicas: 2\n  progressDeadlineSeconds: 600",
		1,
	)
	err := verify(strings.NewReader(manifest))
	if err == nil || !strings.Contains(err.Error(), "must not set replicas") {
		t.Fatalf("verify() error = %v, want a must-not-set-replicas failure", err)
	}
}

func TestVerifyRejectsBarePDBSelectorOnTheFlaggerManagedWorkload(t *testing.T) {
	manifest := strings.Replace(validManifest(), "      app: cyclops-cs-primary", "      app: cyclops-cs", 1)
	err := verify(strings.NewReader(manifest))
	if err == nil || !strings.Contains(err.Error(), `must select app "cyclops-cs-primary"`) {
		t.Fatalf("verify() error = %v, want a PDB selector failure", err)
	}
}

// With no replicas in git, deleting the HPA or dropping the Canary's
// autoscalerRef silently returns run.cua.ai to a single pod. Each of these is
// individually sufficient to cause that, so each is rejected on its own.
func TestVerifyRejectsMissingPinnedHPA(t *testing.T) {
	manifest := strings.Replace(validManifest(), hpaDocument(), "", 1)
	err := verify(strings.NewReader(manifest))
	if err == nil || !strings.Contains(err.Error(), `HorizontalPodAutoscaler "cyclops-cs"`) {
		t.Fatalf("verify() error = %v, want a missing-HPA failure", err)
	}
}

func TestVerifyRejectsUnpinnedHPA(t *testing.T) {
	for _, test := range []struct{ name, from, to string }{
		{name: "min", from: "  minReplicas: 2", to: "  minReplicas: 1"},
		{name: "max", from: "  maxReplicas: 2", to: "  maxReplicas: 5"},
	} {
		t.Run(test.name, func(t *testing.T) {
			manifest := strings.Replace(validManifest(), test.from, test.to, 1)
			err := verify(strings.NewReader(manifest))
			if err == nil || !strings.Contains(err.Error(), "must pin minReplicas and maxReplicas to 2") {
				t.Fatalf("verify() error = %v, want an unpinned-HPA failure", err)
			}
		})
	}
}

func TestVerifyRejectsCanaryWithoutAutoscalerRef(t *testing.T) {
	manifest := strings.Replace(validManifest(), canaryDocument(), `---
apiVersion: flagger.app/v1beta1
kind: Canary
metadata:
  name: cyclops-cs
  namespace: cyclops-cs
spec:
  provider: kubernetes
  targetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: cyclops-cs
`, 1)
	err := verify(strings.NewReader(manifest))
	if err == nil || !strings.Contains(err.Error(), "must set autoscalerRef") {
		t.Fatalf("verify() error = %v, want an autoscalerRef failure", err)
	}
}

func TestVerifyRejectsCanaryOutsideServingNamespace(t *testing.T) {
	wrongCanary := strings.Replace(canaryDocument(), "  namespace: cyclops-cs", "  namespace: default", 1)
	manifest := strings.Replace(validManifest(), canaryDocument(), wrongCanary, 1)
	err := verify(strings.NewReader(manifest))
	if err == nil || !strings.Contains(err.Error(), "wrong apiVersion or namespace") {
		t.Fatalf("verify() error = %v, want a Canary namespace failure", err)
	}
}

func TestVerifyRejectsCanaryTargetingAnotherDeployment(t *testing.T) {
	wrongCanary := strings.Replace(
		canaryDocument(),
		"  targetRef:\n    apiVersion: apps/v1\n    kind: Deployment\n    name: cyclops-cs",
		"  targetRef:\n    apiVersion: apps/v1\n    kind: Deployment\n    name: another-deployment",
		1,
	)
	manifest := strings.Replace(validManifest(), canaryDocument(), wrongCanary, 1)
	err := verify(strings.NewReader(manifest))
	if err == nil || !strings.Contains(err.Error(), `must target Deployment "cyclops-cs"`) {
		t.Fatalf("verify() error = %v, want a Canary targetRef failure", err)
	}
}
