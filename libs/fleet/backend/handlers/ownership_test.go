package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"

	"github.com/trycua/cloud/pkg/featureflags"
)

// Namespace ownership is an authorization decision, and since it moved out of
// the handlers it is no longer one this package can answer on its own. So these
// tests come in two halves.
//
// The first half exercises the FactProvider directly: what it probes, how often,
// and what it does when the apiserver will not answer. That is all this package
// still owns.
//
// The second half runs requests through the real authorization stage —
// auth.RouteMiddleware for the production route, over the production tree, with
// the production provider registered — and asserts the status the caller gets
// together with the number of probes it cost. Those two numbers together are
// the whole behaviour: the same 403 reached with one probe or with none is not
// the same system on a route that serves every noVNC asset.
//
// The stage is put in front of a stub rather than the real handler on purpose.
// h.Svc dials {service}.{namespace}.svc.cluster.local, which in a test
// is a DNS timeout; what is under test here is which requests get that far.

func testHandlers() Handlers {
	return Handlers{
		GatewayCfg: config.GatewayConfiguration{
			Scheme:        "http",
			Port:          "80",
			ClusterDomain: "svc.cluster.local",
		},
	}
}

// authorizationStage builds the production authorization middleware for a route
// in front of a stub handler, and reports whether a request reached the stub.
func authorizationStage(t *testing.T, route string) (http.Handler, *bool) {
	t.Helper()
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	auth.LoadOpa()
	auth.RegisterFactProvider(auth.NamespaceRBACFactProvider, NamespaceRBACFacts(testHandlers()))

	reached := false
	stub := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		reached = true
		w.WriteHeader(http.StatusNoContent)
	})
	return auth.RouteContext(route)(auth.RouteMiddleware(route)(stub)), &reached
}

const (
	svcRoute               = "/api/svc/{namespace}/{service}/{path...}"
	signedServiceURLsRoute = "/api/signed-service-urls/{namespace}"
	namespacesRoute        = "/api/namespaces/{name}"
	namespaceList          = "/api/namespaces"
)

// svcRequest builds a /api/svc request with path values + user stamped.
func svcRequest(u *auth.User, ns, service string) *http.Request {
	r := httptest.NewRequest(http.MethodGet, "/api/svc/"+ns+"/"+service+"/", nil)
	r.SetPathValue("namespace", ns)
	r.SetPathValue("service", service)
	r.SetPathValue("path", "")
	if u != nil {
		r = withUser(r, u)
	}
	return r
}

func signedServiceURLsRequest(u *auth.User, namespace string) *http.Request {
	r := httptest.NewRequest(http.MethodPost, "/api/signed-service-urls/"+namespace, nil)
	r.SetPathValue("namespace", namespace)
	if u != nil {
		r = withUser(r, u)
	}
	return r
}

// factRequest builds the request a FactProvider is handed on a route: the user,
// and the route's parameters stamped by the production RouteContext rather than
// by a second copy of what it does.
func factRequest(u *auth.User, route string, params map[string]string) *http.Request {
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	for key, value := range params {
		r.SetPathValue(key, value)
	}
	if u != nil {
		r = withUser(r, u)
	}
	var stamped *http.Request
	auth.RouteContext(route)(http.HandlerFunc(func(_ http.ResponseWriter, got *http.Request) {
		stamped = got
	})).ServeHTTP(httptest.NewRecorder(), r)
	return stamped
}

const probePath = "/apis/rbac.authorization.k8s.io/v1/namespaces/other-ns/rolebindings"

func spaUser(sub string) *auth.User {
	return &auth.User{ID: sub, AZP: "cyclops-cs-spa"}
}

// ── The provider ────────────────────────────────────────────────────────────

func TestNamespaceRBACFacts_ProbeShape(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	provider := NamespaceRBACFacts(testHandlers())
	facts, err := provider.LoadFacts(context.Background(),
		factRequest(spaUser("test-uuid"), svcRoute, map[string]string{"namespace": "other-ns"}))
	if err != nil {
		t.Fatalf("load facts: %v", err)
	}
	if facts["allowed"] != true {
		t.Fatalf("facts = %#v, want allowed:true", facts)
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected exactly one probe, got %d", len(fk.requests))
	}
	probe := fk.requests[0]
	if probe.method != http.MethodGet || probe.path != probePath {
		t.Fatalf("probe = %s %q, want GET %s", probe.method, probe.path, probePath)
	}
	if got := probe.headers.Get("Impersonate-User"); got != "oidc:test-uuid" {
		t.Fatalf("Impersonate-User = %q, want oidc:test-uuid", got)
	}
	if got := probe.headers.Get("Impersonate-Group"); got != "oidc:cyclops-cs-tenants" {
		t.Fatalf("Impersonate-Group = %q, want oidc:cyclops-cs-tenants", got)
	}
}

// The {name} parameter is what /api/namespaces/{name} binds, and the provider
// has to reach the same namespace the Rego does from it — see
// TestOwnedNamespaceMatchesRego for the other half of that agreement.
func TestNamespaceRBACFacts_ReadsTheNameParameter(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	if _, err := NamespaceRBACFacts(testHandlers()).LoadFacts(context.Background(),
		factRequest(spaUser("test-uuid"), namespacesRoute, map[string]string{"name": "other-ns"})); err != nil {
		t.Fatalf("load facts: %v", err)
	}
	if len(fk.requests) != 1 || fk.requests[0].path != probePath {
		t.Fatalf("probes = %#v, want one GET %s", fk.requests, probePath)
	}
}

// The TTL cache that made the old handler check affordable has to survive the
// move. The policy library's own fact cache is per request, so if the provider
// had reimplemented the probe instead of wrapping it, this would be 3.
func TestNamespaceRBACFacts_VerdictCachedAcrossRequests(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	provider := NamespaceRBACFacts(testHandlers())
	for i := 0; i < 3; i++ {
		facts, err := provider.LoadFacts(context.Background(),
			factRequest(spaUser("test-uuid"), svcRoute, map[string]string{"namespace": "other-ns"}))
		if err != nil || facts["allowed"] != true {
			t.Fatalf("load %d: facts = %#v, err = %v", i, facts, err)
		}
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected 1 probe across 3 loads (cached), got %d", len(fk.requests))
	}
}

func TestNamespaceRBACFacts_NegativeVerdictCached(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	provider := NamespaceRBACFacts(testHandlers())
	for i := 0; i < 2; i++ {
		facts, err := provider.LoadFacts(context.Background(),
			factRequest(spaUser("intruder-uuid"), svcRoute, map[string]string{"namespace": "other-ns"}))
		if err != nil || facts["allowed"] != false {
			t.Fatalf("load %d: facts = %#v, err = %v", i, facts, err)
		}
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected 1 probe across 2 loads (negative cached), got %d", len(fk.requests))
	}
}

func TestNamespaceRBACFacts_CacheIsPerUser(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	provider := NamespaceRBACFacts(testHandlers())
	for _, sub := range []string{"user-a", "user-b"} {
		if _, err := provider.LoadFacts(context.Background(),
			factRequest(spaUser(sub), svcRoute, map[string]string{"namespace": "other-ns"})); err != nil {
			t.Fatalf("load for %s: %v", sub, err)
		}
	}
	// A cached verdict for user-a must not leak to user-b.
	if len(fk.requests) != 2 {
		t.Fatalf("expected 2 probes (one per user), got %d", len(fk.requests))
	}
}

// An unreachable apiserver is a FactUnavailableError rather than a bare one,
// which is what keeps the response a 502 instead of a 500; and it is not cached,
// so a flapping apiserver cannot pin a verdict.
func TestNamespaceRBACFacts_UnavailableIsTypedAndUncached(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusInternalServerError, `oops`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	provider := NamespaceRBACFacts(testHandlers())
	for i := 0; i < 2; i++ {
		_, err := provider.LoadFacts(context.Background(),
			factRequest(spaUser("test-uuid"), svcRoute, map[string]string{"namespace": "other-ns"}))
		var unavailable *auth.FactUnavailableError
		if !errors.As(err, &unavailable) {
			t.Fatalf("load %d: err = %v, want a *auth.FactUnavailableError", i, err)
		}
		if unavailable.Namespace != auth.NamespaceRBACFactNamespace {
			t.Fatalf("error names facts %q, want %q", unavailable.Namespace, auth.NamespaceRBACFactNamespace)
		}
	}
	if len(fk.requests) != 2 {
		t.Fatalf("expected 2 probes (errors uncached), got %d", len(fk.requests))
	}
}

// Neither degenerate input is a policy judgement: they are questions the probe
// cannot form. Both deny, and neither costs a round trip.
func TestNamespaceRBACFacts_DegenerateInputsDenyWithoutProbing(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	provider := NamespaceRBACFacts(testHandlers())
	for _, testCase := range []struct {
		name    string
		request *http.Request
	}{
		{"no user", factRequest(nil, svcRoute, map[string]string{"namespace": "ns-a"})},
		{"empty sub", factRequest(spaUser(""), svcRoute, map[string]string{"namespace": "ns-a"})},
		{"no namespace", factRequest(spaUser("u-1"), namespaceList, map[string]string{})},
		{"empty namespace", factRequest(spaUser("u-1"), svcRoute, map[string]string{"namespace": ""})},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			facts, err := provider.LoadFacts(context.Background(), testCase.request)
			if err != nil {
				t.Fatalf("load facts: %v", err)
			}
			if facts["allowed"] != false {
				t.Fatalf("facts = %#v, want allowed:false", facts)
			}
		})
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no probes, got %d", len(fk.requests))
	}
}

// ── The authorization stage ─────────────────────────────────────────────────

// The noVNC case, end to end: a burst of asset requests from one user for one
// namespace costs one probe, not one each. This is the property the move had to
// preserve — the policy library's fact cache is per request, and only the
// probe's own TTL cache spans them.
func TestSvcRoute_SPAUser_OwnedNamespace_AllowedAndProbedOnce(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	for i := 0; i < 5; i++ {
		w := httptest.NewRecorder()
		stage.ServeHTTP(w, svcRequest(spaUser("test-uuid"), "other-ns", "novnc"))
		if w.Code != http.StatusNoContent || !*reached {
			t.Fatalf("request %d: status = %d, reached = %v; want 204 and the handler reached; body = %s",
				i, w.Code, *reached, w.Body.String())
		}
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected 1 probe across 5 requests (TTL-cached), got %d", len(fk.requests))
	}
}

func TestSvcRoute_SPAUser_NonOwnedNamespace_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status","status":"Failure","reason":"Forbidden"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(spaUser("intruder-uuid"), "other-ns", "novnc"))

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s",
			w.Code, *reached, w.Body.String())
	}
}

func TestSvcRoute_OAuth2ProxyUser_NonOwnedNamespace_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status","status":"Failure","reason":"Forbidden"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(&auth.User{ID: "intruder-uuid", AZP: "oauth2-proxy", Email: "x@y.z"}, "other-ns", "novnc"))

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s",
			w.Code, *reached, w.Body.String())
	}
}

// Per-key clients are bound to their namespace claim, which is a check over the
// token: allowed or denied, it must never cost a probe.
func TestSvcRoute_PerKey_NamespaceMatch_Allowed_NoProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(&auth.User{ID: "svc-acct", AZP: "key-mypool", Namespace: "mypool"}, "mypool", "novnc"))

	if w.Code != http.StatusNoContent || !*reached {
		t.Fatalf("status = %d, reached = %v; want 204 and the handler reached; body = %s",
			w.Code, *reached, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe for per-key client, got %d request(s)", len(fk.requests))
	}
}

func TestSvcRoute_PerKey_NamespaceMismatch_Forbidden_NoProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(&auth.User{ID: "svc-acct", AZP: "key-mypool", Namespace: "mypool"}, "other-ns", "novnc"))

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s",
			w.Code, *reached, w.Body.String())
	}
	// The fake would allow this one. A probe here would be a per-key service
	// account borrowing somebody else's namespace.
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe for per-key client, got %d request(s)", len(fk.requests))
	}
}

func TestSignedServiceURLsRoute_PerKey_NamespaceMismatch_ForbiddenBeforeHandler(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, signedServiceURLsRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, signedServiceURLsRequest(&auth.User{ID: "svc-acct", AZP: "key-mypool", Namespace: "mypool"}, "other-ns"))

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s", w.Code, *reached, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no Kubernetes probe for denied per-key request, got %d request(s)", len(fk.requests))
	}
}

func TestSvcRoute_GitHubPrincipal_ClaimAllows_NoProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(&auth.User{
		ID:                "user-123",
		AZP:               "github-oidc",
		PrincipalType:     auth.PrincipalTypeGitHubOIDC,
		AllowedNamespaces: []string{"allowed-ns"},
	}, "allowed-ns", "novnc"))

	if w.Code != http.StatusNoContent || !*reached {
		t.Fatalf("status = %d, reached = %v; want 204 and the handler reached; body = %s",
			w.Code, *reached, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe, got %d request(s)", len(fk.requests))
	}
}

// This is All(cheapDeny, factLeaf) on a production route: probe_eligible is
// false for a GitHub token, so the conjunction is decided before the leaf
// carrying the provider is reached. The fake would have said yes.
func TestSvcRoute_GitHubPrincipal_UngrantedNamespace_Forbidden_NoProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(&auth.User{
		ID:                "user-123",
		AZP:               "github-oidc",
		PrincipalType:     auth.PrincipalTypeGitHubOIDC,
		AllowedNamespaces: []string{"allowed-ns"},
	}, "other-ns", "novnc"))

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s",
			w.Code, *reached, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe, got %d request(s)", len(fk.requests))
	}
}

// And this is the other short-circuit: a cheap sibling of the ownership
// conjunct — the surface's DNS-label check — denying before it runs at all.
func TestSvcRoute_InvalidNamespace_Forbidden_NoProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, svcRequest(spaUser("test-uuid"), "Bad_NS", "novnc"))

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s",
			w.Code, *reached, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe, got %d request(s)", len(fk.requests))
	}
}

// The failure mode the move had to preserve: an unreachable apiserver is a 502
// the caller can retry, not a 500. Through the lattice it would have been a 500
// without FactUnavailableError.
func TestSvcRoute_K8sUnavailable_FailsClosedWith502(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusInternalServerError, `oops`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, svcRoute)
	for i := 0; i < 2; i++ {
		w := httptest.NewRecorder()
		stage.ServeHTTP(w, svcRequest(spaUser("test-uuid"), "other-ns", "novnc"))
		if w.Code != http.StatusBadGateway || *reached {
			t.Fatalf("request %d: status = %d, reached = %v; want 502 and no handler; body = %s",
				i, w.Code, *reached, w.Body.String())
		}
	}
	// Indeterminate verdicts must not be cached: both requests probe.
	if len(fk.requests) != 2 {
		t.Fatalf("expected 2 probes (errors uncached), got %d", len(fk.requests))
	}
}

func TestNamespacesRoute_GetOtherTenant_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	stage, reached := authorizationStage(t, namespacesRoute)
	r := httptest.NewRequest(http.MethodGet, "/api/namespaces/other-ns", nil)
	r.SetPathValue("name", "other-ns")
	r = withUser(r, spaUser("intruder-uuid"))
	w := httptest.NewRecorder()
	stage.ServeHTTP(w, r)

	if w.Code != http.StatusForbidden || *reached {
		t.Fatalf("status = %d, reached = %v; want 403 and no handler; body = %s",
			w.Code, *reached, w.Body.String())
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected exactly one probe, got %d", len(fk.requests))
	}
}

// The other three routes on the namespaces surface never ran this check, and
// still must not: DELETE goes to the K8s API impersonated, where Capsule is the
// boundary, and the list route names no namespace to probe for in the first
// place.
func TestNamespacesSurface_RoutesOutsideTheBoundaryDoNotProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	for _, testCase := range []struct {
		name    string
		route   string
		method  string
		path    string
		params  map[string]string
		wantHit bool
	}{
		{name: "delete", route: namespacesRoute, method: http.MethodDelete, path: "/api/namespaces/other-ns",
			params: map[string]string{"name": "other-ns"}, wantHit: true},
		{name: "list", route: namespaceList, method: http.MethodGet, path: "/api/namespaces",
			params: map[string]string{}, wantHit: true},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			stage, reached := authorizationStage(t, testCase.route)
			r := httptest.NewRequest(testCase.method, testCase.path, nil)
			for key, value := range testCase.params {
				r.SetPathValue(key, value)
			}
			r = withUser(r, spaUser("test-uuid"))
			w := httptest.NewRecorder()
			stage.ServeHTTP(w, r)

			if (w.Code == http.StatusNoContent) != testCase.wantHit || *reached != testCase.wantHit {
				t.Fatalf("status = %d, reached = %v; want the handler reached = %v; body = %s",
					w.Code, *reached, testCase.wantHit, w.Body.String())
			}
		})
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no probes off the ownership boundary, got %d", len(fk.requests))
	}
}
