package auth

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"testing"

	"github.com/trycua/cloud/pkg/featureflags"
)

// This file records what the authorization stage answers, for every route it
// guards, crossed with every principal shape TokenAuthMiddleware can put on a
// request. The record is a checked-in table.
//
// It exists to make a refactor of the policy provable rather than plausible.
// Splitting one route-aware module into a shared base composed with per-surface
// modules is only correct if no cell of this table moves; a table captured
// against the monolith first is evidence of that, whereas one written afterwards
// would only restate whatever the new modules happen to do.
//
// So: if a cell moves, the change is wrong. Do not regenerate to match it. The
// regeneration flag is here for the case where a row is *added* — a new route,
// a new principal shape, a new parameter case — which is the only edit that
// should ever produce a diff with a clear conscience.
//
// What it drives is the real thing: RouteTree(route) is what main.go dispatches
// that route to, compiled through the same optimizer pipeline PolicyMiddleware
// uses, evaluated over an input built by newRequestPolicyInput. So a route that
// changes surface, or a surface that changes tree, shows up here.
//
// What it does not cover:
//
//   - Which HTTP status a verdict becomes. That is PolicyMiddleware's mapping and
//     policy_middleware_test.go's subject; this table records allow / deny /
//     error, the three values the plan can produce.
//   - Anything downstream of the policy. An "allow" in this table means "the
//     policy stage let it through", never "the request succeeds". It used to
//     mean considerably less than that on /api/svc and GET
//     /api/namespaces/{name}, where the namespace-ownership boundary was a Go
//     check in the handler and invisible here; those routes now carry it as a
//     policy conjunct, and this table records it.
//   - Token validation. Every principal here is already authenticated; what
//     varies is what the token turned out to be.

// characterizationMethods is the method dimension. It is applied to every route
// rather than only to the methods that route registers: RouteContext stamps the
// route name before the mux has any say, and the policy reads input.method
// directly, so a rule that silently depends on a method the route never serves
// is exactly the kind of thing worth pinning.
var characterizationMethods = []string{
	http.MethodGet,
	http.MethodPost,
	http.MethodPatch,
	http.MethodDelete,
	"QUERY",
}

// characterizationAdminSub is the one sub in admin_subs while this table is
// generated. Every other shape's sub is deliberately different, so the admin row
// is the only one that can satisfy is_admin.
const characterizationAdminSub = "admin-1"

// The three answers the namespace-RBAC probe can give, encoded as namespaces so
// that a route case selects one. A fact provider is a request-time input to the
// policy exactly like the token is, and a table that only ever fed it "allowed"
// would record half a boundary: the deny is what the boundary is *for*, and the
// error is a third verdict — the one that decides between a 502 and a 500.
const (
	characterizationOwnedNamespace       = "ns-a"
	characterizationUnownedNamespace     = "ns-b"
	characterizationUnreachableNamespace = "ns-err"
)

// characterizationFacts stands in for handlers.NamespaceRBACFacts. It answers
// from the namespace alone and asks nothing about the principal, which is the
// production provider's contract too — which principals are worth probing is
// authz_ownership.rego's decision, and pinning it here is part of the point.
type characterizationFacts struct{}

func (characterizationFacts) CacheKey() string { return NamespaceRBACFactProvider }

func (characterizationFacts) LoadFacts(_ context.Context, request *http.Request) (FactSet, error) {
	switch OwnedNamespace(request.Context()) {
	case characterizationOwnedNamespace:
		return FactSet{"allowed": true}, nil
	case characterizationUnreachableNamespace:
		return nil, &FactUnavailableError{
			Namespace: NamespaceRBACFactNamespace,
			Err:       errors.New("apiserver unavailable"),
		}
	default:
		return FactSet{"allowed": false}, nil
	}
}

type principalShape struct {
	name string
	user *User
}

// characterizationPrincipals is every shape a request can carry by the time the
// policy runs. The set is drawn from what TokenAuthMiddleware and
// applyUserKeyIdentity actually produce, not from what the policy happens to
// name — which is the point, since a shape the policy does not name is a shape
// whose verdict nobody chose.
func characterizationPrincipals() []principalShape {
	return []principalShape{
		{name: "spa", user: &User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser}},
		{name: "cua-cli", user: &User{ID: "u-1", AZP: "cua-cli", PrincipalType: PrincipalTypeUser}},
		{name: "admin-spa", user: &User{ID: characterizationAdminSub, AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser}},
		{name: "per-key-owned-ns", user: &User{ID: "svc-1", AZP: "key-ns-a", Namespace: "ns-a", PrincipalType: PrincipalTypeUser}},
		{name: "per-key-other-ns", user: &User{ID: "svc-1", AZP: "key-ns-z", Namespace: "ns-z", PrincipalType: PrincipalTypeUser}},
		{name: "user-key", user: &User{ID: "u-1", AZP: "ukey-u-1", PrincipalType: PrincipalTypeUser}},
		{name: "user-key-verified", user: &User{ID: "u-1", AZP: "ukey-u-1", PrincipalType: PrincipalTypeUserKey}},
		{name: "github-oidc", user: &User{
			ID:                "owner-1",
			AZP:               "github-oidc",
			PrincipalType:     PrincipalTypeGitHubOIDC,
			Repository:        "trycua/cloud",
			AllowedNamespaces: []string{"ns-a"},
		}},
		{name: "oauth2-proxy", user: &User{ID: "u-1", AZP: "oauth2-proxy"}},
		{name: "dcr-client", user: &User{ID: "u-1", AZP: "7c1d9f2a-0e3b-4a51-9c8e-2f6b1d7a4c05", PrincipalType: PrincipalTypeUser}},
		{name: "empty-sub", user: &User{ID: "", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser}},
	}
}

// routeCase is one route with one set of path parameters. Several cases per
// route is how the parameter-shaped rules — DNS-label validation, the per-key
// namespace binding, the infra k8s path list, the GitHub namespace grant — get
// both of their answers recorded rather than just whichever one a single
// example happened to hit.
type routeCase struct {
	name   string
	params map[string]string
	// path is the request URL. Only /api/k8s reads it (through
	// pool_admission's own params.path), but PolicyMiddleware builds an
	// input.path for every leaf, so every case sets one.
	path string
	body string
}

// characterizationCases lists the parameter cases for every route. A route with
// no entry fails the test rather than being skipped — an unlisted route is one
// whose verdicts nobody recorded.
func TestChatConversationPatchUsesExistingAuthorizationSurface(t *testing.T) {
	const route = "/api/chat/conversations/{id}"
	if surface, ok := RouteSurface(route); !ok || surface != "chat" {
		t.Fatalf("RouteSurface(%q) = (%q, %t), want (chat, true)", route, surface, ok)
	}
	if !slices.Contains(characterizationMethods, http.MethodPatch) {
		t.Fatalf("characterizationMethods = %v, missing PATCH", characterizationMethods)
	}
	if len(characterizationCases()[route]) == 0 {
		t.Fatalf("characterizationCases missing %q", route)
	}
}

func characterizationCases() map[string][]routeCase {
	cases := map[string][]routeCase{}

	simple := func(route, path string) {
		cases[route] = []routeCase{{name: "plain", params: map[string]string{}, path: path}}
	}
	simple("/api/config", "/api/config")
	simple("/api/analytics/session", "/api/analytics/session")
	simple("/api/analytics/attribution", "/api/analytics/attribution")
	simple("/api/state/query", "/api/state/query")
	simple("/api/usage/overview", "/api/usage/overview")
	simple("/api/usage/pool", "/api/usage/pool")
	simple("/api/usage/browser-timings", "/api/usage/browser-timings")
	simple("/api/chat/conversations", "/api/chat/conversations")
	simple("/api/billing/summary", "/api/billing/summary")
	simple("/api/billing/usage", "/api/billing/usage")
	simple("/api/billing/setup-session", "/api/billing/setup-session")
	simple("/api/billing/portal-session", "/api/billing/portal-session")
	simple("/api/keys", "/api/keys")
	simple("/api/namespaces", "/api/namespaces")
	simple("/api/user-keys", "/api/user-keys")
	simple("/api/github-trust-policies", "/api/github-trust-policies")
	simple("/api/admin/feature-flags", "/api/admin/feature-flags")
	cases["/api/admin/feature-flags/{key}"] = []routeCase{{name: "key", params: map[string]string{"key": "example"}, path: "/api/admin/feature-flags/example"}}

	withID := func(route, prefix string) {
		cases[route] = []routeCase{{
			name:   "id",
			params: map[string]string{"id": "id-1"},
			path:   prefix + "/id-1",
		}}
	}
	withID("/api/keys/{id}", "/api/keys")
	withID("/api/chat/conversations/{id}", "/api/chat/conversations")
	withID("/api/user-keys/{id}", "/api/user-keys")
	withID("/api/github-trust-policies/{id}", "/api/github-trust-policies")
	cases["/api/chat/conversations/{id}/turns"] = []routeCase{{
		name:   "id",
		params: map[string]string{"id": "id-1"},
		path:   "/api/chat/conversations/id-1/turns",
	}}

	// /api/namespaces/{name} needs all three fact answers, and only on GET: the
	// ownership conjunct binds to that one method, and DELETE sharing the route
	// is what makes the method dimension load-bearing here rather than merely
	// thorough.
	cases["/api/namespaces/{name}"] = []routeCase{
		{
			name:   "name",
			params: map[string]string{"name": characterizationOwnedNamespace},
			path:   "/api/namespaces/" + characterizationOwnedNamespace,
		},
		{
			name:   "unowned-name",
			params: map[string]string{"name": characterizationUnownedNamespace},
			path:   "/api/namespaces/" + characterizationUnownedNamespace,
		},
		{
			name:   "unreachable-name",
			params: map[string]string{"name": characterizationUnreachableNamespace},
			path:   "/api/namespaces/" + characterizationUnreachableNamespace,
		},
	}

	// /api/svc gates on the DNS-label shape of its parameters,
	// on the namespace claim matching the path for per-key and GitHub tokens,
	// and on the RBAC fact for everyone else. owned-ns / other-ns / unreachable-ns
	// are the three answers the fact provider gives.
	proxyCases := func(withPath bool) []routeCase {
		suffix := ""
		params := func(namespace, service string) map[string]string {
			out := map[string]string{"namespace": namespace, "service": service}
			if withPath {
				out["path"] = "v1/status"
			}
			return out
		}
		if withPath {
			suffix = "/v1/status"
		}
		return []routeCase{
			{name: "owned-ns", params: params(characterizationOwnedNamespace, "svc-a"), path: "/api/svc/ns-a/svc-a" + suffix},
			{name: "other-ns", params: params(characterizationUnownedNamespace, "svc-a"), path: "/api/svc/ns-b/svc-a" + suffix},
			{name: "unreachable-ns", params: params(characterizationUnreachableNamespace, "svc-a"), path: "/api/svc/ns-err/svc-a" + suffix},
			{name: "invalid-ns", params: params("Not_A_Label", "svc-a"), path: "/api/svc/Not_A_Label/svc-a" + suffix},
			{name: "invalid-service", params: params(characterizationOwnedNamespace, "Svc_A"), path: "/api/svc/ns-a/Svc_A" + suffix},
			{name: "empty-ns", params: params("", "svc-a"), path: "/api/svc//svc-a" + suffix},
		}
	}
	cases["/api/svc/{namespace}/{service}"] = proxyCases(false)
	cases["/api/svc/{namespace}/{service}/{path...}"] = proxyCases(true)

	// /api/signed-service-urls mirrors the /api/svc parameter logic: DNS-label shape,
	// the per-key namespace binding, and the RBAC fact for everyone else —
	// its handler acts with the pod ServiceAccount, so the ownership conjunct
	// is the boundary and all three fact answers matter here too.
	cases["/api/signed-service-urls/{namespace}"] = []routeCase{
		{name: "owned-ns", params: map[string]string{"namespace": characterizationOwnedNamespace}, path: "/api/signed-service-urls/ns-a"},
		{name: "other-ns", params: map[string]string{"namespace": characterizationUnownedNamespace}, path: "/api/signed-service-urls/ns-b"},
		{name: "unreachable-ns", params: map[string]string{"namespace": characterizationUnreachableNamespace}, path: "/api/signed-service-urls/ns-err"},
		{name: "invalid-ns", params: map[string]string{"namespace": "Not_A_Label"}, path: "/api/signed-service-urls/Not_A_Label"},
		{name: "empty-ns", params: map[string]string{"namespace": ""}, path: "/api/signed-service-urls/"},
	}
	cases["/api/signed-service-urls/{namespace}/{id}"] = []routeCase{
		{name: "owned-ns", params: map[string]string{"namespace": characterizationOwnedNamespace, "id": "5cd7f3e4-5390-4c0c-a93b-dd18116d367c"}, path: "/api/signed-service-urls/ns-a/5cd7f3e4-5390-4c0c-a93b-dd18116d367c"},
		{name: "other-ns", params: map[string]string{"namespace": characterizationUnownedNamespace, "id": "5cd7f3e4-5390-4c0c-a93b-dd18116d367c"}, path: "/api/signed-service-urls/ns-b/5cd7f3e4-5390-4c0c-a93b-dd18116d367c"},
		{name: "unreachable-ns", params: map[string]string{"namespace": characterizationUnreachableNamespace, "id": "5cd7f3e4-5390-4c0c-a93b-dd18116d367c"}, path: "/api/signed-service-urls/ns-err/5cd7f3e4-5390-4c0c-a93b-dd18116d367c"},
		{name: "empty-id", params: map[string]string{"namespace": characterizationOwnedNamespace, "id": ""}, path: "/api/signed-service-urls/ns-a/"},
	}

	// /api/k8s carries the richest parameter logic in the policy: the infra-path
	// list, the admin escape hatch over it, the GitHub namespace grant, and the
	// pool-admission leaf reading the body.
	k8sPaths := []struct {
		name string
		path string
		body string
	}{
		{name: "namespaced-pods", path: "api/v1/namespaces/ns-a/pods"},
		{name: "cluster-nodes", path: "api/v1/nodes"},
		{name: "cluster-pods", path: "api/v1/pods"},
		{name: "cyclops-configmaps", path: "api/v1/namespaces/cyclops-cs/configmaps/batch-configs"},
		{name: "capsule-tenants", path: "apis/capsule.clastix.io/v1beta2/tenants"},
		{name: "cluster-claims", path: "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims"},
		{name: "granted-ns-pools", path: "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"},
		{name: "ungranted-ns-pools", path: "apis/cua.ai/v1/namespaces/ns-b/osgymworkspacepools"},
		{name: "granted-ns-claims", path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"},
		// The event feed, in each of the four addressing forms the apiserver
		// serves it under, crossed with the two API groups that serve it. These
		// are the only k8s paths every principal shape is denied on — including
		// admin-spa, which the infra-path rows show getting an escape hatch — so
		// a deny that stopped covering one of them shows up here as an allow
		// appearing in a column that has none.
		{name: "cluster-events", path: "api/v1/events"},
		{name: "namespaced-events", path: "api/v1/namespaces/ns-a/events"},
		{name: "group-cluster-events", path: "apis/events.k8s.io/v1/events"},
		{name: "group-namespaced-events", path: "apis/events.k8s.io/v1/namespaces/ns-a/events"},
		{name: "legacy-watch-events", path: "api/v1/watch/events"},
		{name: "legacy-watch-namespaced-events", path: "api/v1/watch/namespaces/ns-a/events"},
		// The same legacy alias over the cluster-wide collections the infra deny
		// exists to hide. These stream what api/v1/pods and api/v1/nodes list, so
		// their rows must read exactly like those two do.
		{name: "legacy-watch-cluster-pods", path: "api/v1/watch/pods"},
		{name: "legacy-watch-cluster-nodes", path: "api/v1/watch/nodes"},
		// A group whose name merely starts with the events group. It recorded an
		// allow when this surface was a denylist, which is what made the event
		// deny falsifiable; under the allowlist it is denied like anything else
		// nobody enumerated, and the precision of is_event_k8s_path is pinned
		// directly in authz_k8s_test.rego instead.
		{name: "event-group-collision", path: "apis/events.k8s.io.evil/v1/events"},

		// The allowlist's own shapes. The collection cases above cover GET and
		// POST; these cover the item and subresource forms, the one core read
		// that is a subresource, and a discovery path.
		{name: "claim-item", path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-1"},
		{name: "claim-status", path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-1/status"},
		{name: "pod-logs", path: "api/v1/namespaces/ns-a/pods/pod-1/log"},
		{name: "discovery", path: "openapi/v2"},
		// And the decision the allowlist makes that a reader is most likely to
		// want to check: reading a Secret through the proxy is not on it.
		{name: "namespaced-secrets", path: "api/v1/namespaces/ns-a/secrets/ecr-credentials"},
		{
			name: "pool-create-allowed-image",
			path: "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools",
			body: `{"spec":{"template":{"containerDiskImage":"296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace:latest","imagePullSecret":"ecr-credentials"}}}`,
		},
		{
			name: "pool-create-disallowed-image",
			path: "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools",
			body: `{"spec":{"template":{"containerDiskImage":"evil.example/workspace:latest","imagePullSecret":"ecr-credentials"}}}`,
		},
	}
	k8s := make([]routeCase, 0, len(k8sPaths))
	for _, k8sPath := range k8sPaths {
		body := k8sPath.body
		if body == "" {
			body = "{}"
		}
		k8s = append(k8s, routeCase{
			name:   k8sPath.name,
			params: map[string]string{"path": k8sPath.path},
			path:   "/api/k8s/" + k8sPath.path,
			body:   body,
		})
	}
	cases["/api/k8s/{path...}"] = k8s

	return cases
}

// characterizationRequest rebuilds what the authorization stage is handed:
// RouteContext's route and params, TokenAuthMiddleware's User, and a body the
// raw-body leaves can read.
func characterizationRequest(route string, testCase routeCase, method string, user *User) *http.Request {
	request := httptest.NewRequest(method, "http://cyclops-cs.test"+testCase.path, strings.NewReader(testCase.body))
	if testCase.body == "" {
		request.Body = io.NopCloser(strings.NewReader(""))
	}
	ctx := request.Context()
	ctx = context.WithValue(ctx, routeKey, route)
	ctx = context.WithValue(ctx, paramsKey, testCase.params)
	ctx = context.WithValue(ctx, UserKey, user)
	return request.WithContext(ctx)
}

func verdictName(value truth) string {
	switch value {
	case truthTrue:
		return "allow"
	case truthFalse:
		return "deny"
	default:
		return "error"
	}
}

const characterizationGoldenPath = "testdata/route-authorization-table.txt"

// TestRouteAuthorizationCharacterization is the acceptance criterion for any
// change to how routes are bound to policies or how those policies are
// factored. It fails on any moved cell, and its message says so rather than
// suggesting a regeneration.
func TestRouteAuthorizationCharacterization(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", fmt.Sprintf(`[%q]`, characterizationAdminSub))
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	LoadOpa()
	// The ownership conjunct reaches a FactProvider through the registry, so the
	// table is only reproducible if the provider answering it is. Registered for
	// the whole test rather than per case: what varies is the namespace asked
	// about, which is a route case, not a different provider.
	RegisterFactProvider(NamespaceRBACFactProvider, characterizationFacts{})
	resetFlagsCache()
	// The resolved flags outlive t.Setenv, so drop them again rather than leaving
	// a cached admin_subs behind for whichever test runs next.
	t.Cleanup(resetFlagsCache)

	routes := AuthenticatedRoutes()
	cases := characterizationCases()
	for _, route := range routes {
		if len(cases[route]) == 0 {
			t.Fatalf(`route %q has no entry in characterizationCases, so no verdict of its is
recorded. Add at least one parameter case for it.`, route)
		}
	}
	for route := range cases {
		if _, ok := RouteSurface(route); !ok {
			t.Fatalf("characterizationCases lists %q, which is not an authenticated route", route)
		}
	}

	// One compiled plan per surface, exactly as production shares them.
	plans := map[string]*CompiledPolicy{}
	for _, name := range SurfaceNames() {
		tree, ok := SurfaceTree(name)
		if !ok {
			t.Fatalf("SurfaceNames returned %q, which SurfaceTree does not resolve", name)
		}
		compiled, err := Compile(Optimize(tree, DefaultPipeline()))
		if err != nil {
			t.Fatalf("compile surface %q: %v", name, err)
		}
		plans[name] = compiled
	}

	principals := characterizationPrincipals()
	var lines []string
	for _, route := range routes {
		surface, _ := RouteSurface(route)
		plan := plans[surface]
		for _, testCase := range cases[route] {
			for _, method := range characterizationMethods {
				for _, principal := range principals {
					request := characterizationRequest(route, testCase, method, principal.user)
					result := plan.eval(request.Context(), newRequestPolicyInput(request, plan.bodyBudget))
					lines = append(lines, fmt.Sprintf("%s | %s | %s | %s = %s",
						route, testCase.name, method, principal.name, verdictName(result.truth)))
				}
			}
		}
	}
	sort.Strings(lines)
	got := strings.Join(lines, "\n") + "\n"

	if *updateGolden {
		if err := os.MkdirAll(filepath.Dir(characterizationGoldenPath), 0o755); err != nil {
			t.Fatalf("mkdir testdata: %v", err)
		}
		if err := os.WriteFile(characterizationGoldenPath, []byte(got), 0o644); err != nil {
			t.Fatalf("write %s: %v", characterizationGoldenPath, err)
		}
		return
	}

	want, err := os.ReadFile(characterizationGoldenPath)
	if err != nil {
		t.Fatalf("read %s (generate with: go test ./auth/ -run TestRouteAuthorizationCharacterization -update-golden): %v",
			characterizationGoldenPath, err)
	}
	if got == string(want) {
		return
	}

	diff := characterizationDiff(strings.Split(strings.TrimSuffix(string(want), "\n"), "\n"), lines)
	t.Fatalf(`the route authorization table changed:

%s
Every line above is a request the policy stage answers differently than it did
when this table was recorded. If you are refactoring how the policy is
structured, that is the refactor being wrong, and the fix is in the policy, not
in this file. Regenerate only when the diff is purely added or removed rows —
a new route, principal shape, or parameter case:

    go test ./auth/ -run TestRouteAuthorizationCharacterization -update-golden`, diff)
}

// characterizationDiff reports moved cells separately from added and removed
// rows, because the two mean opposite things: a moved cell is an authorization
// change, and an added or removed row is usually just a wider table.
func characterizationDiff(want, got []string) string {
	split := func(lines []string) map[string]string {
		out := make(map[string]string, len(lines))
		for _, line := range lines {
			key, verdict, found := strings.Cut(line, " = ")
			if !found {
				continue
			}
			out[key] = verdict
		}
		return out
	}
	wanted, current := split(want), split(got)

	var moved, added, removed []string
	for key, verdict := range current {
		previous, existed := wanted[key]
		switch {
		case !existed:
			added = append(added, fmt.Sprintf("  + %s = %s", key, verdict))
		case previous != verdict:
			moved = append(moved, fmt.Sprintf("  ! %s: %s -> %s", key, previous, verdict))
		}
	}
	for key, verdict := range wanted {
		if _, ok := current[key]; !ok {
			removed = append(removed, fmt.Sprintf("  - %s = %s", key, verdict))
		}
	}
	sort.Strings(moved)
	sort.Strings(added)
	sort.Strings(removed)

	var builder strings.Builder
	section := func(title string, entries []string) {
		if len(entries) == 0 {
			return
		}
		fmt.Fprintf(&builder, "%s (%d):\n", title, len(entries))
		for _, entry := range entries {
			builder.WriteString(entry)
			builder.WriteString("\n")
		}
	}
	section("MOVED VERDICTS", moved)
	section("ADDED ROWS", added)
	section("REMOVED ROWS", removed)
	return builder.String()
}
