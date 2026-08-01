package auth

import (
	"context"
	"net/http"
	"testing"

	"github.com/open-policy-agent/opa/rego"
)

// These tests exercise the embedded Rego policies directly (authzPolicy /
// filtersPolicy) with a controlled input.flags document, so they verify the
// security logic without depending on the OpenFeature provider. The
// production code feeds the same documents via flagsData() — see
// flags_test.go for the end-to-end path.

func prepareQuery(t *testing.T, query string, modules map[string]string) rego.PreparedEvalQuery {
	t.Helper()
	opts := []func(*rego.Rego){rego.Query(query)}
	for name, src := range modules {
		opts = append(opts, rego.Module(name, src))
	}
	pq, err := rego.New(opts...).PrepareForEval(context.Background())
	if err != nil {
		t.Fatalf("prepare %q: %v", query, err)
	}
	return pq
}

func evalAllow(t *testing.T, input map[string]any) bool {
	t.Helper()
	pq := prepareQuery(t, "data.authz.allow", map[string]string{"authz.rego": authzPolicy})
	rs, err := pq.Eval(context.Background(), rego.EvalInput(input))
	if err != nil {
		t.Fatalf("eval allow: %v", err)
	}
	return rs.Allowed()
}

func evalIsAdmin(t *testing.T, input map[string]any) bool {
	t.Helper()
	pq := prepareQuery(t, "data.authz.is_admin", map[string]string{"authz.rego": authzPolicy})
	rs, err := pq.Eval(context.Background(), rego.EvalInput(input))
	if err != nil {
		t.Fatalf("eval is_admin: %v", err)
	}
	return rs.Allowed()
}

func evalVisibleEvents(t *testing.T, input map[string]any) []any {
	t.Helper()
	pq := prepareQuery(t, "data.filters.visible_events", map[string]string{
		"authz.rego":   authzPolicy,
		"filters.rego": filtersPolicy,
	})
	rs, err := pq.Eval(context.Background(), rego.EvalInput(input))
	if err != nil {
		t.Fatalf("eval visible_events: %v", err)
	}
	if len(rs) == 0 || len(rs[0].Expressions) == 0 {
		return nil
	}
	switch v := rs[0].Expressions[0].Value.(type) {
	case []any:
		return v
	case map[string]any:
		out := make([]any, 0, len(v))
		for _, e := range v {
			out = append(out, e)
		}
		return out
	}
	return nil
}

func evalPoolAdmission(t *testing.T, input map[string]any) bool {
	t.Helper()
	pq := prepareQuery(t, "data.pool_admission.allow", map[string]string{
		"pool_admission.rego": poolAdmissionPolicy,
	})
	rs, err := pq.Eval(context.Background(), rego.EvalInput(input))
	if err != nil {
		t.Fatalf("eval pool admission: %v", err)
	}
	return rs.Allowed()
}

func TestPoolAdmissionImagePullSecret(t *testing.T) {
	const allowedImage = "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo:latest"
	cases := []struct {
		name     string
		method   string
		template map[string]any
		want     bool
	}{
		{"no pull secret", "POST", map[string]any{"containerDiskImage": "docker.io/library/alpine:3.20"}, true},
		{"non ecr secret", "POST", map[string]any{"containerDiskImage": allowedImage, "imagePullSecret": "other-secret"}, false},
		{"ecr secret allowlisted image", "POST", map[string]any{"containerDiskImage": allowedImage, "imagePullSecret": "ecr-credentials"}, true},
		{"ecr secret disallowed image", "POST", map[string]any{"containerDiskImage": "evil.example/workspace:latest", "imagePullSecret": "ecr-credentials"}, false},
		{"repository prefix collision", "POST", map[string]any{"containerDiskImage": "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-evil:latest", "imagePullSecret": "ecr-credentials"}, false},
		{"allowlisted digest", "POST", map[string]any{"containerDiskImage": "296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace@sha256:abc", "imagePullSecret": "ecr-credentials"}, true},
		{"unrelated patch", "PATCH", map[string]any{"cpuCores": 8}, true},
		{"image only patch denied", "PATCH", map[string]any{"containerDiskImage": allowedImage}, false},
		{"secret only patch denied", "PATCH", map[string]any{"imagePullSecret": "ecr-credentials"}, false},
		{"remove secret patch", "PATCH", map[string]any{"imagePullSecret": nil}, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			input := map[string]any{
				"method": tc.method,
				"object": map[string]any{"spec": map[string]any{"template": tc.template}},
			}
			if got := evalPoolAdmission(t, input); got != tc.want {
				t.Fatalf("pool admission = %v, want %v", got, tc.want)
			}
		})
	}
}

func spaUser(sub string) map[string]any {
	return map[string]any{"sub": sub, "azp": "cyclops-cs-spa"}
}

// flagsFor returns an input.flags doc whose admin_subs contains sub iff admin.
func flagsFor(admin bool, sub string) map[string]any {
	subs := []any{}
	if admin {
		subs = []any{sub}
	}
	return map[string]any{"admin_subs": subs}
}

func k8sInput(path string, admin bool) map[string]any {
	const sub = "u-1"
	return map[string]any{
		"route":  "/api/k8s/{path...}",
		"method": "GET",
		"params": map[string]any{"path": path},
		"user":   spaUser(sub),
		"flags":  flagsFor(admin, sub),
	}
}

func TestK8sAllow_InfraPathsRequireAdmin(t *testing.T) {
	cases := []struct {
		name  string
		path  string
		admin bool
		want  bool
	}{
		// Infra-only: denied for non-admins, allowed for admins.
		{"nodes / non-admin", "api/v1/nodes", false, false},
		{"nodes / admin", "api/v1/nodes", true, true},
		{"node subpath / non-admin", "api/v1/nodes/ip-10-0-0-1", false, false},
		{"node subpath / admin", "api/v1/nodes/ip-10-0-0-1", true, true},
		{"cluster-wide pods / non-admin", "api/v1/pods", false, false},
		{"cluster-wide pods / admin", "api/v1/pods", true, true},
		{"batch jobs / non-admin", "apis/batch/v1/namespaces/pool-foo/jobs", false, false},
		{"batch jobs / admin", "apis/batch/v1/namespaces/pool-foo/jobs", true, true},
		{"cyclops-cs configmaps / non-admin", "api/v1/namespaces/cyclops-cs/configmaps/batch-configs", false, false},
		{"cyclops-cs configmaps / admin", "api/v1/namespaces/cyclops-cs/configmaps/batch-configs", true, true},
		// Cluster-wide CRD lists leak every tenant's pools/claims — admin only.
		{"cluster-wide pools / non-admin", "apis/cua.ai/v1/osgymworkspacepools", false, false},
		{"cluster-wide pools / admin", "apis/cua.ai/v1/osgymworkspacepools", true, true},
		{"cluster-wide claims / non-admin", "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims", false, false},
		{"cluster-wide claims / admin", "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims", true, true},
		{"Capsule tenants / non-admin", "apis/capsule.clastix.io/v1beta2/tenants", false, false},
		{"Capsule tenant item / non-admin", "apis/capsule.clastix.io/v1beta2/tenants/user-u-1", false, false},
		{"Capsule legacy tenants / non-admin", "apis/capsule.clastix.io/v1beta1/tenants", false, false},
		{"Capsule future tenant subresource / non-admin", "apis/capsule.clastix.io/v99/tenants/user-u-1/status", false, false},
		{"Capsule tenants / admin", "apis/capsule.clastix.io/v1beta2/tenants", true, true},
		{"Capsule neighboring resource / non-admin", "apis/capsule.clastix.io/v1beta2/tenantowners", false, true},
		{"Capsule group prefix collision / non-admin", "apis/capsule.clastix.io.evil/v1beta2/tenants", false, true},

		// Customer-facing: allowed regardless of admin.
		{"namespaced pods / non-admin", "api/v1/namespaces/pool-foo/pods", false, true},
		{"pod logs / non-admin", "api/v1/namespaces/pool-foo/pods/p-1/log", false, true},
		{"pod metrics / non-admin", "apis/metrics.k8s.io/v1beta1/namespaces/pool-foo/pods/p-1", false, true},
		// Namespaced pool/claim lists are how the SPA reads them — stay open
		// (Capsule RBAC scopes the user to their own namespaces).
		{"namespaced pools / non-admin", "apis/cua.ai/v1/namespaces/pool-foo/osgymworkspacepools", false, true},
		{"namespaced claims / non-admin", "apis/osgym.cua.ai/v1alpha1/namespaces/pool-foo/osgymsandboxclaims", false, true},
		{"events list / non-admin (contents filtered elsewhere)", "api/v1/events", false, true},
		{"namespaced events / non-admin", "api/v1/namespaces/pool-foo/events", false, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := evalAllow(t, k8sInput(tc.path, tc.admin)); got != tc.want {
				t.Fatalf("allow(path=%q admin=%v) = %v, want %v", tc.path, tc.admin, got, tc.want)
			}
		})
	}
}

func TestK8sAllow_RequiresSpaClient(t *testing.T) {
	// A non-SPA token (e.g. a per-key client) is denied even on an
	// otherwise-open path, and even if its sub were in admin_subs.
	in := k8sInput("api/v1/namespaces/pool-foo/pods", true)
	in["user"] = map[string]any{"sub": "u-1", "azp": "key-pool-foo"}
	if evalAllow(t, in) {
		t.Fatal("expected non-SPA client to be denied on /api/k8s")
	}
}

func TestK8sAllow_UserKeyCannotReachCapsuleTenantAPI(t *testing.T) {
	in := k8sInput("apis/capsule.clastix.io/v1beta2/tenants", false)
	in["method"] = http.MethodPost
	in["user"] = map[string]any{"sub": "u-1", "azp": "ukey-u-1"}
	if evalAllow(t, in) {
		t.Fatal("expected user key to be denied on the Capsule Tenant API")
	}
}

func TestIsAdmin(t *testing.T) {
	cases := []struct {
		name  string
		flags map[string]any
		sub   string
		want  bool
	}{
		{"sub in admin_subs", map[string]any{"admin_subs": []any{"a", "b"}}, "b", true},
		{"sub not in admin_subs", map[string]any{"admin_subs": []any{"a", "b"}}, "c", false},
		{"empty admin_subs", map[string]any{"admin_subs": []any{}}, "a", false},
		{"missing flags doc", nil, "a", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			in := map[string]any{"user": map[string]any{"sub": tc.sub}}
			if tc.flags != nil {
				in["flags"] = tc.flags
			}
			if got := evalIsAdmin(t, in); got != tc.want {
				t.Fatalf("is_admin = %v, want %v", got, tc.want)
			}
		})
	}
}

func TestVisibleEvents(t *testing.T) {
	events := []any{
		map[string]any{"reason": "PoolCreated"},
		map[string]any{"reason": "PoolReady"},
		map[string]any{"reason": "InternalScaleDecision"},
	}

	t.Run("admin sees all", func(t *testing.T) {
		in := map[string]any{
			"user":   map[string]any{"sub": "admin"},
			"flags":  map[string]any{"admin_subs": []any{"admin"}, "event_reason_allowlist": []any{"PoolCreated"}},
			"events": events,
		}
		if got := len(evalVisibleEvents(t, in)); got != 3 {
			t.Fatalf("admin visible_events = %d, want 3", got)
		}
	})

	t.Run("non-admin sees only allowlisted reasons", func(t *testing.T) {
		in := map[string]any{
			"user":   map[string]any{"sub": "nobody"},
			"flags":  map[string]any{"admin_subs": []any{"admin"}, "event_reason_allowlist": []any{"PoolCreated", "PoolReady"}},
			"events": events,
		}
		if got := len(evalVisibleEvents(t, in)); got != 2 {
			t.Fatalf("non-admin visible_events = %d, want 2", got)
		}
	})

	t.Run("non-admin with empty allowlist sees none", func(t *testing.T) {
		in := map[string]any{
			"user":   map[string]any{"sub": "nobody"},
			"flags":  map[string]any{"admin_subs": []any{"admin"}, "event_reason_allowlist": []any{}},
			"events": events,
		}
		if got := len(evalVisibleEvents(t, in)); got != 0 {
			t.Fatalf("non-admin empty allowlist visible_events = %d, want 0", got)
		}
	})

	t.Run("non-admin with missing flags sees none", func(t *testing.T) {
		in := map[string]any{
			"user":   map[string]any{"sub": "nobody"},
			"events": events,
		}
		if got := len(evalVisibleEvents(t, in)); got != 0 {
			t.Fatalf("non-admin missing flags visible_events = %d, want 0", got)
		}
	})
}
