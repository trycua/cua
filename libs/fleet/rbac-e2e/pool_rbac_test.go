// Package rbace2e reproduces — against a real kube-apiserver (envtest, no
// cluster/VM/Docker) — the production 403 a cyclops-cs user hit creating a
// workspace pool:
//
//	osgymworkspacepools.cua.ai is forbidden: User
//	"oidc:f70ef30e-9282-4a3b-b630-8a6bb4528c5d" cannot create resource
//	"osgymworkspacepools" in API group "cua.ai" in the namespace
//	"godot-qa-godot-qa"
//
// Why this is the faithful layer: the 403 is an apiserver RBAC decision, not
// anything cyclops computes. The backend's OPA policy ALLOWS namespaced pool
// creation (authz.rego) and forwards the request to the apiserver with
// impersonation headers (handlers.k8sImpersonate sets Impersonate-User:
// oidc:<sub>, Impersonate-Group: oidc:user-<sub>). The apiserver authorizes
// that impersonated identity via RBAC.
//
// Since commit 5516dcf7 ("Fix cross-tenant pool/claim data leak via RBAC
// enforcement") pool CRUD is granted ONLY per-namespace, through the `admin`
// ClusterRole that Capsule binds inside each tenant owner's namespaces —
// clusters/kopf-k3s/capsule/pool-manager-clusterrole.yaml aggregates the pool
// verbs into `admin`. Capsule creates that binding ONLY for a user it governs,
// i.e. whose group is in CapsuleConfiguration.spec.userGroups. The kopf
// provisioner patches each oidc:user-<sub> group into userGroups every 60s,
// but the capsule-config Flux Kustomization (prune, 10m) resets userGroups to
// the Git default ["oidc:team-a"]. During that window the owner is ungoverned,
// the per-namespace binding is absent, and — with no cluster-wide pool grant
// to fall back on — pool creation 403s even in the owner's OWN namespace.
//
// Two tests, both against a real apiserver with no Capsule operator:
//
//   - TestPoolCreate_Forbidden_WhenOwnerUngoverned reproduces the denial (no
//     binding -> the exact 403) and shows the per-namespace fix (bind the
//     owner's group, the way a governed owner is bound -> create succeeds).
//
//   - TestPoolCreate_Succeeds_WhenRemovedClusterWideGrantRestored is a negative
//     control: with the same apiserver/user/namespace and NO per-namespace
//     binding, restoring the exact cluster-wide grant that 5516dcf7 deleted
//     flips the 403 to success — pinning the regression to that removal.
//
// Run:
//
//	cd cyclops-cs/rbac-e2e
//	KUBEBUILDER_ASSETS="$(go run sigs.k8s.io/controller-runtime/tools/setup-envtest@release-0.19 use 1.31.0 -p path)" \
//	  go test -run TestPoolCreate -v
package rbace2e

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/dynamic"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	"sigs.k8s.io/yaml"
)

// The exact identity and namespace from the bug report.
const (
	userSub   = "f70ef30e-9282-4a3b-b630-8a6bb4528c5d"
	impUser   = "oidc:" + userSub      // Impersonate-User
	impGroup  = "oidc:user-" + userSub // Impersonate-Group
	namespace = "godot-qa-godot-qa"    // the user's workspace namespace
)

// poolClusterRole is the real production manifest whose rules Capsule folds
// into the per-namespace `admin` binding. Referencing the live file keeps the
// test honest: if the grant changes, the test reflects it.
const poolClusterRole = "../../clusters/kopf-k3s/capsule/pool-manager-clusterrole.yaml"

var poolGVR = schema.GroupVersionResource{Group: "cua.ai", Version: "v1", Resource: "osgymworkspacepools"}

func newPool(name string) *unstructured.Unstructured {
	return &unstructured.Unstructured{Object: map[string]any{
		"apiVersion": "cua.ai/v1",
		"kind":       "OSGymWorkspacePool",
		"metadata":   map[string]any{"name": name},
		"spec":       map[string]any{"replicas": int64(1)},
	}}
}

// startReproEnv boots a real apiserver (envtest) with RBAC enforced, installs
// the pool CRD, creates the target namespace, and returns an admin clientset
// (system:masters — used only for setup) plus a dynamic pool client that
// impersonates the reported user exactly like handlers.k8sImpersonate().
func startReproEnv(t *testing.T) (*kubernetes.Clientset, dynamic.NamespaceableResourceInterface) {
	t.Helper()
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS unset; see the package doc for the setup-envtest one-liner")
	}

	env := &envtest.Environment{
		CRDDirectoryPaths:     []string{"testdata"},
		ErrorIfCRDPathMissing: true,
	}
	// Enforce RBAC for impersonated identities, exactly like the real cluster.
	// (The default envtest client is in system:masters and bypasses RBAC, so it
	// can still perform setup; impersonated users are authorized by RBAC.)
	env.ControlPlane.GetAPIServer().Configure().Set("authorization-mode", "Node,RBAC")

	cfg, err := env.Start()
	if err != nil {
		t.Fatalf("start envtest apiserver: %v", err)
	}
	t.Cleanup(func() { _ = env.Stop() })

	admin := kubernetes.NewForConfigOrDie(cfg)

	// The namespace exists. In prod the backend creates it via the cluster-wide
	// `create namespaces` grant to system:authenticated (capsule-rbac/rbac.yaml);
	// here we create it directly. It is NOT adopted into the owner's tenant,
	// because Capsule only adopts/binds for users it governs.
	if _, err := admin.CoreV1().Namespaces().Create(context.Background(), &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{Name: namespace},
	}, metav1.CreateOptions{}); err != nil {
		t.Fatalf("create namespace %q: %v", namespace, err)
	}

	// Impersonate the user the way handlers.k8sImpersonate() does.
	impCfg := rest.CopyConfig(cfg)
	impCfg.Impersonate = rest.ImpersonationConfig{
		UserName: impUser,
		Groups:   []string{impGroup, "system:authenticated"},
	}
	pools := dynamic.NewForConfigOrDie(impCfg).Resource(poolGVR)
	return admin, pools
}

// createPool attempts to create a pool in the target namespace as the
// impersonated user, returning the apiserver error (nil on success).
func createPool(ctx context.Context, pools dynamic.NamespaceableResourceInterface) error {
	_, err := pools.Namespace(namespace).Create(ctx, newPool("godot-qa"), metav1.CreateOptions{})
	return err
}

// createPoolEventually retries createPool to absorb RBAC cache propagation
// (the authorizer sees a new binding near-instantly, but not synchronously).
func createPoolEventually(ctx context.Context, pools dynamic.NamespaceableResourceInterface) error {
	var err error
	for i := 0; i < 50; i++ {
		if err = createPool(ctx, pools); err == nil {
			return nil
		}
		time.Sleep(200 * time.Millisecond)
	}
	return err
}

func TestPoolCreate_Forbidden_WhenOwnerUngoverned(t *testing.T) {
	admin, pools := startReproEnv(t)
	ctx := context.Background()

	// ── Reproduce the reported 403 ───────────────────────────────────────
	// The owner's group has no RoleBinding in this namespace, and the
	// cluster-wide pool grant was removed in 5516dcf7, so nothing authorizes
	// the create.
	err := createPool(ctx, pools)
	if err == nil {
		t.Fatal("expected the reported 403, but pool creation SUCCEEDED for the ungoverned owner")
	}
	if !apierrors.IsForbidden(err) {
		t.Fatalf("expected a Forbidden (403) error, got: %v", err)
	}
	// Assert it is the exact apiserver RBAC denial from the report.
	msg := err.Error()
	for _, want := range []string{
		`cannot create resource "osgymworkspacepools"`,
		`in API group "cua.ai"`,
		impUser,
		namespace,
	} {
		if !strings.Contains(msg, want) {
			t.Fatalf("denial message missing %q\nfull message: %s", want, msg)
		}
	}
	t.Logf("REPRODUCED the reported error:\n  %s", msg)

	// ── Prove the per-namespace fix (the Capsule-governed path) ──────────
	// Apply the REAL repo ClusterRole and bind the owner's group to it in the
	// namespace — what Capsule does for a governed owner (it binds the owner to
	// `admin`, into which this ClusterRole aggregates; the ClusterRole also
	// carries the pool verbs directly). Now the identical create must succeed.
	applyRealPoolClusterRole(t, ctx, admin)
	if _, err := admin.RbacV1().RoleBindings(namespace).Create(ctx, &rbacv1.RoleBinding{
		ObjectMeta: metav1.ObjectMeta{Name: "owner-pools", Namespace: namespace},
		Subjects: []rbacv1.Subject{{
			Kind:     rbacv1.GroupKind,
			APIGroup: rbacv1.GroupName,
			Name:     impGroup,
		}},
		RoleRef: rbacv1.RoleRef{
			APIGroup: rbacv1.GroupName,
			Kind:     "ClusterRole",
			Name:     "cua:aggregate-pools-to-admin",
		},
	}, metav1.CreateOptions{}); err != nil {
		t.Fatalf("create RoleBinding for the owner: %v", err)
	}
	if err := createPoolEventually(ctx, pools); err != nil {
		t.Fatalf("once the owner's group is bound in the namespace, pool creation should succeed; got: %v", err)
	}
	t.Log("CONFIRMED: binding the owner's group in the namespace (what Capsule does for a governed owner) restores pool creation")
}

// TestPoolCreate_Succeeds_WhenRemovedClusterWideGrantRestored is a negative
// control that pins the 403 to commit 5516dcf7 specifically. That commit
// removed a cluster-wide grant from clusters/kopf-k3s/capsule-rbac/rbac.yaml:
// the `capsule-tenant-cluster-resources` ClusterRole used to grant
// system:authenticated cluster-wide CRUD on cua.ai/osgymworkspacepools (bound
// via a ClusterRoleBinding to system:authenticated).
//
// Same apiserver, same user, same namespace, NO per-namespace binding — the
// ONLY variable is that grant. Without it the create 403s (the bug); restore
// exactly the deleted rule and the identical create succeeds. So the 403 is
// caused by that removal, not by some unrelated artifact of the test harness.
func TestPoolCreate_Succeeds_WhenRemovedClusterWideGrantRestored(t *testing.T) {
	admin, pools := startReproEnv(t)
	ctx := context.Background()

	// Baseline = post-5516dcf7 state (no grant, no binding): the reported 403.
	if err := createPool(ctx, pools); !apierrors.IsForbidden(err) {
		t.Fatalf("baseline: expected Forbidden without the cluster-wide grant, got: %v", err)
	}

	// Restore EXACTLY the rule 5516dcf7 deleted (pre-regression state):
	//   ClusterRole capsule-tenant-cluster-resources
	//     - apiGroups: ["cua.ai"]
	//       resources: ["osgymworkspacepools"]
	//       verbs: ["list","get","create","update","patch","delete"]
	//   + ClusterRoleBinding of it to group system:authenticated.
	if _, err := admin.RbacV1().ClusterRoles().Create(ctx, &rbacv1.ClusterRole{
		ObjectMeta: metav1.ObjectMeta{Name: "capsule-tenant-cluster-resources"},
		Rules: []rbacv1.PolicyRule{{
			APIGroups: []string{"cua.ai"},
			Resources: []string{"osgymworkspacepools"},
			Verbs:     []string{"list", "get", "create", "update", "patch", "delete"},
		}},
	}, metav1.CreateOptions{}); err != nil {
		t.Fatalf("recreate the removed ClusterRole: %v", err)
	}
	if _, err := admin.RbacV1().ClusterRoleBindings().Create(ctx, &rbacv1.ClusterRoleBinding{
		ObjectMeta: metav1.ObjectMeta{Name: "capsule-tenant-cluster-resources"},
		Subjects: []rbacv1.Subject{{
			Kind:     rbacv1.GroupKind,
			APIGroup: rbacv1.GroupName,
			Name:     "system:authenticated",
		}},
		RoleRef: rbacv1.RoleRef{
			APIGroup: rbacv1.GroupName,
			Kind:     "ClusterRole",
			Name:     "capsule-tenant-cluster-resources",
		},
	}, metav1.CreateOptions{}); err != nil {
		t.Fatalf("recreate the removed ClusterRoleBinding: %v", err)
	}

	// Pre-regression: the identical create now succeeds — nothing else changed.
	if err := createPoolEventually(ctx, pools); err != nil {
		t.Fatalf("with the cluster-wide grant restored, pool creation should succeed; got: %v", err)
	}
	t.Log("CONFIRMED: restoring the cluster-wide grant that 5516dcf7 removed flips the identical create to success — the 403 is that regression")
}

func applyRealPoolClusterRole(t *testing.T, ctx context.Context, admin *kubernetes.Clientset) {
	t.Helper()
	raw, err := os.ReadFile(poolClusterRole)
	if err != nil {
		t.Fatalf("read %s: %v", poolClusterRole, err)
	}
	var cr rbacv1.ClusterRole
	if err := yaml.Unmarshal(raw, &cr); err != nil {
		t.Fatalf("parse ClusterRole from %s: %v", poolClusterRole, err)
	}
	if _, err := admin.RbacV1().ClusterRoles().Create(ctx, &cr, metav1.CreateOptions{}); err != nil {
		t.Fatalf("create ClusterRole %q: %v", cr.Name, err)
	}
}
