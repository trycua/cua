package auth

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"testing"

	"github.com/open-policy-agent/opa/ast"
	"github.com/open-policy-agent/opa/rego"
)

// These tests exercise the embedded Rego policies directly (authzPolicy and the
// surface modules) with a controlled input.flags document, so they verify the
// security logic without depending on the OpenFeature provider. The
// production code feeds the same documents via flagsData() — see
// flags_test.go for the end-to-end path.

// TestRouteModulesNeverReadInputFacts pins an invariant the route policies rely
// on: PolicyMiddleware always puts a facts document in the input, built only
// from the fact providers the evaluating leaf declared, and neither leaf in
// policy_routes.go declares any. So input.facts is always {} for these modules,
// and a rule that reads it is reading an empty object — undefined lookups,
// silently.
//
// It is an assertion over the parsed module rather than a set of requests on
// purpose. The equivalence harness this replaced sampled the routes its table
// listed, which was 14 of 34 — a rule reading input.facts under any other route
// was invisible to it. Walking the refs holds for every route, method, and
// input at once, and it cost nothing to keep when that harness was deleted.
//
// If a leaf in policy_routes.go ever gains WithFacts, the module it names stops
// belonging in this table — reading input.facts is the point of that option.
func TestRouteModulesNeverReadInputFacts(t *testing.T) {
	modules := map[string]string{
		"authz.rego":          authzPolicy,
		"pool_admission.rego": poolAdmissionPolicy,
	}
	// Every base and surface module too, read from the same map LoadOpa
	// registers, so a surface added there is covered without being named here.
	for _, module := range surfacePolicySources {
		modules[module.filename] = module.source
	}
	factsRef := ast.MustParseRef("input.facts")

	for name, source := range modules {
		t.Run(name, func(t *testing.T) {
			module, err := ast.ParseModule(name, source)
			if err != nil {
				t.Fatalf("parse %s: %v", name, err)
			}
			ast.WalkRefs(module, func(ref ast.Ref) bool {
				if ref.HasPrefix(factsRef) {
					t.Errorf(`%s reads %v, but the leaf that compiles it declares no fact
providers, so input.facts is always {} and this lookup is undefined. Either
give that leaf a WithFacts provider in policy_routes.go, or read the value
from somewhere that is actually populated.`, name, ref)
				}
				// Keep walking: report every offending ref, not just the first.
				return false
			})
		})
	}
}

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

func TestPerKeyClientUsesConfiguredPrefixFromUserInput(t *testing.T) {
	query := prepareQuery(t, "data.authz.is_per_key_client", map[string]string{"authz.rego": authzPolicy})
	for _, testCase := range []struct {
		name string
		user *User
		want bool
	}{
		{name: "custom prefix client", user: &User{AZP: "poolkey-ns-a", KeyClientPfx: "poolkey-"}, want: true},
		{name: "legacy prefix client", user: &User{AZP: "key-ns-a", KeyClientPfx: "poolkey-"}, want: false},
		{name: "default prefix client", user: &User{AZP: "key-ns-a"}, want: true},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			results, err := query.Eval(context.Background(), rego.EvalInput(map[string]any{"user": buildUserInput(testCase.user)}))
			if err != nil {
				t.Fatalf("evaluate per-key client policy: %v", err)
			}
			got := len(results) > 0 && len(results[0].Expressions) > 0 && results[0].Expressions[0].Value == true
			if got != testCase.want {
				t.Fatalf("is_per_key_client = %t, want %t; input = %#v", got, testCase.want, buildUserInput(testCase.user))
			}
		})
	}
}

func TestSignedServiceURLsCustomPerKeyPrefixIsNamespaceScoped(t *testing.T) {
	for _, testCase := range []struct {
		name      string
		namespace string
		want      bool
	}{
		{name: "matching namespace", namespace: "ns-a", want: true},
		{name: "other namespace", namespace: "ns-b", want: false},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			input := map[string]any{
				"route":  "/api/signed-service-urls/{namespace}",
				"method": http.MethodGet,
				"params": map[string]string{"namespace": testCase.namespace},
				"user":   buildUserInput(&User{ID: "svc-1", AZP: "poolkey-ns-a", KeyClientPfx: "poolkey-", Namespace: "ns-a"}),
				"flags":  map[string]any{},
			}
			if got := evalAllow(t, input); got != testCase.want {
				t.Fatalf("allow(namespace=%q) = %t, want %t", testCase.namespace, got, testCase.want)
			}
		})
	}
}

// evalAllow answers what the route's production policy would, over a raw input
// document. There is no single module to query any more: a route runs
// All(base, surface), so this resolves the tree main.go dispatches input.route
// to and folds its leaves the way compiledAll does.
//
// Leaves that read the request body are skipped — pool admission is one, and it
// has evalPoolAdmission for its own coverage. So an "allow" here means the route
// *authorization* allowed it, which is what every caller of this helper is
// asking about.
func evalAllow(t *testing.T, input map[string]any) bool {
	t.Helper()
	LoadOpa()

	route, _ := input["route"].(string)
	tree, ok := RouteTree(route)
	if !ok {
		t.Fatalf("input names route %q, which is bound to no authorization surface", route)
	}
	return evalPolicyNode(t, tree, input)
}

func evalPolicyNode(t *testing.T, node Node, input map[string]any) bool {
	t.Helper()
	switch policy := node.(type) {
	case Leaf:
		if policy.MaxBody > 0 {
			return true
		}
		modules, err := policy.Source.modules()
		if err != nil {
			t.Fatalf("load modules for %q: %v", policy.Query, err)
		}
		sources := make(map[string]string, len(modules))
		for _, module := range modules {
			sources[module.name] = module.source
		}
		result, err := prepareQuery(t, policy.Query, sources).Eval(context.Background(), rego.EvalInput(input))
		if err != nil {
			t.Fatalf("eval %q: %v", policy.Query, err)
		}
		return result.Allowed()
	case AllNode:
		for _, child := range policy.Children {
			if !evalPolicyNode(t, child, input) {
				return false
			}
		}
		return true
	case BecauseNode:
		return evalPolicyNode(t, policy.Child, input)
	case AnyNode:
		for _, child := range policy.Children {
			if evalPolicyNode(t, child, input) {
				return true
			}
		}
		return false
	default:
		t.Fatalf("route policy contains unknown node %T", node)
		return false
	}
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

func evalChatEnabled(t *testing.T, input map[string]any) bool {
	t.Helper()
	pq := prepareQuery(t, "data.authz.chat_enabled", map[string]string{"authz.rego": authzPolicy})
	rs, err := pq.Eval(context.Background(), rego.EvalInput(input))
	if err != nil {
		t.Fatalf("eval chat_enabled: %v", err)
	}
	return rs.Allowed()
}

func TestChatEnabled(t *testing.T) {
	tests := []struct {
		name  string
		input map[string]any
		want  bool
	}{
		{name: "admin", input: map[string]any{"user": spaUser("admin"), "flags": map[string]any{"admin_subs": []any{"admin"}, "chat_subs": []any{}}}, want: true},
		{name: "allowlisted", input: map[string]any{"user": spaUser("listed"), "flags": map[string]any{"admin_subs": []any{}, "chat_subs": []any{"listed"}}}, want: true},
		{name: "unlisted", input: map[string]any{"user": spaUser("other"), "flags": map[string]any{"admin_subs": []any{}, "chat_subs": []any{"listed"}}}, want: false},
		{name: "missing allowlist", input: map[string]any{"user": spaUser("other"), "flags": map[string]any{"admin_subs": []any{}}}, want: false},
		{name: "malformed allowlist", input: map[string]any{"user": spaUser("other"), "flags": map[string]any{"admin_subs": []any{}, "chat_subs": "other"}}, want: false},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := evalChatEnabled(t, test.input); got != test.want {
				t.Fatalf("chat_enabled = %v, want %v", got, test.want)
			}
		})
	}
}

func evalPoolAdmission(t *testing.T, input map[string]any) bool {
	t.Helper()
	pq := prepareQuery(t, "data.pool_admission.allow", map[string]string{
		"authz.rego":          authzPolicy,
		"pool_admission.rego": poolAdmissionPolicy,
	})
	rs, err := pq.Eval(context.Background(), rego.EvalInput(input))
	if err != nil {
		t.Fatalf("eval pool admission: %v", err)
	}
	return rs.Allowed()
}

func TestPoolAdmissionImagePullSecret(t *testing.T) {
	const allowedImage = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest"
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
		{"omarchy digest", "POST", map[string]any{"containerDiskImage": "296062593712.dkr.ecr.us-west-2.amazonaws.com/omarchy-workspace@sha256:c9cdba09d8cd2f742b9e9fa3818ca29dbcb66ee40edd057621e2987098226950", "imagePullSecret": "ecr-credentials"}, true},
		{"unrelated patch", "PATCH", map[string]any{"cpuCores": 8}, true},
		{"image only patch denied", "PATCH", map[string]any{"containerDiskImage": allowedImage}, false},
		{"secret only patch denied", "PATCH", map[string]any{"imagePullSecret": "ecr-credentials"}, false},
		{"remove secret patch", "PATCH", map[string]any{"imagePullSecret": nil}, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			body, err := json.Marshal(map[string]any{"spec": map[string]any{"template": tc.template}})
			if err != nil {
				t.Fatalf("marshal body: %v", err)
			}
			input := map[string]any{
				"method": tc.method,
				"params": map[string]any{"path": "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"},
				"body":   string(body),
				"user":   map[string]any{"sub": "user-1"},
				"flags":  flagsFor(false, "user-1"),
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

// TestK8sAllow_AllowlistVerdicts records what the allowlist admits and what it
// refuses. Since #6704 there is no "requires admin" tier: admin is not a way
// around the allowlist, so a path is either enumerated or it is denied, and the
// admin column exists here only to show that it changes nothing.
func TestK8sAllow_AllowlistVerdicts(t *testing.T) {
	cases := []struct {
		name  string
		path  string
		admin bool
		want  bool
	}{
		// Cluster-scoped and infra paths: denied for everyone. These rows used
		// to read non-admin deny / admin allow.
		{"nodes / non-admin", "api/v1/nodes", false, false},
		{"nodes / admin", "api/v1/nodes", true, false},
		{"node subpath / admin", "api/v1/nodes/ip-10-0-0-1", true, false},
		{"cluster-wide pods / admin", "api/v1/pods", true, false},
		{"batch jobs / admin", "apis/batch/v1/namespaces/pool-foo/jobs", true, false},
		{"cyclops-cs configmaps / admin", "api/v1/namespaces/cyclops-cs/configmaps/batch-configs", true, false},
		{"cluster-wide pools / admin", "apis/cua.ai/v1/osgymworkspacepools", true, false},
		{"cluster-wide claims / admin", "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims", true, false},
		{"cluster-wide warm pools / admin", "apis/osgym.cua.ai/v1alpha1/osgymsandboxwarmpools", true, false},
		{"cluster-wide templates / non-admin", "apis/osgym.cua.ai/v1alpha1/osgymsandboxtemplates", false, false},
		{"cluster-wide sandboxes / non-admin", "apis/osgym.cua.ai/v1alpha1/osgymsandboxes", false, false},
		{"Capsule tenants / admin", "apis/capsule.clastix.io/v1beta2/tenants", true, false},
		{"Capsule tenant item / non-admin", "apis/capsule.clastix.io/v1beta2/tenants/user-u-1", false, false},
		{"Capsule legacy tenants / non-admin", "apis/capsule.clastix.io/v1beta1/tenants", false, false},
		// The lookalikes that used to prove is_infra_k8s_path is exact. They are
		// denied now for the ordinary reason -- nobody enumerated them -- so the
		// precision they were guarding is asserted against the predicate itself
		// in authz_k8s_test.rego, where a route verdict can no longer see it.
		{"Capsule neighboring resource / non-admin", "apis/capsule.clastix.io/v1beta2/tenantowners", false, false},
		{"Capsule group prefix collision / non-admin", "apis/capsule.clastix.io.evil/v1beta2/tenants", false, false},

		// Enumerated and therefore admitted. k8sInput drives GET, which is the
		// only verb every one of these shares; the per-verb matrix lives in
		// authz_k8s_test.rego.
		{"namespaced pods / non-admin", "api/v1/namespaces/pool-foo/pods", false, true},
		{"pod logs / non-admin", "api/v1/namespaces/pool-foo/pods/p-1/log", false, true},
		{"pod metrics / non-admin", "apis/metrics.k8s.io/v1beta1/namespaces/pool-foo/pods/p-1", false, true},
		{"namespaced services / non-admin", "api/v1/namespaces/pool-foo/services", false, true},
		{"kubevirt VMs / non-admin", "apis/kubevirt.io/v1/namespaces/pool-foo/virtualmachines", false, true},
		{"discovery / non-admin", "openapi/v2", false, true},
		// Namespaced pool/claim lists are how the SDK reads them
		// (Capsule RBAC scopes the user to their own namespaces).
		{"namespaced pools / non-admin", "apis/cua.ai/v1/namespaces/pool-foo/osgymworkspacepools", false, true},
		{"namespaced claims / non-admin", "apis/osgym.cua.ai/v1alpha1/namespaces/pool-foo/osgymsandboxclaims", false, true},
		{"namespaced warm pools / non-admin", "apis/osgym.cua.ai/v1alpha1/namespaces/pool-foo/osgymsandboxwarmpools", false, true},
		{"namespaced templates / non-admin", "apis/osgym.cua.ai/v1alpha1/namespaces/pool-foo/osgymsandboxtemplates", false, true},
		{"namespaced sandboxes / non-admin", "apis/osgym.cua.ai/v1alpha1/namespaces/pool-foo/osgymsandboxes", false, true},
		{"namespaced sandboxes / admin", "apis/osgym.cua.ai/v1alpha1/namespaces/pool-foo/osgymsandboxes", true, true},

		// Unenumerated, and denied for that reason alone -- no exclusion list
		// mentions any of them. The Secret read is the one a reviewer is most
		// likely to want to argue about: it succeeded in production, from the
		// SDK live-test flow, and the allowlist drops it.
		{"namespaced secrets / non-admin", "api/v1/namespaces/pool-foo/secrets/ecr-credentials", false, false},
		{"namespaced configmaps / non-admin", "api/v1/namespaces/pool-foo/configmaps", false, false},
		{"storage classes / non-admin", "apis/storage.k8s.io/v1/storageclasses", false, false},

		// Events: denied for everyone, admins included. Unlike every infra path
		// above, the admin row here is a deny too — that asymmetry is the whole
		// difference between "hidden from the nav" and "not proxied".
		//
		// All eight served shapes: two API groups, each cluster-scoped and
		// namespaced, each with a legacy /watch/ alias.
		{"cluster events / non-admin", "api/v1/events", false, false},
		{"cluster events / admin", "api/v1/events", true, false},
		{"cluster event item / non-admin", "api/v1/events/evt-1", false, false},
		{"namespaced events / non-admin", "api/v1/namespaces/pool-foo/events", false, false},
		{"namespaced events / admin", "api/v1/namespaces/pool-foo/events", true, false},
		{"namespaced event item / non-admin", "api/v1/namespaces/pool-foo/events/evt-1", false, false},
		{"group cluster events / non-admin", "apis/events.k8s.io/v1/events", false, false},
		{"group cluster events / admin", "apis/events.k8s.io/v1/events", true, false},
		{"group namespaced events / non-admin", "apis/events.k8s.io/v1/namespaces/pool-foo/events", false, false},
		{"group event item / non-admin", "apis/events.k8s.io/v1/namespaces/pool-foo/events/evt-1", false, false},
		{"group other version events / non-admin", "apis/events.k8s.io/v1beta1/events", false, false},
		{"legacy watch cluster events / non-admin", "api/v1/watch/events", false, false},
		{"legacy watch namespaced events / non-admin", "api/v1/watch/namespaces/pool-foo/events", false, false},
		{"legacy watch group events / non-admin", "apis/events.k8s.io/v1/watch/events", false, false},
		{"legacy watch group namespaced events / non-admin", "apis/events.k8s.io/v1/watch/namespaces/pool-foo/events", false, false},
		// Neighbours of the event paths. Only one is still an allow: "events" as
		// a namespace name, on a resource the allowlist admits. The rest are
		// denied because they are unenumerated, not because the event predicate
		// matched them -- which is exactly why that predicate's precision is now
		// asserted directly in authz_k8s_test.rego instead of here.
		{"namespace named events / non-admin", "api/v1/namespaces/events/pods", false, true},
		{"event lookalike resource / non-admin", "api/v1/namespaces/pool-foo/eventsources", false, false},
		{"event group prefix collision / non-admin", "apis/events.k8s.io.evil/v1/events", false, false},
		{"events group neighbour resource / non-admin", "apis/events.k8s.io/v1/eventsources", false, false},
		{"watch on non-events / non-admin", "api/v1/watch/namespaces/pool-foo/pods", false, false},
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

func TestK8sAllow_GitHubExactScope(t *testing.T) {
	githubInput := func(method, path string, namespaces ...string) map[string]any {
		allowed := make([]any, len(namespaces))
		for index, namespace := range namespaces {
			allowed[index] = namespace
		}
		return map[string]any{
			"route":  "/api/k8s/{path...}",
			"method": method,
			"params": map[string]any{"path": path},
			"user": map[string]any{
				"sub":                "owner-1",
				"azp":                "github-oidc",
				"principal_type":     "github_oidc",
				"allowed_namespaces": allowed,
			},
			"flags": flagsFor(false, "owner-1"),
		}
	}

	cases := []struct {
		name       string
		method     string
		path       string
		namespaces []string
		want       bool
	}{
		{"legacy collection get", http.MethodGet, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", []string{"ns-a"}, true},
		{"legacy collection post", http.MethodPost, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", []string{"ns-a"}, true},
		{"legacy collection patch", http.MethodPatch, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", []string{"ns-a"}, true},
		{"legacy collection delete", http.MethodDelete, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", []string{"ns-a"}, true},
		{"legacy item post", http.MethodPost, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools/pool-a", []string{"ns-a"}, true},
		{"legacy item patch", http.MethodPatch, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools/pool-a", []string{"ns-a"}, true},
		{"native warm pool item get", http.MethodGet, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools/pool-a", []string{"ns-a"}, true},
		{"native warm pool item patch", http.MethodPatch, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools/pool-a", []string{"ns-a"}, true},
		{"native warm pool item post denied", http.MethodPost, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools/pool-a", []string{"ns-a"}, false},
		{"native warm pool collection patch denied", http.MethodPatch, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools", []string{"ns-a"}, false},
		{"native claim collection post", http.MethodPost, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims", []string{"ns-a"}, true},
		{"native claim item delete", http.MethodDelete, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a", []string{"ns-a"}, true},
		{"native template get", http.MethodGet, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates/template-a", []string{"ns-a"}, true},
		{"namespace outside grant", http.MethodGet, "apis/cua.ai/v1/namespaces/ns-b/osgymworkspacepools", []string{"ns-a"}, false},
		{"template write denied", http.MethodPost, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates", []string{"ns-a"}, false},
		{"claim item post denied", http.MethodPost, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a", []string{"ns-a"}, false},
		{"claim collection patch denied", http.MethodPatch, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims", []string{"ns-a"}, false},
		{"claim item patch", http.MethodPatch, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a", []string{"ns-a"}, true},
		{"claim subresource denied", http.MethodGet, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a/status", []string{"ns-a"}, false},
		{"cluster path denied", http.MethodGet, "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims", []string{"ns-a"}, false},
		{"empty item denied", http.MethodGet, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools/", []string{"ns-a"}, false},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			input := githubInput(testCase.method, testCase.path, testCase.namespaces...)
			if got := evalAllow(t, input); got != testCase.want {
				t.Fatalf("allow(%s %s) = %v, want %v", testCase.method, testCase.path, got, testCase.want)
			}
		})
	}
}

func TestPoolAdmissionRequestPolicy(t *testing.T) {
	const allowedImage = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest"
	input := func(method, path, body, sub string, admin bool) map[string]any {
		return map[string]any{
			"method": method,
			"params": map[string]any{"path": path},
			"body":   body,
			"user":   map[string]any{"sub": sub},
			"flags":  flagsFor(admin, sub),
		}
	}

	cases := []struct {
		name   string
		input  map[string]any
		wanted bool
	}{
		{
			name:   "unrelated request allowed",
			input:  input(http.MethodPost, "api/v1/namespaces/ns-a/configmaps", `not json`, "user-1", false),
			wanted: true,
		},
		{
			name:   "malformed pool body denied",
			input:  input(http.MethodPost, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", `not json`, "user-1", false),
			wanted: false,
		},
		{
			name:   "disallowed image denied",
			input:  input(http.MethodPost, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", `{"spec":{"template":{"containerDiskImage":"evil.example/image:latest","imagePullSecret":"ecr-credentials"}}}`, "user-1", false),
			wanted: false,
		},
		{
			name:   "allowed image accepted",
			input:  input(http.MethodPost, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", fmt.Sprintf(`{"spec":{"template":{"containerDiskImage":%q,"imagePullSecret":"ecr-credentials"}}}`, allowedImage), "user-1", false),
			wanted: true,
		},
		{
			name:   "mixed-case macos denied for non-admin",
			input:  input(http.MethodPost, "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", `{"spec":{"template":{"runtime":"MacOS"}}}`, "user-1", false),
			wanted: false,
		},
		{
			name:   "macos runtime class denied for non-admin",
			input:  input(http.MethodPost, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates", `{"spec":{"vmTemplate":{"runtimeClassName":"cua-macos-native"}}}`, "user-1", false),
			wanted: false,
		},
		{
			name:   "macos node selector denied for non-admin",
			input:  input(http.MethodPatch, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates/template-a", `{"spec":{"vmTemplate":{"nodeSelector":{"cua.ai/macos":"true"}}}}`, "user-1", false),
			wanted: false,
		},
		{
			name:   "macos accepted for admin",
			input:  input(http.MethodPost, "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates", `{"spec":{"vmTemplate":{"runtime":"macos"}}}`, "admin-1", true),
			wanted: true,
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			if got := evalPoolAdmission(t, testCase.input); got != testCase.wanted {
				t.Fatalf("pool admission = %v, want %v", got, testCase.wanted)
			}
		})
	}
}

func TestPoolAdmissionIgnoresLookalikeResourceGroups(t *testing.T) {
	input := map[string]any{
		"method": http.MethodPost,
		"params": map[string]any{"path": "apis/evil.example/v1/namespaces/ns-a/osgymworkspacepools"},
		"body":   `not json`,
		"user":   map[string]any{"sub": "user-1"},
		"flags":  flagsFor(false, "user-1"),
	}
	if !evalPoolAdmission(t, input) {
		t.Fatal("lookalike resource group should be outside pool admission policy scope")
	}
}

func TestSignedServiceURLsRejectLegacyPerKeyPrefixUnderCustomConfiguration(t *testing.T) {
	input := map[string]any{
		"route":  "/api/signed-service-urls/{namespace}",
		"method": http.MethodGet,
		"params": map[string]string{"namespace": "ns-a"},
		"user":   buildUserInput(&User{ID: "svc-1", AZP: "key-ns-a", KeyClientPfx: "poolkey-", Namespace: "ns-a"}),
		"flags":  map[string]any{},
		"facts":  map[string]any{"namespace_rbac": map[string]any{"allowed": true}},
	}
	if evalAllow(t, input) {
		t.Fatal("legacy key- client must not regain signed URL access through RBAC under a custom prefix")
	}
}
